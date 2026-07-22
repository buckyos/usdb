use super::content::{MinerPassKind, MinerPassState, MintValidationErrorCode};
use super::energy::PassEnergyManagerRef;
use super::energy_formula::{Energy, calc_inheritable_energy};
use super::pass_commit::{PassBlockMutation, PassBlockMutationCollector};
use crate::config::ConfigManagerRef;
use crate::storage::{MinerPassInfo, MinerPassSnapshotInfo, MinerPassStorageRef};
use bitcoincore_rpc::bitcoin::Txid;
use ord::InscriptionId;
use ordinals::SatPoint;
use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use usdb_util::{BtcScriptHash, address_string_to_script_hash};

pub struct PassMintInscriptionInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,

    // The minting transaction info
    pub mint_txid: Txid,
    pub mint_block_height: u32,
    pub mint_owner: BtcScriptHash, // The owner address who minted the pass

    pub satpoint: SatPoint,

    // The inscription content
    pub mint_version: u32,
    pub pass_kind: MinerPassKind,
    pub usdb_main: String,
    pub leader_pass_id: Option<InscriptionId>,
    pub leader_btc_addr: Option<String>,
    pub prev: Vec<InscriptionId>,
}

pub struct InvalidPassMintInscriptionInfo {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub mint_txid: Txid,
    pub mint_block_height: u32,
    pub mint_owner: BtcScriptHash,
    pub satpoint: SatPoint,
    pub error_code: String,
    pub error_reason: String,
}

/// Leader reference encoding used by a collab pass mint payload.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CollabLeaderRefKind {
    /// Fixed `leader_pass_id` binding.
    LeaderPassId,
    /// BTC address binding that resolves to the address owner's active standard pass at query height.
    LeaderBtcAddr,
}

/// Result of resolving a collab pass Leader reference at one BTC height.
pub struct ResolvedCollabLeader {
    /// Leader reference kind declared by the collab pass.
    pub leader_ref_kind: CollabLeaderRefKind,
    /// Original Leader reference value from the collab pass mint payload.
    pub leader_ref_value: String,
    /// Historical active standard Leader snapshot at the query height.
    pub leader: MinerPassSnapshotInfo,
}

struct MintStateValidationError {
    code: MintValidationErrorCode,
    reason: String,
}

pub struct MinerPassManager {
    config: ConfigManagerRef,
    storage: MinerPassStorageRef,
    energy_manager: PassEnergyManagerRef,
    current_block_collector: Mutex<Option<PassBlockMutationCollector>>,
}

impl MinerPassManager {
    pub fn new(
        config: ConfigManagerRef,
        miner_pass_storage: MinerPassStorageRef,
        energy_manager: PassEnergyManagerRef,
    ) -> Result<Self, String> {
        Ok(Self {
            config,
            storage: miner_pass_storage,
            energy_manager,
            current_block_collector: Mutex::new(None),
        })
    }

    pub fn miner_pass_storage(&self) -> &MinerPassStorageRef {
        &self.storage
    }

    pub fn begin_block_mutation_collection(&self, block_height: u32) -> Result<(), String> {
        let mut current = self.current_block_collector.lock().unwrap();
        if current.is_some() {
            let msg = format!(
                "Pass block mutation collector is already active when starting block {}",
                block_height
            );
            error!("{}", msg);
            return Err(msg);
        }
        *current = Some(PassBlockMutationCollector::new(block_height));
        Ok(())
    }

    pub fn take_block_mutation_collector(
        &self,
        block_height: u32,
    ) -> Result<PassBlockMutationCollector, String> {
        let mut current = self.current_block_collector.lock().unwrap();
        let collector = current.take().ok_or_else(|| {
            let msg = format!(
                "Pass block mutation collector is not active when finalizing block {}",
                block_height
            );
            error!("{}", msg);
            msg
        })?;
        if collector.block_height() != block_height {
            let msg = format!(
                "Pass block mutation collector height mismatch: expected_block_height={}, collector_block_height={}",
                block_height,
                collector.block_height()
            );
            error!("{}", msg);
            return Err(msg);
        }
        Ok(collector)
    }

    pub fn clear_block_mutation_collection(&self) {
        let mut current = self.current_block_collector.lock().unwrap();
        *current = None;
    }

    pub fn has_active_block_mutation_collection(&self) -> bool {
        self.current_block_collector.lock().unwrap().is_some()
    }

    fn resolve_leader_btc_owner_for_mint(
        &self,
        mint_info: &PassMintInscriptionInfo,
    ) -> Result<Option<BtcScriptHash>, String> {
        let Some(leader_btc_addr) = mint_info.leader_btc_addr.as_deref() else {
            return Ok(None);
        };

        let network = self.config.config().bitcoin.network();
        address_string_to_script_hash(leader_btc_addr, &network)
            .map(Some)
            .map_err(|e| {
                let msg = format!(
                    "Failed to normalize collab leader_btc_addr while minting pass: inscription_id={}, leader_btc_addr={}, network={}, error={}",
                    mint_info.inscription_id, leader_btc_addr, network, e
                );
                error!("{}", msg);
                msg
            })
    }

    /// Resolve the Leader referenced by a collab pass at a specific BTC height.
    ///
    /// `leader_pass_id` is a fixed binding to that pass id. `leader_btc_addr`
    /// is resolved through the configured BTC network, then matched to the
    /// address owner's active standard pass snapshot at `block_height`.
    pub fn resolve_collab_leader_at_height(
        &self,
        collab_pass: &MinerPassInfo,
        block_height: u32,
    ) -> Result<Option<ResolvedCollabLeader>, String> {
        if collab_pass.pass_kind != MinerPassKind::Collab {
            return Ok(None);
        }

        match (
            collab_pass.leader_pass_id.as_ref(),
            collab_pass.leader_btc_addr.as_deref(),
        ) {
            (Some(leader_pass_id), None) => Ok(self
                .resolve_leader_pass_id_at_height(leader_pass_id, block_height)?
                .map(|leader| ResolvedCollabLeader {
                    leader_ref_kind: CollabLeaderRefKind::LeaderPassId,
                    leader_ref_value: leader_pass_id.to_string(),
                    leader,
                })),
            (None, Some(leader_btc_addr)) => Ok(self
                .resolve_leader_btc_addr_at_height(leader_btc_addr, block_height)?
                .map(|leader| ResolvedCollabLeader {
                    leader_ref_kind: CollabLeaderRefKind::LeaderBtcAddr,
                    leader_ref_value: leader_btc_addr.to_string(),
                    leader,
                })),
            (None, None) => Ok(None),
            (Some(leader_pass_id), Some(leader_btc_addr)) => {
                let msg = format!(
                    "Collab pass has multiple leader refs: inscription_id={}, leader_pass_id={}, leader_btc_addr={}",
                    collab_pass.inscription_id, leader_pass_id, leader_btc_addr
                );
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    /// Resolve a fixed `leader_pass_id` binding to an active standard pass snapshot at a BTC height.
    ///
    /// The referenced pass must exist at `block_height`, be `Active`, and have
    /// `Standard` pass kind. Other historical states resolve to `None`.
    pub fn resolve_leader_pass_id_at_height(
        &self,
        leader_pass_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<MinerPassSnapshotInfo>, String> {
        let Some(snapshot) = self
            .storage
            .get_pass_snapshot_from_history_at_height(leader_pass_id, block_height)?
        else {
            return Ok(None);
        };

        if snapshot.pass.pass_kind == MinerPassKind::Standard
            && snapshot.pass.state == MinerPassState::Active
        {
            Ok(Some(snapshot))
        } else {
            Ok(None)
        }
    }

    /// Resolve a `leader_btc_addr` binding through the current BTC network to an active standard pass snapshot.
    ///
    /// Address parsing intentionally uses `self.config.config().bitcoin.network()`
    /// so mainnet/testnet/regtest leader refs cannot silently cross networks.
    pub fn resolve_leader_btc_addr_at_height(
        &self,
        leader_btc_addr: &str,
        block_height: u32,
    ) -> Result<Option<MinerPassSnapshotInfo>, String> {
        let network = self.config.config().bitcoin.network();
        let owner = address_string_to_script_hash(leader_btc_addr, &network).map_err(|e| {
            let msg = format!(
                "Failed to resolve leader_btc_addr to script hash: leader_btc_addr={}, network={}, error={}",
                leader_btc_addr, network, e
            );
            error!("{}", msg);
            msg
        })?;

        self.storage
            .get_owner_active_standard_pass_from_history_at_height(&owner, block_height)
    }

    fn push_block_mutation(&self, mutation: PassBlockMutation) {
        if let Some(collector) = self.current_block_collector.lock().unwrap().as_mut() {
            collector.push(mutation);
            return;
        }

        warn!(
            "Pass block mutation collector is inactive; skipping mutation recording: mutation={:?}",
            mutation
        );
    }

    pub async fn on_mint_pass(&self, mint_info: &PassMintInscriptionInfo) -> Result<(), String> {
        if let Some(invalid) = self.validate_mint_state(mint_info)? {
            self.record_invalid_mint_from_mint_info(mint_info, invalid)
                .await?;
            return Ok(());
        }

        // First check if the owner already has an active pass
        self.dormant_last_pass(mint_info).await?;

        // Insert the new pass as active
        let leader_btc_owner = self.resolve_leader_btc_owner_for_mint(mint_info)?;
        let info = MinerPassInfo {
            inscription_id: mint_info.inscription_id,
            inscription_number: mint_info.inscription_number,
            mint_txid: mint_info.mint_txid,
            mint_block_height: mint_info.mint_block_height,
            mint_owner: mint_info.mint_owner,

            satpoint: mint_info.satpoint,

            mint_version: mint_info.mint_version,
            pass_kind: mint_info.pass_kind,
            usdb_main: mint_info.usdb_main.clone(),
            leader_pass_id: mint_info.leader_pass_id,
            leader_btc_addr: mint_info.leader_btc_addr.clone(),
            leader_btc_owner,
            prev: mint_info.prev.clone(),
            invalid_code: None,
            invalid_reason: None,

            state: MinerPassState::Active,
            owner: mint_info.mint_owner,
        };
        // Persist current snapshot and append pass history event at mint height.
        self.storage
            .add_new_mint_pass_at_height(&info, mint_info.mint_block_height)?;
        self.push_block_mutation(PassBlockMutation::Mint {
            inscription_id: info.inscription_id.to_string(),
            inscription_number: info.inscription_number,
            mint_owner: info.mint_owner.to_string(),
            satpoint: info.satpoint.to_string(),
            mint_version: info.mint_version,
            pass_kind: info.pass_kind.as_str().to_string(),
            usdb_main: info.usdb_main.clone(),
            leader_pass_id: info.leader_pass_id.as_ref().map(|v| v.to_string()),
            leader_btc_addr: info.leader_btc_addr.clone(),
            prev: info.prev.iter().map(|v| v.to_string()).collect(),
        });

        info!(
            "New Miner Pass {} minted at block height {} for owner {}",
            mint_info.inscription_id, mint_info.mint_block_height, mint_info.mint_owner
        );

        // State pre-validation above guarantees every prev is eligible before any mutation is written.
        let mut inherited_energy = 0u128;
        for prev_inscription_id in &mint_info.prev {
            let prev_pass = self
                .storage
                .get_pass_by_inscription_id(prev_inscription_id)?
                .ok_or_else(|| {
                    let msg = format!(
                        "Previous miner pass {} disappeared after validation for mint {}",
                        prev_inscription_id, mint_info.inscription_id
                    );
                    error!("{}", msg);
                    msg
                })?;
            if prev_pass.owner != mint_info.mint_owner || prev_pass.state != MinerPassState::Dormant
            {
                let msg = format!(
                    "Previous miner pass {} became ineligible after validation for mint {}: owner={}, state={}",
                    prev_inscription_id,
                    mint_info.inscription_id,
                    prev_pass.owner,
                    prev_pass.state.as_str()
                );
                error!("{}", msg);
                return Err(msg);
            }

            let energy = self
                .consume_pass(prev_inscription_id, mint_info.mint_block_height)
                .await?;
            inherited_energy = inherited_energy.saturating_add(energy);
        }

        // Update energy record for the new pass with inherited energy
        self.energy_manager
            .on_new_pass(
                &mint_info.inscription_id,
                &mint_info.mint_owner,
                mint_info.mint_block_height,
                inherited_energy,
            )
            .await?;

        Ok(())
    }

    fn validate_mint_state(
        &self,
        mint_info: &PassMintInscriptionInfo,
    ) -> Result<Option<MintStateValidationError>, String> {
        if let Some(invalid) = self.validate_leader_pass_binding(mint_info)? {
            return Ok(Some(invalid));
        }

        let old_active = self
            .storage
            .get_last_active_mint_pass_by_owner(&mint_info.mint_owner)?;
        let mut seen_prev = BTreeSet::new();

        for prev_inscription_id in &mint_info.prev {
            if !seen_prev.insert(prev_inscription_id.to_string()) {
                return Ok(Some(MintStateValidationError {
                    code: MintValidationErrorCode::InvalidPrevId,
                    reason: format!(
                        "Duplicate prev inscription id {} for mint {}",
                        prev_inscription_id, mint_info.inscription_id
                    ),
                }));
            }

            let Some(prev_pass) = self
                .storage
                .get_pass_by_inscription_id(prev_inscription_id)?
            else {
                return Ok(Some(MintStateValidationError {
                    code: MintValidationErrorCode::InvalidPrevId,
                    reason: format!(
                        "Previous miner pass {} not found for mint {}",
                        prev_inscription_id, mint_info.inscription_id
                    ),
                }));
            };

            if prev_pass.owner != mint_info.mint_owner {
                return Ok(Some(MintStateValidationError {
                    code: MintValidationErrorCode::InvalidPrevId,
                    reason: format!(
                        "Previous miner pass {} owner {} does not match mint {} owner {}",
                        prev_inscription_id,
                        prev_pass.owner,
                        mint_info.inscription_id,
                        mint_info.mint_owner
                    ),
                }));
            }

            let is_virtual_old_active = old_active
                .as_ref()
                .map(|pass| pass.inscription_id == *prev_inscription_id)
                .unwrap_or(false);
            let eligible_state = if is_virtual_old_active {
                prev_pass.state == MinerPassState::Active
            } else {
                prev_pass.state == MinerPassState::Dormant
            };

            if !eligible_state {
                return Ok(Some(MintStateValidationError {
                    code: MintValidationErrorCode::InvalidPrevId,
                    reason: format!(
                        "Previous miner pass {} is in state {}, expected Dormant{} for mint {}",
                        prev_inscription_id,
                        prev_pass.state.as_str(),
                        if old_active
                            .as_ref()
                            .map(|pass| pass.inscription_id == *prev_inscription_id)
                            .unwrap_or(false)
                        {
                            " or the same-owner active pass that this mint supersedes"
                        } else {
                            ""
                        },
                        mint_info.inscription_id
                    ),
                }));
            }
        }

        Ok(None)
    }

    fn validate_leader_pass_binding(
        &self,
        mint_info: &PassMintInscriptionInfo,
    ) -> Result<Option<MintStateValidationError>, String> {
        if mint_info.pass_kind != MinerPassKind::Collab {
            return Ok(None);
        }

        let Some(leader_pass_id) = mint_info.leader_pass_id.as_ref() else {
            return Ok(None);
        };

        if *leader_pass_id == mint_info.inscription_id {
            return Ok(Some(MintStateValidationError {
                code: MintValidationErrorCode::InvalidLeaderPassId,
                reason: format!(
                    "Collab mint {} cannot reference itself as leader_pass_id",
                    mint_info.inscription_id
                ),
            }));
        }

        let Some(leader_pass) = self.storage.get_pass_by_inscription_id(leader_pass_id)? else {
            return Ok(Some(MintStateValidationError {
                code: MintValidationErrorCode::InvalidLeaderPassId,
                reason: format!(
                    "Leader pass {} not found for collab mint {}",
                    leader_pass_id, mint_info.inscription_id
                ),
            }));
        };

        if leader_pass.pass_kind != MinerPassKind::Standard {
            return Ok(Some(MintStateValidationError {
                code: MintValidationErrorCode::InvalidLeaderPassId,
                reason: format!(
                    "Leader pass {} for collab mint {} must be standard, got {}",
                    leader_pass_id,
                    mint_info.inscription_id,
                    leader_pass.pass_kind.as_str()
                ),
            }));
        }

        if leader_pass.state != MinerPassState::Active {
            return Ok(Some(MintStateValidationError {
                code: MintValidationErrorCode::InvalidLeaderPassId,
                reason: format!(
                    "Leader pass {} for collab mint {} must be Active, got {}",
                    leader_pass_id,
                    mint_info.inscription_id,
                    leader_pass.state.as_str()
                ),
            }));
        }

        Ok(None)
    }

    async fn record_invalid_mint_from_mint_info(
        &self,
        mint_info: &PassMintInscriptionInfo,
        invalid: MintStateValidationError,
    ) -> Result<(), String> {
        warn!(
            "USDB mint failed state validation: inscription_id={}, block_height={}, owner={}, error_code={}, error_reason={}",
            mint_info.inscription_id,
            mint_info.mint_block_height,
            mint_info.mint_owner,
            invalid.code.as_str(),
            invalid.reason
        );

        let invalid_info = InvalidPassMintInscriptionInfo {
            inscription_id: mint_info.inscription_id,
            inscription_number: mint_info.inscription_number,
            mint_txid: mint_info.mint_txid,
            mint_block_height: mint_info.mint_block_height,
            mint_owner: mint_info.mint_owner,
            satpoint: mint_info.satpoint,
            error_code: invalid.code.as_str().to_string(),
            error_reason: invalid.reason,
        };

        self.on_invalid_mint_pass(&invalid_info).await
    }

    pub async fn on_invalid_mint_pass(
        &self,
        invalid_info: &InvalidPassMintInscriptionInfo,
    ) -> Result<(), String> {
        let info = MinerPassInfo {
            inscription_id: invalid_info.inscription_id,
            inscription_number: invalid_info.inscription_number,
            mint_txid: invalid_info.mint_txid,
            mint_block_height: invalid_info.mint_block_height,
            mint_owner: invalid_info.mint_owner,
            satpoint: invalid_info.satpoint,
            mint_version: 0,
            pass_kind: MinerPassKind::Standard,
            usdb_main: "".to_string(),
            leader_pass_id: None,
            leader_btc_addr: None,
            leader_btc_owner: None,
            prev: Vec::new(),
            invalid_code: Some(invalid_info.error_code.clone()),
            invalid_reason: Some(invalid_info.error_reason.clone()),
            owner: invalid_info.mint_owner,
            state: MinerPassState::Invalid,
        };
        // Invalid mint should also be visible in history timeline at inscription height.
        self.storage
            .add_invalid_mint_pass_at_height(&info, invalid_info.mint_block_height)?;
        self.push_block_mutation(PassBlockMutation::InvalidMint {
            inscription_id: info.inscription_id.to_string(),
            inscription_number: info.inscription_number,
            mint_owner: info.mint_owner.to_string(),
            satpoint: info.satpoint.to_string(),
            error_code: invalid_info.error_code.clone(),
            error_reason: invalid_info.error_reason.clone(),
        });

        warn!(
            "Invalid mint inscription recorded: module=pass_manager, inscription_id={}, block_height={}, owner={}, error_code={}, error_reason={}",
            invalid_info.inscription_id,
            invalid_info.mint_block_height,
            invalid_info.mint_owner,
            invalid_info.error_code,
            invalid_info.error_reason
        );
        Ok(())
    }

    // Dormant the last active pass for the same owner if exists, and update energy record for the dormant pass.
    // This is called when minting a new pass, to ensure there is only one active pass for each owner at any time.
    // The new minted pass will be active, and the existing active pass will be marked as dormant.
    // The dormant pass can still be consumed later to inherit energy to the new minted pass, but it cannot be consumed together with the new minted pass
    async fn dormant_last_pass(&self, mint_info: &PassMintInscriptionInfo) -> Result<(), String> {
        // First check the pass already exists on the same address
        let existing_pass = self
            .storage
            .get_last_active_mint_pass_by_owner(&mint_info.mint_owner)?;
        if let Some(last_pass) = existing_pass {
            assert!(
                last_pass.state == MinerPassState::Active,
                "Existing pass should be active {}",
                last_pass.inscription_id
            );
            assert!(
                last_pass.mint_block_height <= mint_info.mint_block_height,
                "Existing pass mint block height {} should be less than or equal to new mint pass block height {}",
                last_pass.mint_block_height,
                mint_info.mint_block_height
            );

            warn!(
                "Owner {} already has an active Miner Pass {} at block height {}, new mint pass {} at block height {} will be dormant",
                mint_info.mint_owner,
                last_pass.inscription_id,
                last_pass.mint_block_height,
                mint_info.inscription_id,
                mint_info.mint_block_height
            );

            // Settle active energy before the pass leaves Active. UIP-0002 requires
            // same-height balance penalty/growth to be materialized before the state effect.
            self.energy_manager
                .on_pass_dormant(&last_pass.inscription_id, mint_info.mint_block_height)
                .await?;

            // Mark the last pass as dormant.
            // Use height-aware state transition so historical active-set reconstruction stays deterministic.
            self.storage.update_state_at_height(
                &last_pass.inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                mint_info.mint_block_height,
            )?;
            self.push_block_mutation(PassBlockMutation::StateTransition {
                inscription_id: last_pass.inscription_id.to_string(),
                from_state: MinerPassState::Active.as_str().to_string(),
                to_state: MinerPassState::Dormant.as_str().to_string(),
                owner: last_pass.owner.to_string(),
                satpoint: last_pass.satpoint.to_string(),
            });

            info!(
                "Last Pass {} marked as Dormant due to new pass {} for owner {}",
                last_pass.inscription_id, mint_info.inscription_id, mint_info.mint_owner
            );
        }

        Ok(())
    }

    // Consume the pass at the given block height, returning its UIP-0003 inheritable raw energy.
    async fn consume_pass(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Energy, String> {
        // Mark the pass as consumed
        let pass = self
            .storage
            .get_pass_by_inscription_id(inscription_id)?
            .ok_or_else(|| {
                let msg = format!(
                    "Miner Pass {} not found for consuming at block height {}",
                    inscription_id, block_height
                );
                error!("{}", msg);
                msg
            })?;

        // The pass must be dormant before consuming
        assert_eq!(
            pass.state,
            MinerPassState::Dormant,
            "Miner Pass {} must be dormant before consuming, but state is {:?}",
            inscription_id,
            pass.state
        );

        self.storage.update_state_at_height(
            inscription_id,
            MinerPassState::Consumed,
            pass.state.clone(),
            block_height,
        )?;
        self.push_block_mutation(PassBlockMutation::StateTransition {
            inscription_id: inscription_id.to_string(),
            from_state: pass.state.as_str().to_string(),
            to_state: MinerPassState::Consumed.as_str().to_string(),
            owner: pass.owner.to_string(),
            satpoint: pass.satpoint.to_string(),
        });

        // Get the latest energy at block_height.
        // The pass may become dormant at an earlier height, so exact-height lookup is not reliable.
        let ret = self
            .energy_manager
            .get_pass_energy(inscription_id, block_height)
            .await?;
        if ret.is_none() {
            let msg = format!(
                "Miner Pass {} energy not found at block height {} for consuming",
                inscription_id, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }
        let energy = ret.unwrap();
        assert_eq!(
            energy.state,
            MinerPassState::Dormant,
            "Miner Pass {} energy state must be Dormant before consuming, but state is {:?}",
            inscription_id,
            energy.state
        );

        let inheritable_energy = calc_inheritable_energy(energy.energy);

        self.energy_manager
            .on_pass_consumed(inscription_id, &pass.owner, block_height)?;

        info!(
            "Miner Pass {} consumed at block height {}, raw energy {}, inheritable energy {}",
            inscription_id, block_height, energy.energy, inheritable_energy
        );

        Ok(inheritable_energy)
    }

    pub async fn on_pass_transfer(
        &self,
        inscription_id: &InscriptionId,
        new_owner: &BtcScriptHash,
        satpoint: &SatPoint,
        block_height: u32,
    ) -> Result<(), String> {
        info!(
            "Miner Pass {} transferred to new owner {} at block height {}, new satpoint {}",
            inscription_id, new_owner, block_height, satpoint
        );

        // First lookup the pass by inscription id
        let pass = self.storage.get_pass_by_inscription_id(inscription_id)?;
        let pass = pass.ok_or_else(|| {
            let msg = format!(
                "Miner Pass {} not found for transfer at block height {}",
                inscription_id, block_height
            );
            error!("{}", msg);
            msg
        })?;

        if matches!(
            pass.state,
            MinerPassState::Consumed | MinerPassState::Burned | MinerPassState::Invalid
        ) {
            warn!(
                "Terminal Miner Pass {} transferred at block height {}; keep consensus owner/satpoint unchanged: state={}, owner={}, satpoint={}",
                inscription_id,
                block_height,
                pass.state.as_str(),
                pass.owner,
                pass.satpoint
            );
            return Ok(());
        }

        // Update energy record for the pass before transfer if the pass is active
        if pass.state == MinerPassState::Active {
            self.energy_manager
                .update_pass_energy(inscription_id, block_height)
                .await?;
        }

        if pass.owner == *new_owner {
            warn!(
                "Miner Pass {} transferred to the same owner {}, skip updating owner",
                inscription_id, new_owner
            );
            self.storage.update_satpoint_at_height(
                inscription_id,
                &pass.satpoint,
                satpoint,
                block_height,
            )?;
            self.push_block_mutation(PassBlockMutation::SatpointUpdate {
                inscription_id: inscription_id.to_string(),
                state: pass.state.as_str().to_string(),
                owner: pass.owner.to_string(),
                from_satpoint: pass.satpoint.to_string(),
                to_satpoint: satpoint.to_string(),
            });
        } else {
            if pass.state == MinerPassState::Active {
                // Freeze active energy at transfer height and mark pass state as Dormant first.
                self.energy_manager
                    .on_pass_dormant(inscription_id, block_height)
                    .await?;
                self.storage.update_state_at_height(
                    inscription_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    block_height,
                )?;
                self.push_block_mutation(PassBlockMutation::StateTransition {
                    inscription_id: inscription_id.to_string(),
                    from_state: MinerPassState::Active.as_str().to_string(),
                    to_state: MinerPassState::Dormant.as_str().to_string(),
                    owner: pass.owner.to_string(),
                    satpoint: pass.satpoint.to_string(),
                });
            }

            // Transfer the ownership in storage
            self.storage.transfer_owner_at_height(
                inscription_id,
                new_owner,
                satpoint,
                block_height,
            )?;
            self.push_block_mutation(PassBlockMutation::OwnerTransfer {
                inscription_id: inscription_id.to_string(),
                state: if pass.state == MinerPassState::Active {
                    MinerPassState::Dormant.as_str().to_string()
                } else {
                    pass.state.as_str().to_string()
                },
                from_owner: pass.owner.to_string(),
                to_owner: new_owner.to_string(),
                from_satpoint: pass.satpoint.to_string(),
                to_satpoint: satpoint.to_string(),
            });
        }

        Ok(())
    }

    pub async fn on_pass_burned(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<(), String> {
        info!(
            "Miner Pass {} burned at block height {}",
            inscription_id, block_height
        );

        // First lookup the pass by inscription id
        let pass = self.storage.get_pass_by_inscription_id(inscription_id)?;
        let pass = pass.ok_or_else(|| {
            let msg = format!(
                "Miner Pass {} not found for burning at block height {}",
                inscription_id, block_height
            );
            error!("{}", msg);
            msg
        })?;

        match pass.state {
            MinerPassState::Active | MinerPassState::Dormant => {}
            MinerPassState::Consumed => {
                warn!(
                    "Miner Pass {} was already Consumed before burn at block height {}; keep consumed economic state",
                    inscription_id, block_height
                );
                return Ok(());
            }
            MinerPassState::Burned => {
                warn!(
                    "Miner Pass {} was already Burned before duplicate burn at block height {}; skip",
                    inscription_id, block_height
                );
                return Ok(());
            }
            MinerPassState::Invalid => {
                warn!(
                    "Invalid Miner Pass {} burned at block height {}; no economic state transition",
                    inscription_id, block_height
                );
                return Ok(());
            }
        }

        self.energy_manager
            .on_pass_burned(
                inscription_id,
                &pass.owner,
                pass.state.clone(),
                block_height,
            )
            .await?;

        self.storage.update_state_at_height(
            inscription_id,
            MinerPassState::Burned,
            pass.state.clone(),
            block_height,
        )?;
        self.push_block_mutation(PassBlockMutation::StateTransition {
            inscription_id: inscription_id.to_string(),
            from_state: pass.state.as_str().to_string(),
            to_state: MinerPassState::Burned.as_str().to_string(),
            owner: pass.owner.to_string(),
            satpoint: pass.satpoint.to_string(),
        });

        Ok(())
    }
}

pub type MinerPassManagerRef = Arc<MinerPassManager>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigManager;
    use crate::index::energy::{BalanceProvider, PassEnergyManager};
    use crate::index::energy_formula::calc_collab_contribution;
    use crate::storage::{MinerPassStorage, PassEnergyRecord, PassEnergyStorage};
    use balance_history::AddressBalance;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::secp256k1::{Secp256k1, SecretKey};
    use bitcoincore_rpc::bitcoin::{Address, Network, OutPoint, PublicKey, ScriptBuf, Txid};
    use std::path::PathBuf;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::ToBtcScriptHash;

    #[derive(Default)]
    struct NoopBalanceProvider;

    impl BalanceProvider for NoopBalanceProvider {
        fn get_balance_at_height<'a>(
            &'a self,
            _address: BtcScriptHash,
            _block_height: u32,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>,
        > {
            Box::pin(async move { Ok(Vec::new()) })
        }

        fn get_balance_at_range<'a>(
            &'a self,
            _address: BtcScriptHash,
            _block_range: std::ops::Range<u32>,
        ) -> Pin<
            Box<dyn std::future::Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>,
        > {
            Box::pin(async move { Ok(Vec::new()) })
        }
    }

    fn test_root_dir(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("usdb_indexer_pass_test_{}_{}", test_name, nanos))
    }

    fn test_script_hash(tag: u8) -> BtcScriptHash {
        let script = ScriptBuf::from(vec![tag; 32]);
        script.to_btc_script_hash()
    }

    fn test_inscription_id(tag: u8, index: u32) -> InscriptionId {
        let txid = Txid::from_slice(&[tag; 32]).unwrap();
        InscriptionId { txid, index }
    }

    fn test_satpoint(tag: u8, vout: u32, offset: u64) -> SatPoint {
        SatPoint {
            outpoint: OutPoint {
                txid: Txid::from_slice(&[tag; 32]).unwrap(),
                vout,
            },
            offset,
        }
    }

    fn setup_empty_manager(test_name: &str) -> (PathBuf, MinerPassStorageRef, MinerPassManager) {
        let root_dir = test_root_dir(test_name);
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let storage = Arc::new(MinerPassStorage::new(&config.data_dir()).unwrap());
        let energy_storage = PassEnergyStorage::new(&config.data_dir()).unwrap();
        let energy_manager = Arc::new(PassEnergyManager::new_with_deps(
            config.clone(),
            energy_storage,
            Arc::new(NoopBalanceProvider),
        ));
        let manager = MinerPassManager::new(config, storage.clone(), energy_manager).unwrap();

        (root_dir, storage, manager)
    }

    fn test_mint_info(
        inscription_id: InscriptionId,
        owner: BtcScriptHash,
        block_height: u32,
        prev: Vec<InscriptionId>,
    ) -> PassMintInscriptionInfo {
        PassMintInscriptionInfo {
            inscription_number: inscription_id.index as i32,
            mint_txid: inscription_id.txid,
            mint_block_height: block_height,
            mint_owner: owner,
            satpoint: test_satpoint(21, 0, 0),
            mint_version: 1,
            pass_kind: MinerPassKind::Standard,
            usdb_main: "0x1111111111111111111111111111111111111111".to_string(),
            leader_pass_id: None,
            leader_btc_addr: None,
            prev,
            inscription_id,
        }
    }

    fn test_collab_mint_info(
        inscription_id: InscriptionId,
        owner: BtcScriptHash,
        block_height: u32,
        leader_pass_id: InscriptionId,
    ) -> PassMintInscriptionInfo {
        PassMintInscriptionInfo {
            inscription_number: inscription_id.index as i32,
            mint_txid: inscription_id.txid,
            mint_block_height: block_height,
            mint_owner: owner,
            satpoint: test_satpoint(22, 0, 0),
            mint_version: 1,
            pass_kind: MinerPassKind::Collab,
            usdb_main: String::new(),
            leader_pass_id: Some(leader_pass_id),
            leader_btc_addr: None,
            prev: Vec::new(),
            inscription_id,
        }
    }

    fn test_pass_info(
        inscription_id: InscriptionId,
        owner: BtcScriptHash,
        block_height: u32,
        pass_kind: MinerPassKind,
        state: MinerPassState,
    ) -> MinerPassInfo {
        MinerPassInfo {
            inscription_id,
            inscription_number: inscription_id.index as i32,
            mint_txid: inscription_id.txid,
            mint_block_height: block_height,
            mint_owner: owner,
            satpoint: test_satpoint(23, 0, 0),
            mint_version: 1,
            pass_kind,
            usdb_main: if pass_kind == MinerPassKind::Standard {
                "0x1111111111111111111111111111111111111111".to_string()
            } else {
                String::new()
            },
            leader_pass_id: None,
            leader_btc_addr: None,
            leader_btc_owner: None,
            prev: Vec::new(),
            invalid_code: None,
            invalid_reason: None,
            owner,
            state,
        }
    }

    fn test_collab_pass_info_with_leader_pass(
        inscription_id: InscriptionId,
        owner: BtcScriptHash,
        block_height: u32,
        leader_pass_id: InscriptionId,
    ) -> MinerPassInfo {
        let mut pass = test_pass_info(
            inscription_id,
            owner,
            block_height,
            MinerPassKind::Collab,
            MinerPassState::Active,
        );
        pass.leader_pass_id = Some(leader_pass_id);
        pass
    }

    fn test_collab_pass_info_with_leader_addr(
        inscription_id: InscriptionId,
        owner: BtcScriptHash,
        block_height: u32,
        leader_btc_addr: &str,
    ) -> MinerPassInfo {
        let mut pass = test_pass_info(
            inscription_id,
            owner,
            block_height,
            MinerPassKind::Collab,
            MinerPassState::Active,
        );
        pass.leader_btc_addr = Some(leader_btc_addr.to_string());
        pass.leader_btc_owner =
            Some(address_string_to_script_hash(leader_btc_addr, &Network::Bitcoin).unwrap());
        pass
    }

    fn setup_manager(
        test_name: &str,
    ) -> (
        PathBuf,
        MinerPassStorageRef,
        MinerPassManager,
        InscriptionId,
        BtcScriptHash,
        SatPoint,
    ) {
        let root_dir = test_root_dir(test_name);
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let storage = Arc::new(MinerPassStorage::new(&config.data_dir()).unwrap());
        let energy_storage = PassEnergyStorage::new(&config.data_dir()).unwrap();
        let energy_manager = Arc::new(PassEnergyManager::new_with_deps(
            config.clone(),
            energy_storage,
            Arc::new(NoopBalanceProvider),
        ));
        let manager =
            MinerPassManager::new(config, storage.clone(), energy_manager.clone()).unwrap();

        let inscription_id = test_inscription_id(1, 0);
        let owner = test_script_hash(7);
        let satpoint = test_satpoint(2, 0, 0);

        let pass = MinerPassInfo {
            inscription_id,
            inscription_number: 1,
            mint_txid: Txid::from_slice(&[3; 32]).unwrap(),
            mint_block_height: 100,
            mint_owner: owner,
            satpoint,
            mint_version: 1,
            pass_kind: MinerPassKind::Standard,
            usdb_main: "0x1111111111111111111111111111111111111111".to_string(),
            leader_pass_id: None,
            leader_btc_addr: None,
            leader_btc_owner: None,
            prev: Vec::new(),
            invalid_code: None,
            invalid_reason: None,
            owner,
            state: MinerPassState::Active,
        };
        storage.add_new_mint_pass(&pass).unwrap();
        storage
            .update_state(
                &inscription_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
            )
            .unwrap();
        energy_manager
            .insert_pass_energy_record_for_test(&PassEnergyRecord {
                inscription_id,
                block_height: 100,
                state: MinerPassState::Dormant,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 100_000,
                owner_delta: 0,
                energy: 42,
            })
            .unwrap();

        (root_dir, storage, manager, inscription_id, owner, satpoint)
    }

    #[tokio::test]
    async fn test_on_mint_pass_missing_prev_records_invalid() {
        let (root_dir, storage, manager) = setup_empty_manager("mint_missing_prev_invalid");
        let owner = test_script_hash(31);
        let mint_id = test_inscription_id(32, 0);
        let missing_prev = test_inscription_id(33, 0);
        let mint_info = test_mint_info(mint_id, owner, 101, vec![missing_prev]);

        manager.on_mint_pass(&mint_info).await.unwrap();

        let stored = storage
            .get_pass_by_inscription_id(&mint_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MinerPassState::Invalid);
        assert_eq!(stored.owner, owner);
        assert_eq!(
            stored.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidPrevId.as_str())
        );
        assert!(
            stored
                .invalid_reason
                .as_deref()
                .unwrap_or_default()
                .contains("not found")
        );
        assert!(
            storage
                .get_all_active_pass_by_page(0, 10)
                .unwrap()
                .is_empty()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_invalid_prev_keeps_old_active() {
        let (root_dir, storage, manager) = setup_empty_manager("mint_invalid_prev_keeps_old");
        let owner = test_script_hash(34);
        let old_id = test_inscription_id(35, 0);
        let new_id = test_inscription_id(36, 0);
        let missing_prev = test_inscription_id(37, 0);
        let old_pass = MinerPassInfo {
            inscription_id: old_id,
            inscription_number: 1,
            mint_txid: old_id.txid,
            mint_block_height: 100,
            mint_owner: owner,
            satpoint: test_satpoint(35, 0, 0),
            mint_version: 1,
            pass_kind: MinerPassKind::Standard,
            usdb_main: "0x1111111111111111111111111111111111111111".to_string(),
            leader_pass_id: None,
            leader_btc_addr: None,
            leader_btc_owner: None,
            prev: Vec::new(),
            invalid_code: None,
            invalid_reason: None,
            owner,
            state: MinerPassState::Active,
        };
        storage
            .add_new_mint_pass_at_height(&old_pass, old_pass.mint_block_height)
            .unwrap();

        let mint_info = test_mint_info(new_id, owner, 101, vec![old_id, missing_prev]);
        manager.on_mint_pass(&mint_info).await.unwrap();

        let old_after = storage
            .get_pass_by_inscription_id(&old_id)
            .unwrap()
            .unwrap();
        assert_eq!(old_after.state, MinerPassState::Active);

        let new_after = storage
            .get_pass_by_inscription_id(&new_id)
            .unwrap()
            .unwrap();
        assert_eq!(new_after.state, MinerPassState::Invalid);
        assert_eq!(
            new_after.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidPrevId.as_str())
        );

        let active = storage.get_all_active_pass_by_page(0, 10).unwrap();
        assert_eq!(active.len(), 1);
        assert_eq!(active[0].inscription_id, old_id);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_prev_owner_mismatch_records_invalid() {
        let (root_dir, storage, manager) = setup_empty_manager("mint_prev_owner_mismatch");
        let prev_owner = test_script_hash(38);
        let mint_owner = test_script_hash(39);
        let prev_id = test_inscription_id(38, 0);
        let mint_id = test_inscription_id(39, 0);
        let prev_pass = test_pass_info(
            prev_id,
            prev_owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&prev_pass, prev_pass.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &prev_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                101,
            )
            .unwrap();

        let mint_info = test_mint_info(mint_id, mint_owner, 102, vec![prev_id]);
        manager.on_mint_pass(&mint_info).await.unwrap();

        let prev_after = storage
            .get_pass_by_inscription_id(&prev_id)
            .unwrap()
            .unwrap();
        assert_eq!(prev_after.owner, prev_owner);
        assert_eq!(prev_after.state, MinerPassState::Dormant);

        let mint_after = storage
            .get_pass_by_inscription_id(&mint_id)
            .unwrap()
            .unwrap();
        assert_eq!(mint_after.owner, mint_owner);
        assert_eq!(mint_after.state, MinerPassState::Invalid);
        assert_eq!(
            mint_after.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidPrevId.as_str())
        );
        assert!(
            mint_after
                .invalid_reason
                .as_deref()
                .unwrap_or_default()
                .contains("does not match")
        );
        assert!(
            storage
                .get_all_active_pass_by_page(0, 10)
                .unwrap()
                .is_empty()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_duplicate_prev_records_invalid_without_consuming() {
        let (root_dir, storage, manager) = setup_empty_manager("mint_duplicate_prev_invalid");
        let owner = test_script_hash(40);
        let prev_id = test_inscription_id(40, 0);
        let mint_id = test_inscription_id(41, 0);
        let prev_pass = test_pass_info(
            prev_id,
            owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&prev_pass, prev_pass.mint_block_height)
            .unwrap();
        storage
            .update_state_at_height(
                &prev_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                101,
            )
            .unwrap();

        let mint_info = test_mint_info(mint_id, owner, 102, vec![prev_id, prev_id]);
        manager.on_mint_pass(&mint_info).await.unwrap();

        let prev_after = storage
            .get_pass_by_inscription_id(&prev_id)
            .unwrap()
            .unwrap();
        assert_eq!(prev_after.state, MinerPassState::Dormant);

        let mint_after = storage
            .get_pass_by_inscription_id(&mint_id)
            .unwrap()
            .unwrap();
        assert_eq!(mint_after.state, MinerPassState::Invalid);
        assert_eq!(
            mint_after.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidPrevId.as_str())
        );
        assert!(
            mint_after
                .invalid_reason
                .as_deref()
                .unwrap_or_default()
                .contains("Duplicate prev")
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_multi_prev_inherited_energy_saturates() {
        let (root_dir, storage, manager) = setup_empty_manager("mint_multi_prev_saturates");
        let owner = test_script_hash(74);
        let prev_1 = test_inscription_id(75, 0);
        let prev_2 = test_inscription_id(76, 0);
        let new_id = test_inscription_id(77, 0);

        for (prev_id, tag) in [(prev_1, 75u128), (prev_2, 76u128)] {
            let prev_pass = test_pass_info(
                prev_id,
                owner,
                100,
                MinerPassKind::Standard,
                MinerPassState::Active,
            );
            storage
                .add_new_mint_pass_at_height(&prev_pass, prev_pass.mint_block_height)
                .unwrap();
            storage
                .update_state_at_height(
                    &prev_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    100,
                )
                .unwrap();
            manager
                .energy_manager
                .insert_pass_energy_record_for_test(&PassEnergyRecord {
                    inscription_id: prev_id,
                    block_height: 100,
                    state: MinerPassState::Dormant,
                    active_block_height: 100,
                    owner_address: owner,
                    owner_balance: 0,
                    owner_delta: 0,
                    energy: Energy::MAX - tag,
                })
                .unwrap();
        }

        let mint_info = test_mint_info(new_id, owner, 101, vec![prev_1, prev_2]);
        manager.on_mint_pass(&mint_info).await.unwrap();

        let new_energy = manager
            .energy_manager
            .get_pass_energy(&new_id, 101)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(new_energy.state, MinerPassState::Active);
        assert_eq!(new_energy.energy, Energy::MAX);

        for prev_id in [prev_1, prev_2] {
            let consumed = manager
                .energy_manager
                .get_pass_energy(&prev_id, 101)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(consumed.state, MinerPassState::Consumed);
            assert_eq!(consumed.energy, 0);
        }

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_collab_prev_remint_inherits_only_raw_energy() {
        let (root_dir, storage, manager) = setup_empty_manager("collab_prev_remint_raw_only");
        let leader_owner = test_script_hash(78);
        let leader_id = test_inscription_id(78, 0);
        let leader_pass = test_pass_info(
            leader_id,
            leader_owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&leader_pass, leader_pass.mint_block_height)
            .unwrap();

        let prev_raw_energy = 1_000u128;
        let expected_inherited = calc_inheritable_energy(prev_raw_energy);
        let forbidden_effective_like_inherited =
            calc_inheritable_energy(prev_raw_energy + calc_collab_contribution(prev_raw_energy));
        assert!(
            forbidden_effective_like_inherited > expected_inherited,
            "fixture must distinguish raw-only inheritance from derived effective-energy inheritance"
        );

        for (old_tag, new_tag, owner_tag, new_kind) in [
            (79, 80, 79, MinerPassKind::Standard),
            (81, 82, 81, MinerPassKind::Collab),
        ] {
            let owner = test_script_hash(owner_tag);
            let old_collab_id = test_inscription_id(old_tag, 0);
            let old_collab =
                test_collab_pass_info_with_leader_pass(old_collab_id, owner, 101, leader_id);
            storage
                .add_new_mint_pass_at_height(&old_collab, old_collab.mint_block_height)
                .unwrap();
            storage
                .update_state_at_height(
                    &old_collab_id,
                    MinerPassState::Dormant,
                    MinerPassState::Active,
                    120,
                )
                .unwrap();
            manager
                .energy_manager
                .insert_pass_energy_record_for_test(&PassEnergyRecord {
                    inscription_id: old_collab_id,
                    block_height: 120,
                    state: MinerPassState::Dormant,
                    active_block_height: 120,
                    owner_address: owner,
                    owner_balance: 100_000,
                    owner_delta: 0,
                    energy: prev_raw_energy,
                })
                .unwrap();

            let new_id = test_inscription_id(new_tag, 0);
            let mut mint_info = test_mint_info(new_id, owner, 121, vec![old_collab_id]);
            if new_kind == MinerPassKind::Collab {
                mint_info.pass_kind = MinerPassKind::Collab;
                mint_info.usdb_main = String::new();
                mint_info.leader_pass_id = Some(leader_id);
            }

            manager.on_mint_pass(&mint_info).await.unwrap();

            let old_after = storage
                .get_pass_by_inscription_id(&old_collab_id)
                .unwrap()
                .unwrap();
            assert_eq!(old_after.state, MinerPassState::Consumed);

            let old_energy = manager
                .energy_manager
                .get_pass_energy(&old_collab_id, 121)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(old_energy.state, MinerPassState::Consumed);
            assert_eq!(old_energy.energy, 0);

            let new_pass = storage
                .get_pass_by_inscription_id(&new_id)
                .unwrap()
                .unwrap();
            assert_eq!(new_pass.state, MinerPassState::Active);
            assert_eq!(new_pass.pass_kind, new_kind);

            let new_energy = manager
                .energy_manager
                .get_pass_energy(&new_id, 121)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(new_energy.state, MinerPassState::Active);
            assert_eq!(new_energy.energy, expected_inherited);
            assert_ne!(new_energy.energy, forbidden_effective_like_inherited);
        }

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_collab_accepts_active_standard_leader_pass() {
        let (root_dir, storage, manager) =
            setup_empty_manager("collab_active_standard_leader_valid");
        let leader_owner = test_script_hash(40);
        let collab_owner = test_script_hash(41);
        let leader_id = test_inscription_id(42, 0);
        let collab_id = test_inscription_id(43, 0);
        let leader_pass = test_pass_info(
            leader_id,
            leader_owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&leader_pass, leader_pass.mint_block_height)
            .unwrap();

        let mint_info = test_collab_mint_info(collab_id, collab_owner, 101, leader_id);
        manager.on_mint_pass(&mint_info).await.unwrap();

        let stored = storage
            .get_pass_by_inscription_id(&collab_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MinerPassState::Active);
        assert_eq!(stored.pass_kind, MinerPassKind::Collab);
        assert_eq!(stored.leader_pass_id, Some(leader_id));

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_collab_leader_btc_addr_persists_owner() {
        let (root_dir, storage, manager) = setup_empty_manager("collab_leader_btc_addr_owner");
        let leader_btc_addr = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let leader_owner = address_string_to_script_hash(
            leader_btc_addr,
            &manager.config.config().bitcoin.network(),
        )
        .unwrap();
        let collab_owner = test_script_hash(44);
        let collab_id = test_inscription_id(45, 0);
        let mut mint_info = test_mint_info(collab_id, collab_owner, 101, Vec::new());
        mint_info.pass_kind = MinerPassKind::Collab;
        mint_info.usdb_main = String::new();
        mint_info.leader_btc_addr = Some(leader_btc_addr.to_string());

        manager.on_mint_pass(&mint_info).await.unwrap();

        let stored = storage
            .get_pass_by_inscription_id(&collab_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MinerPassState::Active);
        assert_eq!(stored.pass_kind, MinerPassKind::Collab);
        assert_eq!(stored.leader_pass_id, None);
        assert_eq!(stored.leader_btc_addr.as_deref(), Some(leader_btc_addr));
        assert_eq!(stored.leader_btc_owner, Some(leader_owner));

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_resolve_leader_pass_id_requires_active_standard_snapshot() {
        let (root_dir, storage, manager) = setup_empty_manager("resolve_leader_pass_id");
        let leader_owner = test_script_hash(61);
        let collab_owner = test_script_hash(62);
        let leader_id = test_inscription_id(63, 0);
        let collab_id = test_inscription_id(64, 0);
        let leader_pass = test_pass_info(
            leader_id,
            leader_owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&leader_pass, 100)
            .unwrap();

        let collab_pass =
            test_collab_pass_info_with_leader_pass(collab_id, collab_owner, 101, leader_id);
        let resolved = manager
            .resolve_collab_leader_at_height(&collab_pass, 101)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.leader_ref_kind, CollabLeaderRefKind::LeaderPassId);
        assert_eq!(resolved.leader_ref_value, leader_id.to_string());
        assert_eq!(resolved.leader.pass.inscription_id, leader_id);
        assert_eq!(resolved.leader.pass.pass_kind, MinerPassKind::Standard);
        assert_eq!(resolved.leader.pass.state, MinerPassState::Active);

        storage
            .update_state_at_height(
                &leader_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                110,
            )
            .unwrap();
        assert!(
            manager
                .resolve_collab_leader_at_height(&collab_pass, 110)
                .unwrap()
                .is_none()
        );

        let collab_leader_id = test_inscription_id(65, 0);
        let collab_leader = test_pass_info(
            collab_leader_id,
            test_script_hash(66),
            100,
            MinerPassKind::Collab,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&collab_leader, 100)
            .unwrap();
        assert!(
            manager
                .resolve_leader_pass_id_at_height(&collab_leader_id, 100)
                .unwrap()
                .is_none()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_resolve_leader_btc_addr_follows_active_standard_remint() {
        let (root_dir, storage, manager) = setup_empty_manager("resolve_leader_btc_addr");
        let leader_btc_addr = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh";
        let leader_owner = address_string_to_script_hash(
            leader_btc_addr,
            &manager.config.config().bitcoin.network(),
        )
        .unwrap();
        let collab_owner = test_script_hash(67);
        let leader_1_id = test_inscription_id(68, 0);
        let leader_2_id = test_inscription_id(69, 0);
        let collab_id = test_inscription_id(70, 0);

        let leader_1 = test_pass_info(
            leader_1_id,
            leader_owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage.add_new_mint_pass_at_height(&leader_1, 100).unwrap();
        let collab_pass =
            test_collab_pass_info_with_leader_addr(collab_id, collab_owner, 101, leader_btc_addr);

        assert!(
            manager
                .resolve_collab_leader_at_height(&collab_pass, 99)
                .unwrap()
                .is_none()
        );
        let resolved_110 = manager
            .resolve_collab_leader_at_height(&collab_pass, 110)
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved_110.leader_ref_kind,
            CollabLeaderRefKind::LeaderBtcAddr
        );
        assert_eq!(resolved_110.leader_ref_value, leader_btc_addr);
        assert_eq!(resolved_110.leader.pass.inscription_id, leader_1_id);

        storage
            .update_state_at_height(
                &leader_1_id,
                MinerPassState::Dormant,
                MinerPassState::Active,
                120,
            )
            .unwrap();
        let leader_2 = test_pass_info(
            leader_2_id,
            leader_owner,
            120,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage.add_new_mint_pass_at_height(&leader_2, 120).unwrap();

        let resolved_120 = manager
            .resolve_collab_leader_at_height(&collab_pass, 120)
            .unwrap()
            .unwrap();
        assert_eq!(resolved_120.leader.pass.inscription_id, leader_2_id);
        assert_eq!(resolved_120.leader.pass.owner, leader_owner);
        assert_eq!(resolved_120.leader.pass.state, MinerPassState::Active);
        assert_eq!(resolved_120.leader.pass.pass_kind, MinerPassKind::Standard);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_resolve_leader_btc_addr_rejects_wrong_network() {
        let (root_dir, _storage, manager) =
            setup_empty_manager("resolve_leader_btc_addr_wrong_network");
        let secp = Secp256k1::new();
        let secret = SecretKey::from_slice(&[71; 32]).unwrap();
        let public_key = PublicKey::new(
            bitcoincore_rpc::bitcoin::secp256k1::PublicKey::from_secret_key(&secp, &secret),
        );
        let testnet_leader_btc_addr = Address::p2pkh(public_key, Network::Testnet).to_string();

        let err = match manager.resolve_leader_btc_addr_at_height(&testnet_leader_btc_addr, 100) {
            Ok(resolved) => panic!(
                "Expected wrong-network leader_btc_addr to fail, resolved={}",
                resolved.is_some()
            ),
            Err(err) => err,
        };
        assert!(err.contains("Address network mismatch"), "{err}");

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_collab_missing_leader_records_invalid() {
        let (root_dir, storage, manager) = setup_empty_manager("collab_missing_leader_invalid");
        let collab_owner = test_script_hash(44);
        let missing_leader_id = test_inscription_id(45, 0);
        let collab_id = test_inscription_id(46, 0);
        let mint_info = test_collab_mint_info(collab_id, collab_owner, 101, missing_leader_id);

        manager.on_mint_pass(&mint_info).await.unwrap();

        let stored = storage
            .get_pass_by_inscription_id(&collab_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MinerPassState::Invalid);
        assert_eq!(
            stored.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidLeaderPassId.as_str())
        );
        assert!(
            stored
                .invalid_reason
                .as_deref()
                .unwrap_or_default()
                .contains("not found")
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_collab_dormant_leader_records_invalid() {
        let (root_dir, storage, manager) = setup_empty_manager("collab_dormant_leader_invalid");
        let leader_owner = test_script_hash(47);
        let collab_owner = test_script_hash(48);
        let leader_id = test_inscription_id(49, 0);
        let collab_id = test_inscription_id(50, 0);
        let leader_pass = test_pass_info(
            leader_id,
            leader_owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&leader_pass, leader_pass.mint_block_height)
            .unwrap();
        storage
            .update_state(&leader_id, MinerPassState::Dormant, MinerPassState::Active)
            .unwrap();

        let mint_info = test_collab_mint_info(collab_id, collab_owner, 101, leader_id);
        manager.on_mint_pass(&mint_info).await.unwrap();

        let stored = storage
            .get_pass_by_inscription_id(&collab_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MinerPassState::Invalid);
        assert_eq!(
            stored.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidLeaderPassId.as_str())
        );
        assert!(
            stored
                .invalid_reason
                .as_deref()
                .unwrap_or_default()
                .contains("must be Active")
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_collab_non_standard_leader_records_invalid() {
        let (root_dir, storage, manager) =
            setup_empty_manager("collab_non_standard_leader_invalid");
        let leader_owner = test_script_hash(51);
        let collab_owner = test_script_hash(52);
        let leader_id = test_inscription_id(53, 0);
        let collab_id = test_inscription_id(54, 0);
        let leader_pass = test_pass_info(
            leader_id,
            leader_owner,
            100,
            MinerPassKind::Collab,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&leader_pass, leader_pass.mint_block_height)
            .unwrap();

        let mint_info = test_collab_mint_info(collab_id, collab_owner, 101, leader_id);
        manager.on_mint_pass(&mint_info).await.unwrap();

        let stored = storage
            .get_pass_by_inscription_id(&collab_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MinerPassState::Invalid);
        assert_eq!(
            stored.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidLeaderPassId.as_str())
        );
        assert!(
            stored
                .invalid_reason
                .as_deref()
                .unwrap_or_default()
                .contains("must be standard")
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_mint_pass_collab_self_leader_records_invalid() {
        let (root_dir, storage, manager) = setup_empty_manager("collab_self_leader_invalid");
        let collab_owner = test_script_hash(55);
        let collab_id = test_inscription_id(56, 0);
        let mint_info = test_collab_mint_info(collab_id, collab_owner, 101, collab_id);

        manager.on_mint_pass(&mint_info).await.unwrap();

        let stored = storage
            .get_pass_by_inscription_id(&collab_id)
            .unwrap()
            .unwrap();
        assert_eq!(stored.state, MinerPassState::Invalid);
        assert_eq!(
            stored.invalid_code.as_deref(),
            Some(MintValidationErrorCode::InvalidLeaderPassId.as_str())
        );
        assert!(
            stored
                .invalid_reason
                .as_deref()
                .unwrap_or_default()
                .contains("cannot reference itself")
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_transfer_same_owner_updates_satpoint() {
        let (root_dir, storage, manager, inscription_id, owner, old_satpoint) =
            setup_manager("transfer_same_owner");

        let new_satpoint = test_satpoint(9, 1, 42);
        manager
            .on_pass_transfer(&inscription_id, &owner, &new_satpoint, 101)
            .await
            .unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.owner, owner);
        assert_eq!(updated.state, MinerPassState::Dormant);
        assert_eq!(updated.satpoint, new_satpoint);
        assert_ne!(updated.satpoint, old_satpoint);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_transfer_consumed_keeps_consensus_owner_and_satpoint() {
        let (root_dir, storage, manager, inscription_id, owner, old_satpoint) =
            setup_manager("transfer_consumed_noop");
        storage
            .update_state(
                &inscription_id,
                MinerPassState::Consumed,
                MinerPassState::Dormant,
            )
            .unwrap();

        let new_owner = test_script_hash(70);
        let new_satpoint = test_satpoint(70, 1, 42);
        manager
            .on_pass_transfer(&inscription_id, &new_owner, &new_satpoint, 101)
            .await
            .unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Consumed);
        assert_eq!(updated.owner, owner);
        assert_eq!(updated.satpoint, old_satpoint);
        assert!(
            storage
                .get_pass_history_by_page_in_height_range(&inscription_id, 101, 101, 0, 10, false)
                .unwrap()
                .is_empty()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_transfer_burned_keeps_consensus_owner_and_satpoint() {
        let (root_dir, storage, manager, inscription_id, owner, old_satpoint) =
            setup_manager("transfer_burned_noop");
        storage
            .update_state(
                &inscription_id,
                MinerPassState::Burned,
                MinerPassState::Dormant,
            )
            .unwrap();

        let new_owner = test_script_hash(71);
        let new_satpoint = test_satpoint(71, 1, 42);
        manager
            .on_pass_transfer(&inscription_id, &new_owner, &new_satpoint, 101)
            .await
            .unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Burned);
        assert_eq!(updated.owner, owner);
        assert_eq!(updated.satpoint, old_satpoint);
        assert!(
            storage
                .get_pass_history_by_page_in_height_range(&inscription_id, 101, 101, 0, 10, false)
                .unwrap()
                .is_empty()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_transfer_invalid_keeps_consensus_owner_and_satpoint() {
        let (root_dir, storage, manager, inscription_id, owner, old_satpoint) =
            setup_manager("transfer_invalid_noop");
        storage
            .update_state(
                &inscription_id,
                MinerPassState::Invalid,
                MinerPassState::Dormant,
            )
            .unwrap();

        let new_owner = test_script_hash(72);
        let new_satpoint = test_satpoint(72, 1, 42);
        manager
            .on_pass_transfer(&inscription_id, &new_owner, &new_satpoint, 101)
            .await
            .unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Invalid);
        assert_eq!(updated.owner, owner);
        assert_eq!(updated.satpoint, old_satpoint);
        assert!(
            storage
                .get_pass_history_by_page_in_height_range(&inscription_id, 101, 101, 0, 10, false)
                .unwrap()
                .is_empty()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_burned_from_dormant_updates_state() {
        let (root_dir, storage, manager, inscription_id, _owner, _satpoint) =
            setup_manager("burn_dormant");

        manager.on_pass_burned(&inscription_id, 101).await.unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Burned);

        let energy_101 = manager
            .energy_manager
            .get_pass_energy(&inscription_id, 101)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(energy_101.state, MinerPassState::Burned);
        assert_eq!(energy_101.energy, 0);

        let energy_102 = manager
            .energy_manager
            .get_pass_energy(&inscription_id, 102)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(energy_102.state, MinerPassState::Burned);
        assert_eq!(energy_102.energy, 0);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_burned_rejects_pass_energy_state_mismatch() {
        let (root_dir, storage, manager, inscription_id, owner, _satpoint) =
            setup_manager("burn_energy_state_mismatch");

        manager
            .energy_manager
            .insert_pass_energy_record_for_test(&PassEnergyRecord {
                inscription_id,
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 100_000,
                owner_delta: 0,
                energy: 42,
            })
            .unwrap();

        let err = manager
            .on_pass_burned(&inscription_id, 101)
            .await
            .unwrap_err();
        assert!(err.contains("Energy state mismatch before burned transition"));

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Dormant);
        assert!(
            manager
                .energy_manager
                .get_pass_energy_record_exact(&inscription_id, 101)
                .unwrap()
                .is_none()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_burned_from_active_writes_burned_energy_terminal() {
        let (root_dir, storage, manager) = setup_empty_manager("burn_active_energy_terminal");
        let inscription_id = test_inscription_id(61, 0);
        let owner = test_script_hash(61);
        let pass = test_pass_info(
            inscription_id,
            owner,
            100,
            MinerPassKind::Standard,
            MinerPassState::Active,
        );
        storage
            .add_new_mint_pass_at_height(&pass, pass.mint_block_height)
            .unwrap();
        manager
            .energy_manager
            .insert_pass_energy_record_for_test(&PassEnergyRecord {
                inscription_id,
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 200_000,
                owner_delta: 0,
                energy: 7,
            })
            .unwrap();

        manager.on_pass_burned(&inscription_id, 105).await.unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Burned);

        let exact_105 = manager
            .energy_manager
            .get_pass_energy_record_exact(&inscription_id, 105)
            .unwrap()
            .unwrap();
        assert_eq!(exact_105.state, MinerPassState::Burned);
        assert_eq!(exact_105.energy, 0);

        let energy_106 = manager
            .energy_manager
            .get_pass_energy(&inscription_id, 106)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(energy_106.state, MinerPassState::Burned);
        assert_eq!(energy_106.energy, 0);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_on_pass_burned_from_consumed_keeps_consumed_state() {
        let (root_dir, storage, manager, inscription_id, owner, _satpoint) =
            setup_manager("burn_consumed_noop");

        storage
            .update_state(
                &inscription_id,
                MinerPassState::Consumed,
                MinerPassState::Dormant,
            )
            .unwrap();
        manager
            .energy_manager
            .on_pass_consumed(&inscription_id, &owner, 101)
            .unwrap();

        manager.on_pass_burned(&inscription_id, 102).await.unwrap();

        let updated = storage
            .get_pass_by_inscription_id(&inscription_id)
            .unwrap()
            .unwrap();
        assert_eq!(updated.state, MinerPassState::Consumed);

        let energy_102 = manager
            .energy_manager
            .get_pass_energy(&inscription_id, 102)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(energy_102.state, MinerPassState::Consumed);
        assert_eq!(energy_102.energy, 0);
        assert!(
            manager
                .energy_manager
                .get_pass_energy_record_exact(&inscription_id, 102)
                .unwrap()
                .is_none()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }
}
