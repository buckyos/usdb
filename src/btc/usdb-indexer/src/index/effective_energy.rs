use super::content::{MinerPassKind, MinerPassState};
use super::energy::{PassEnergyManagerRef, PassEnergyResult};
use super::energy_formula::{Energy, calc_collab_contribution, calc_standard_effective_energy};
use crate::storage::{MinerPassSnapshotInfo, MinerPassStorageRef, PassEnergyRecord};
use bitcoincore_rpc::bitcoin::Network;
use ord::InscriptionId;
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use usdb_util::{USDBScriptHash, address_string_to_script_hash};

/// Query mode used by derived UIP-0004 energy resolution.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DerivedPassEnergyMode {
    /// Require a raw energy record exactly at the requested height.
    Exact,
    /// Use the latest raw record at or before the requested height and project it to that height.
    AtOrBefore,
}

/// Runtime-derived energy view for one pass at one BTC height.
///
/// This view keeps UIP-0003 raw energy separate from UIP-0004 effective energy:
/// collab contribution and effective energy are derived at query time and are
/// never written back to the raw energy ledger.
#[derive(Clone, Debug)]
pub struct DerivedPassEnergySnapshot {
    /// Stored raw energy record used as the query base.
    pub record: PassEnergyRecord,
    /// Pass state at the query height.
    pub state: MinerPassState,
    /// Pass kind at the query height.
    pub pass_kind: MinerPassKind,
    /// UIP-0003 raw energy at the query height.
    pub raw_energy: Energy,
    /// Aggregated UIP-0004 collab contribution at the query height.
    pub collab_contribution: Energy,
    /// UIP-0004 effective energy at the query height.
    pub effective_energy: Energy,
    /// Number of active collab passes included in `collab_contribution`.
    pub collab_breakdown_count: u64,
}

/// Batch-derived candidate energy for one active standard pass.
pub struct DerivedCandidateEnergySnapshot {
    /// Historical active standard pass snapshot used for owner and kind fields.
    pub pass: MinerPassSnapshotInfo,
    /// Stored raw energy record used as the projection base.
    pub record: PassEnergyRecord,
    /// UIP-0003 raw energy projected to the candidate-set height.
    pub raw_energy: Energy,
    /// Aggregate UIP-0004 contribution from active collab passes.
    pub collab_contribution: Energy,
    /// Saturating sum of raw energy and collab contribution.
    pub effective_energy: Energy,
    /// Number of active collab passes included in the aggregate.
    pub collab_breakdown_count: u64,
}

/// Failure category for batch candidate derivation.
pub enum CandidateSetDerivationError {
    /// SQLite/RocksDB or serialization failure while reading durable state.
    Storage(String),
    /// Durable pass and energy rows disagree with candidate-set invariants.
    Invariant {
        /// Pass most directly associated with the violated invariant.
        pass_id: String,
        /// Stable diagnostic detail for logs and structured RPC errors.
        detail: String,
    },
}

impl From<String> for CandidateSetDerivationError {
    fn from(value: String) -> Self {
        Self::Storage(value)
    }
}

/// One audited collab contribution item resolved for a Leader at one BTC height.
#[derive(Clone, Debug)]
pub struct DerivedCollabBreakdownItem {
    /// Collab pass inscription id.
    pub collab_pass_id: InscriptionId,
    /// Collab pass owner script hash at the query height.
    pub collab_owner: USDBScriptHash,
    /// Raw energy record height used for the collab pass.
    pub record_block_height: u32,
    /// UIP-0003 raw energy projected to the query height.
    pub collab_raw_energy: Energy,
    /// UIP-0004 contribution after applying collab weight.
    pub collab_contribution: Energy,
    /// Leader reference kind declared by the collab pass.
    pub leader_ref_kind: String,
    /// Original Leader reference value declared by the collab pass.
    pub leader_ref_value: String,
}

/// Full collab breakdown for one Leader at one BTC height.
pub struct DerivedCollabBreakdown {
    /// Leader pass snapshot at the query height.
    pub leader: MinerPassSnapshotInfo,
    /// Sum of all item contributions.
    pub aggregate_collab_contribution: Energy,
    /// All collab items contributing to this Leader before RPC pagination.
    pub items: Vec<DerivedCollabBreakdownItem>,
}

struct CollabBreakdownCollector<'a> {
    leader_pass_id: &'a InscriptionId,
    leader_owner: &'a USDBScriptHash,
    block_height: u32,
    seen_collabs: &'a mut BTreeSet<InscriptionId>,
    aggregate: &'a mut Energy,
    items: &'a mut Vec<DerivedCollabBreakdownItem>,
}

#[derive(Clone, Copy)]
enum CollabRelationSource {
    LeaderPassId,
    LeaderBtcOwner,
}

impl CollabRelationSource {
    fn as_str(self) -> &'static str {
        match self {
            Self::LeaderPassId => "leader_pass_id",
            Self::LeaderBtcOwner => "leader_btc_owner",
        }
    }
}

/// Read-only UIP-0004 effective energy resolver.
///
/// The resolver combines raw energy records, pass history snapshots, and
/// collab Leader resolution. It intentionally has no write path, so candidate
/// scoring and RPC reads cannot mutate the UIP-0003 raw energy database.
pub struct EffectiveEnergyResolver {
    pass_storage: MinerPassStorageRef,
    pass_energy_manager: PassEnergyManagerRef,
    network: Network,
    collab_page_size: usize,
}

/// Shared reference to the runtime derived effective energy resolver.
pub type EffectiveEnergyResolverRef = Arc<EffectiveEnergyResolver>;

impl EffectiveEnergyResolver {
    /// Build a read-only resolver over pass storage, raw energy, and Leader resolution.
    pub fn new(
        pass_storage: MinerPassStorageRef,
        pass_energy_manager: PassEnergyManagerRef,
        network: Network,
        collab_page_size: usize,
    ) -> Self {
        Self {
            pass_storage,
            pass_energy_manager,
            network,
            collab_page_size: collab_page_size.max(1),
        }
    }

    /// Resolve raw, collab contribution, and effective energy for one pass.
    ///
    /// `raw_energy` follows the requested mode for the target pass. Collab
    /// contributions always use UIP-0003 raw energy at the same query height,
    /// resolved with at-or-before projection so a collab does not need to have
    /// an exact record at every query height.
    pub fn resolve_pass_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
        mode: DerivedPassEnergyMode,
    ) -> Result<Option<DerivedPassEnergySnapshot>, String> {
        let Some((record, raw_result)) =
            self.resolve_target_raw_energy(inscription_id, block_height, mode)?
        else {
            return Ok(None);
        };

        let Some(pass_snapshot) = self
            .pass_storage
            .get_pass_snapshot_from_history_at_height(inscription_id, block_height)?
        else {
            let msg = format!(
                "Pass snapshot missing while resolving effective energy: inscription_id={}, block_height={}, record_height={}",
                inscription_id, block_height, record.block_height
            );
            error!("{}", msg);
            return Err(msg);
        };

        let state = pass_snapshot.pass.state.clone();
        if raw_result.state != state {
            let msg = format!(
                "Pass state and energy state mismatch while resolving effective energy: inscription_id={}, block_height={}, pass_state={}, energy_state={}, record_height={}",
                inscription_id,
                block_height,
                state.as_str(),
                raw_result.state.as_str(),
                record.block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let pass_kind = pass_snapshot.pass.pass_kind;
        let (collab_contribution, effective_energy, collab_breakdown_count) =
            if state == MinerPassState::Active && pass_kind == MinerPassKind::Standard {
                let (collab_contribution, collab_breakdown_count) = self
                    .resolve_standard_collab_summary(
                        inscription_id,
                        &pass_snapshot.pass.owner,
                        block_height,
                    )?;
                (
                    collab_contribution,
                    calc_standard_effective_energy(raw_result.energy, collab_contribution),
                    collab_breakdown_count,
                )
            } else {
                (0, 0, 0)
            };

        Ok(Some(DerivedPassEnergySnapshot {
            record,
            state,
            pass_kind,
            raw_energy: raw_result.energy,
            collab_contribution,
            effective_energy,
            collab_breakdown_count,
        }))
    }

    /// Resolve all active standard candidate energies with one pass-history scan.
    ///
    /// This is equivalent to resolving every active standard pass separately,
    /// but builds fixed-id and address-owner Leader maps once. Raw energy is
    /// still read independently for every standard and collab pass so UIP-0003
    /// projection and corruption checks remain unchanged.
    pub fn resolve_active_standard_candidate_set(
        &self,
        block_height: u32,
    ) -> Result<Vec<DerivedCandidateEnergySnapshot>, CandidateSetDerivationError> {
        let standards = self.load_all_active_passes(block_height, MinerPassKind::Standard)?;
        if standards.is_empty() {
            return Ok(Vec::new());
        }
        let collabs = self.load_all_active_passes(block_height, MinerPassKind::Collab)?;

        let mut standard_by_id = HashMap::with_capacity(standards.len());
        let mut standard_by_owner = HashMap::with_capacity(standards.len());
        for (index, standard) in standards.iter().enumerate() {
            standard_by_id.insert(standard.pass.inscription_id.to_string(), index);
            let owner = standard.pass.owner.to_string();
            if let Some(previous) = standard_by_owner.insert(owner.clone(), index) {
                let msg = format!(
                    "Multiple active standard passes share one owner while building candidate set: block_height={}, owner={}, first_pass_id={}, second_pass_id={}",
                    block_height,
                    owner,
                    standards[previous].pass.inscription_id,
                    standard.pass.inscription_id
                );
                error!("{}", msg);
                return Err(CandidateSetDerivationError::Invariant {
                    pass_id: standard.pass.inscription_id.to_string(),
                    detail: msg,
                });
            }
        }

        let mut collab_aggregates = vec![(0u128, 0u64); standards.len()];
        for collab in collabs {
            let leader_index = match (
                collab.pass.leader_pass_id.as_ref(),
                collab.pass.leader_btc_addr.as_deref(),
                collab.pass.leader_btc_owner.as_ref(),
            ) {
                (Some(leader_pass_id), None, None) => {
                    standard_by_id.get(&leader_pass_id.to_string()).copied()
                }
                (None, Some(leader_btc_addr), Some(stored_leader_owner)) => {
                    let normalized_owner = address_string_to_script_hash(
                        leader_btc_addr,
                        &self.network,
                    )
                    .map_err(|error| {
                        let detail = format!(
                            "Invalid collab Leader BTC address while building candidate set: collab_pass_id={}, leader_btc_addr={}, network={}, error={}",
                            collab.pass.inscription_id,
                            leader_btc_addr,
                            self.network,
                            error
                        );
                        error!("{}", detail);
                        CandidateSetDerivationError::Invariant {
                            pass_id: collab.pass.inscription_id.to_string(),
                            detail,
                        }
                    })?;
                    if normalized_owner != *stored_leader_owner {
                        let msg = format!(
                            "Collab Leader BTC owner mismatch while building candidate set: collab_pass_id={}, leader_btc_addr={}, stored_owner={}, normalized_owner={}, network={}",
                            collab.pass.inscription_id,
                            leader_btc_addr,
                            stored_leader_owner,
                            normalized_owner,
                            self.network
                        );
                        error!("{}", msg);
                        return Err(CandidateSetDerivationError::Invariant {
                            pass_id: collab.pass.inscription_id.to_string(),
                            detail: msg,
                        });
                    }
                    standard_by_owner
                        .get(&normalized_owner.to_string())
                        .copied()
                }
                (None, None, None) => None,
                _ => {
                    let msg = format!(
                        "Invalid collab Leader reference while building candidate set: collab_pass_id={}, block_height={}",
                        collab.pass.inscription_id, block_height
                    );
                    error!("{}", msg);
                    return Err(CandidateSetDerivationError::Invariant {
                        pass_id: collab.pass.inscription_id.to_string(),
                        detail: msg,
                    });
                }
            };
            let Some(leader_index) = leader_index else {
                continue;
            };
            let Some((_, collab_raw)) = self.resolve_raw_energy_record_at_or_before(
                &collab.pass.inscription_id,
                block_height,
            )?
            else {
                let msg = format!(
                    "Active collab pass missing raw energy while building candidate set: collab_pass_id={}, block_height={}",
                    collab.pass.inscription_id, block_height
                );
                error!("{}", msg);
                return Err(CandidateSetDerivationError::Invariant {
                    pass_id: collab.pass.inscription_id.to_string(),
                    detail: msg,
                });
            };
            if collab_raw.state != MinerPassState::Active {
                let msg = format!(
                    "Active collab pass energy state mismatch while building candidate set: collab_pass_id={}, block_height={}, energy_state={}",
                    collab.pass.inscription_id,
                    block_height,
                    collab_raw.state.as_str()
                );
                error!("{}", msg);
                return Err(CandidateSetDerivationError::Invariant {
                    pass_id: collab.pass.inscription_id.to_string(),
                    detail: msg,
                });
            }
            let contribution = calc_collab_contribution(collab_raw.energy);
            let aggregate = &mut collab_aggregates[leader_index];
            aggregate.0 = aggregate.0.saturating_add(contribution);
            aggregate.1 = aggregate.1.saturating_add(1);
        }

        let mut candidates = Vec::with_capacity(standards.len());
        for (standard, (collab_contribution, collab_breakdown_count)) in
            standards.into_iter().zip(collab_aggregates)
        {
            let Some((record, raw_result)) = self.resolve_target_raw_energy(
                &standard.pass.inscription_id,
                block_height,
                DerivedPassEnergyMode::AtOrBefore,
            )?
            else {
                let msg = format!(
                    "Active standard pass missing raw energy while building candidate set: pass_id={}, block_height={}",
                    standard.pass.inscription_id, block_height
                );
                error!("{}", msg);
                return Err(CandidateSetDerivationError::Invariant {
                    pass_id: standard.pass.inscription_id.to_string(),
                    detail: msg,
                });
            };
            if raw_result.state != MinerPassState::Active {
                let msg = format!(
                    "Active standard pass energy state mismatch while building candidate set: pass_id={}, block_height={}, energy_state={}",
                    standard.pass.inscription_id,
                    block_height,
                    raw_result.state.as_str()
                );
                error!("{}", msg);
                return Err(CandidateSetDerivationError::Invariant {
                    pass_id: standard.pass.inscription_id.to_string(),
                    detail: msg,
                });
            }
            candidates.push(DerivedCandidateEnergySnapshot {
                pass: standard,
                record,
                raw_energy: raw_result.energy,
                collab_contribution,
                effective_energy: calc_standard_effective_energy(
                    raw_result.energy,
                    collab_contribution,
                ),
                collab_breakdown_count,
            });
        }
        Ok(candidates)
    }

    fn load_all_active_passes(
        &self,
        block_height: u32,
        pass_kind: MinerPassKind,
    ) -> Result<Vec<MinerPassSnapshotInfo>, String> {
        let mut page = 0usize;
        let mut passes = Vec::new();
        loop {
            let rows = match pass_kind {
                MinerPassKind::Standard => self
                    .pass_storage
                    .get_active_standard_passes_by_page_from_history_at_height(
                        page,
                        self.collab_page_size,
                        block_height,
                    )?,
                MinerPassKind::Collab => self
                    .pass_storage
                    .get_active_collab_passes_by_page_from_history_at_height(
                        page,
                        self.collab_page_size,
                        block_height,
                    )?,
            };
            let page_len = rows.len();
            passes.extend(rows);
            if page_len < self.collab_page_size {
                break;
            }
            page = page.checked_add(1).ok_or_else(|| {
                let msg = format!(
                    "Pass pagination overflow while building candidate set: block_height={}, pass_kind={}",
                    block_height,
                    pass_kind.as_str()
                );
                error!("{}", msg);
                msg
            })?;
        }
        Ok(passes)
    }

    /// Resolve all active collab pass contributions for one Leader pass.
    ///
    /// Missing Leader history returns `Ok(None)`. A present non-active or
    /// non-standard pass returns an empty breakdown because UIP-0004 only lets
    /// active standard passes receive effective collab contribution.
    pub fn resolve_collab_breakdown(
        &self,
        leader_pass_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<DerivedCollabBreakdown>, String> {
        let Some(leader) = self
            .pass_storage
            .get_pass_snapshot_from_history_at_height(leader_pass_id, block_height)?
        else {
            return Ok(None);
        };

        if leader.pass.state != MinerPassState::Active
            || leader.pass.pass_kind != MinerPassKind::Standard
        {
            return Ok(Some(DerivedCollabBreakdown {
                leader,
                aggregate_collab_contribution: 0,
                items: Vec::new(),
            }));
        }

        let (aggregate_collab_contribution, items) = self.resolve_standard_collab_breakdown_items(
            leader_pass_id,
            &leader.pass.owner,
            block_height,
        )?;

        Ok(Some(DerivedCollabBreakdown {
            leader,
            aggregate_collab_contribution,
            items,
        }))
    }

    fn resolve_target_raw_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
        mode: DerivedPassEnergyMode,
    ) -> Result<Option<(PassEnergyRecord, PassEnergyResult)>, String> {
        let record = match mode {
            DerivedPassEnergyMode::Exact => self
                .pass_energy_manager
                .get_pass_energy_record_exact(inscription_id, block_height)?,
            DerivedPassEnergyMode::AtOrBefore => self
                .pass_energy_manager
                .get_pass_energy_record_at_or_before(inscription_id, block_height)?,
        };
        let Some(record) = record else {
            return Ok(None);
        };

        let raw_result = match mode {
            DerivedPassEnergyMode::Exact => PassEnergyResult {
                energy: record.energy,
                state: record.state.clone(),
            },
            DerivedPassEnergyMode::AtOrBefore => self
                .pass_energy_manager
                .project_energy_record_no_balance_change(&record, block_height),
        };

        Ok(Some((record, raw_result)))
    }

    fn resolve_standard_collab_summary(
        &self,
        leader_pass_id: &InscriptionId,
        leader_owner: &USDBScriptHash,
        block_height: u32,
    ) -> Result<(Energy, u64), String> {
        let (aggregate, items) = self.resolve_standard_collab_breakdown_items(
            leader_pass_id,
            leader_owner,
            block_height,
        )?;
        Ok((aggregate, items.len() as u64))
    }

    fn resolve_standard_collab_breakdown_items(
        &self,
        leader_pass_id: &InscriptionId,
        leader_owner: &USDBScriptHash,
        block_height: u32,
    ) -> Result<(Energy, Vec<DerivedCollabBreakdownItem>), String> {
        let mut seen_collabs = BTreeSet::new();
        let mut aggregate: Energy = 0;
        let mut items = Vec::new();

        let mut collector = CollabBreakdownCollector {
            leader_pass_id,
            leader_owner,
            block_height,
            seen_collabs: &mut seen_collabs,
            aggregate: &mut aggregate,
            items: &mut items,
        };

        self.collect_collab_breakdown_pages(
            |page| {
                self.pass_storage
                    .get_active_collab_passes_by_leader_pass_id_from_history_at_height(
                        page,
                        self.collab_page_size,
                        block_height,
                        leader_pass_id,
                    )
            },
            &mut collector,
            CollabRelationSource::LeaderPassId,
        )?;

        self.collect_collab_breakdown_pages(
            |page| {
                self.pass_storage
                    .get_active_collab_passes_by_leader_btc_owner_from_history_at_height(
                        page,
                        self.collab_page_size,
                        block_height,
                        leader_owner,
                    )
            },
            &mut collector,
            CollabRelationSource::LeaderBtcOwner,
        )?;

        Ok((aggregate, items))
    }

    fn collect_collab_breakdown_pages<F>(
        &self,
        mut load_page: F,
        collector: &mut CollabBreakdownCollector<'_>,
        source: CollabRelationSource,
    ) -> Result<(), String>
    where
        F: FnMut(usize) -> Result<Vec<MinerPassSnapshotInfo>, String>,
    {
        let mut page = 0usize;

        loop {
            let collabs = load_page(page)?;
            if collabs.is_empty() {
                break;
            }

            for collab_snapshot in &collabs {
                if !collector
                    .seen_collabs
                    .insert(collab_snapshot.pass.inscription_id)
                {
                    continue;
                }

                let (leader_ref_kind, leader_ref_value) = match source {
                    CollabRelationSource::LeaderPassId => match (
                        collab_snapshot.pass.leader_pass_id.as_ref(),
                        collab_snapshot.pass.leader_btc_addr.as_ref(),
                        collab_snapshot.pass.leader_btc_owner.as_ref(),
                    ) {
                        (Some(leader_pass_id), None, None)
                            if leader_pass_id == collector.leader_pass_id =>
                        {
                            ("leader_pass_id", leader_pass_id.to_string())
                        }
                        _ => {
                            let msg = format!(
                                "Invalid fixed Leader relation returned by storage: collab_pass_id={}, leader_pass_id={}, block_height={}",
                                collab_snapshot.pass.inscription_id,
                                collector.leader_pass_id,
                                collector.block_height
                            );
                            error!("{}", msg);
                            return Err(msg);
                        }
                    },
                    CollabRelationSource::LeaderBtcOwner => match (
                        collab_snapshot.pass.leader_pass_id.as_ref(),
                        collab_snapshot.pass.leader_btc_addr.as_deref(),
                        collab_snapshot.pass.leader_btc_owner.as_ref(),
                    ) {
                        (None, Some(leader_btc_addr), Some(stored_leader_owner)) => {
                            let normalized_owner = address_string_to_script_hash(
                                leader_btc_addr,
                                &self.network,
                            )
                            .map_err(|error| {
                                let msg = format!(
                                    "Invalid address Leader relation returned by storage: collab_pass_id={}, leader_btc_addr={}, network={}, error={}",
                                    collab_snapshot.pass.inscription_id,
                                    leader_btc_addr,
                                    self.network,
                                    error
                                );
                                error!("{}", msg);
                                msg
                            })?;
                            if normalized_owner != *stored_leader_owner
                                || normalized_owner != *collector.leader_owner
                            {
                                let msg = format!(
                                    "Address Leader owner mismatch returned by storage: collab_pass_id={}, leader_pass_id={}, stored_owner={}, normalized_owner={}, expected_owner={}, network={}",
                                    collab_snapshot.pass.inscription_id,
                                    collector.leader_pass_id,
                                    stored_leader_owner,
                                    normalized_owner,
                                    collector.leader_owner,
                                    self.network
                                );
                                error!("{}", msg);
                                return Err(msg);
                            }
                            ("leader_btc_addr", leader_btc_addr.to_string())
                        }
                        _ => {
                            let msg = format!(
                                "Invalid address Leader relation returned by storage: collab_pass_id={}, leader_pass_id={}, block_height={}",
                                collab_snapshot.pass.inscription_id,
                                collector.leader_pass_id,
                                collector.block_height
                            );
                            error!("{}", msg);
                            return Err(msg);
                        }
                    },
                };

                let Some((record_block_height, collab_raw)) = self
                    .resolve_raw_energy_record_at_or_before(
                        &collab_snapshot.pass.inscription_id,
                        collector.block_height,
                    )?
                else {
                    let msg = format!(
                        "Active collab pass missing raw energy while resolving Leader contribution: collab_inscription_id={}, leader_inscription_id={}, block_height={}",
                        collab_snapshot.pass.inscription_id,
                        collector.leader_pass_id,
                        collector.block_height
                    );
                    error!("{}", msg);
                    return Err(msg);
                };
                if collab_raw.state != MinerPassState::Active {
                    let msg = format!(
                        "Active collab pass energy state mismatch while resolving Leader contribution: collab_inscription_id={}, leader_inscription_id={}, block_height={}, energy_state={}",
                        collab_snapshot.pass.inscription_id,
                        collector.leader_pass_id,
                        collector.block_height,
                        collab_raw.state.as_str()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                let collab_contribution = calc_collab_contribution(collab_raw.energy);
                *collector.aggregate = (*collector.aggregate).saturating_add(collab_contribution);
                collector.items.push(DerivedCollabBreakdownItem {
                    collab_pass_id: collab_snapshot.pass.inscription_id,
                    collab_owner: collab_snapshot.pass.owner,
                    record_block_height,
                    collab_raw_energy: collab_raw.energy,
                    collab_contribution,
                    leader_ref_kind: leader_ref_kind.to_string(),
                    leader_ref_value,
                });
            }

            if collabs.len() < self.collab_page_size {
                break;
            }
            page = page.checked_add(1).ok_or_else(|| {
                let msg = format!(
                    "Collab pagination overflow while resolving effective energy: leader_inscription_id={}, block_height={}, source={}",
                    collector.leader_pass_id,
                    collector.block_height,
                    source.as_str()
                );
                error!("{}", msg);
                msg
            })?;
        }

        Ok(())
    }

    fn resolve_raw_energy_record_at_or_before(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<(u32, PassEnergyResult)>, String> {
        let Some(record) = self
            .pass_energy_manager
            .get_pass_energy_record_at_or_before(inscription_id, block_height)?
        else {
            return Ok(None);
        };
        let record_block_height = record.block_height;
        Ok(Some((
            record_block_height,
            self.pass_energy_manager
                .project_energy_record_no_balance_change(&record, block_height),
        )))
    }
}
