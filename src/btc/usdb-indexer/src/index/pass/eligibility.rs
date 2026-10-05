//! UIP-0016 state-machine execution with explicit event-height schema and admission selection.

use std::collections::BTreeSet;

use crate::index::rules::{MintSchemaRules, PassStateRules};
use usdb_util::{BtcRuleContext, SourceAuthorization};

use super::{
    MinerPassManager, MintOperationPath, MintStateValidationError, PassMintInscriptionInfo,
};
use crate::btc::{
    mint_evidence::{MintEvidenceContext, MintSatOutcome, MintSourceOutcome},
    transaction_balance::TransactionBalanceContext,
};
use crate::index::{MinerPassState, MintValidationErrorCode, PassBlockMutation};
use crate::storage::{MinerPassInfo, MinerPassMintAudit, MintSourceAudit};

impl MinerPassManager {
    /// Apply a mint under explicitly selected schema/state rules using ordered chain evidence.
    /// The caller must derive payload fields from this inscription and enforce the active schema.
    /// Returns None for a recorded protocol Invalid, or the successful path. Data/I/O errors
    /// require the caller to abort the whole block, including energy, SQLite and tracker staging.
    pub(crate) async fn on_mint_pass(
        &self,
        mint: &PassMintInscriptionInfo,
        evidence: &MintEvidenceContext,
        balances: &TransactionBalanceContext,
        rules: &BtcRuleContext,
    ) -> Result<Option<MintOperationPath>, String> {
        self.require_rule_context(mint.mint_block_height, rules)?;
        let schema = MintSchemaRules::at(rules)?;
        let state = PassStateRules::at(rules)?;
        let mut audit = MinerPassMintAudit {
            schema_version: "miner-pass-mint-audit:v1".into(),
            inscription_id: mint.inscription_id.to_string(),
            block_height: mint.mint_block_height,
            block_hash: String::new(),
            recipient: Some(mint.mint_owner),
            balance_before_tx: None,
            ever_valid_owner: None,
            source: None,
            operation_path: None,
            error_code: None,
            error_reason: None,
        };
        let result = async {
            let path = match state {
                PassStateRules::V2 => {
                    self.apply_mint_v2(mint, evidence, balances, &mut audit, schema, state)
                        .await?
                }
                // This test contract retains v2 state representation and adds only admission checks.
                #[cfg(test)]
                PassStateRules::ConformanceNoNewCollab => {
                    self.apply_mint_v2(mint, evidence, balances, &mut audit, schema, state)
                        .await?
                }
            };
            audit.operation_path = path.map(|path| {
                match path {
                    MintOperationPath::FirstOpening => "first_opening",
                    MintOperationPath::SameOwner => "same_owner",
                    MintOperationPath::CrossOwner => "cross_owner",
                }
                .into()
            });
            if path.is_none() {
                let pass = self
                    .storage
                    .get_pass_by_inscription_id(&mint.inscription_id)?
                    .ok_or("Rejected mint missing from writer state")?;
                audit.error_code = pass.invalid_code;
                audit.error_reason = pass.invalid_reason;
            }
            self.storage.put_mint_audit(&audit)?;
            Ok(path)
        }
        .await;
        result.map_err(|err: String| {
            // Keep the same operation context in logs and in errors propagated to the block executor.
            let msg = format!(
                "MinerPass v2 block processing failed: module=pass_manager, action=apply_mint, inscription_id={}, block_height={}, mint_txid={}, mint_owner={}, satpoint={}, registry_id={}, active_version_set_id={}, error={err}",
                mint.inscription_id,
                mint.mint_block_height,
                mint.mint_txid,
                mint.mint_owner,
                mint.satpoint,
                rules.activation_registry_id(),
                rules.active_version_set_id()
            );
            error!("{msg}");
            msg
        })
    }

    async fn apply_mint_v2(
        &self,
        mint: &PassMintInscriptionInfo,
        evidence: &MintEvidenceContext,
        balances: &TransactionBalanceContext,
        audit: &mut MinerPassMintAudit,
        schema: MintSchemaRules,
        state: PassStateRules,
    ) -> Result<Option<MintOperationPath>, String> {
        self.storage.require_block_savepoint()?;
        self.energy_manager
            .require_pending_block(mint.mint_block_height)?;
        let collector_height = self
            .current_block_collector
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| c.block_height());
        if collector_height != Some(mint.mint_block_height) {
            return Err(format!(
                "MinerPass v2 requires matching block mutation collector: expected_height={}, collector_height={collector_height:?}",
                mint.mint_block_height
            ));
        }
        let inputs = evidence.block_prevouts()?;
        audit.block_hash = inputs.block().block_hash().to_string();
        if inputs.height() != mint.mint_block_height {
            return Err(format!(
                "Mint height disagrees with evidence block: mint_height={}, evidence_height={}, evidence_block_hash={}",
                mint.mint_block_height,
                inputs.height(),
                inputs.block().block_hash()
            ));
        }
        balances.require_matching_block(&inputs)?;
        if mint.mint_txid != mint.inscription_id.txid {
            return Err(format!(
                "Mint transaction disagrees with inscription id: mint_txid={}, inscription_txid={}",
                mint.mint_txid, mint.inscription_id.txid
            ));
        }
        if mint.mint_version != schema.payload_version() {
            return self
                .reject_v2(
                    mint,
                    MintValidationErrorCode::InvalidSchema,
                    format!(
                        "MinerPass state machine v2 requires mint schema v{}: actual={}",
                        schema.payload_version(),
                        mint.mint_version
                    ),
                )
                .await;
        }
        // A new admission restriction never revalidates existing passes or partially consumes prev.
        match state {
            PassStateRules::V2 => {}
            #[cfg(test)]
            PassStateRules::ConformanceNoNewCollab => {
                if mint.pass_kind == crate::index::MinerPassKind::Collab {
                    return self
                        .reject_v2(
                            mint,
                            MintValidationErrorCode::InvalidUsdbCollab,
                            "Conformance state rule rejects new collaboration mints".into(),
                        )
                        .await;
                }
            }
        }
        let sat = match evidence.locate_mint(mint.inscription_id)? {
            MintSatOutcome::Located(sat) => sat,
            MintSatOutcome::Unsupported(reason) => {
                return self
                    .reject_v2(
                        mint,
                        MintValidationErrorCode::UnsupportedInscription,
                        reason.into(),
                    )
                    .await;
            }
        };
        if sat.mint_owner != mint.mint_owner || sat.satpoint != mint.satpoint {
            return Err(format!(
                "Mint destination/satpoint disagrees with actual reveal sat: mint_owner={}, reveal_owner={}, mint_satpoint={}, reveal_satpoint={}",
                mint.mint_owner, sat.mint_owner, mint.satpoint, sat.satpoint
            ));
        }
        let position = inputs
            .block()
            .txdata
            .iter()
            .position(|tx| tx.compute_txid() == mint.mint_txid)
            .ok_or_else(|| {
                format!(
                    "Mint transaction absent from evidence block: mint_txid={}, evidence_height={}, evidence_block_hash={}, transaction_count={}",
                    mint.mint_txid,
                    inputs.height(),
                    inputs.block().block_hash(),
                    inputs.block().txdata.len()
                )
            })?;
        let balance = balances.balance_before(&mint.mint_owner, position)?;
        let occupied = self
            .storage
            .has_ever_valid_owner(&mint.mint_owner, mint.mint_block_height)?;
        audit.balance_before_tx = Some(balance);
        audit.ever_valid_owner = Some(occupied);
        let can_open = balance == 0 && !occupied;

        // Only a true first opening can avoid loading historical source authorization. The
        // same recipient may already have acquired a valid pass in an earlier ordered event.
        let (path, source_owner) = if can_open && mint.prev.is_empty() {
            (MintOperationPath::FirstOpening, mint.mint_owner)
        } else {
            let proof = match evidence.prove_source(&sat)? {
                MintSourceOutcome::Proven(proof) => proof,
                MintSourceOutcome::CoinbaseCommit => {
                    return self
                        .reject_v2(
                            mint,
                            MintValidationErrorCode::UnauthorizedSource,
                            "Coinbase commit has no spending owner".into(),
                        )
                        .await;
                }
            };
            audit.source = Some(MintSourceAudit::from(&proof));
            if let SourceAuthorization::Unsupported(reason) = proof.authorization {
                return self
                    .reject_v2(mint, MintValidationErrorCode::UnauthorizedSource, reason)
                    .await;
            }
            if proof.source_owner == mint.mint_owner {
                (MintOperationPath::SameOwner, proof.source_owner)
            } else if !can_open {
                let reason = format!(
                    "Destination cannot open: balance_before_tx={balance}, ever_valid_owner={occupied}; source differs from destination"
                );
                return self
                    .reject_v2(mint, MintValidationErrorCode::IneligibleRecipient, reason)
                    .await;
            } else if mint.prev.is_empty() {
                // Defensive: first-opening precedence currently makes this branch unreachable.
                return self
                    .reject_v2(
                        mint,
                        MintValidationErrorCode::InvalidPrevId,
                        "Cross-owner inheritance requires prev".into(),
                    )
                    .await;
            } else {
                (MintOperationPath::CrossOwner, proof.source_owner)
            }
        };

        if let Some(invalid) = self.validate_leader_pass_binding(mint)? {
            self.record_invalid_mint_from_mint_info(mint, invalid)
                .await?;
            return Ok(None);
        }
        // Normalize before any source dormancy or prev consumption, including address references.
        let leader_btc_owner = match self.resolve_leader_btc_owner_for_mint(mint) {
            Ok(owner) => owner,
            Err(reason) => {
                return self
                    .reject_v2(mint, MintValidationErrorCode::InvalidLeaderBtcAddr, reason)
                    .await;
            }
        };
        let mut seen = BTreeSet::new();
        let mut source_active = Vec::new();
        for id in &mint.prev {
            if !seen.insert(*id) {
                return self
                    .reject_v2(
                        mint,
                        MintValidationErrorCode::InvalidPrevId,
                        format!("Duplicate prev inscription id {id}"),
                    )
                    .await;
            }
            let Some(pass) = self.storage.get_pass_by_inscription_id(id)? else {
                return self
                    .reject_v2(
                        mint,
                        MintValidationErrorCode::InvalidPrevId,
                        format!("Previous miner pass {id} not found"),
                    )
                    .await;
            };
            if pass.owner != source_owner
                || !matches!(pass.state, MinerPassState::Active | MinerPassState::Dormant)
            {
                let reason = format!(
                    "Previous miner pass {id} is not eligible for source {source_owner}: owner={}, state={}",
                    pass.owner,
                    pass.state.as_str()
                );
                return self
                    .reject_v2(mint, MintValidationErrorCode::InvalidPrevId, reason)
                    .await;
            }
            if path == MintOperationPath::CrossOwner && pass.state == MinerPassState::Active {
                source_active.push(pass);
            }
        }

        // All protocol validations finish before the first mutation. Runtime errors below are
        // block failures; the existing savepoint/pending-energy recovery must roll them back.
        match path {
            MintOperationPath::FirstOpening => {}
            MintOperationPath::SameOwner => self.dormant_last_pass(mint).await?,
            MintOperationPath::CrossOwner => {
                for pass in &source_active {
                    self.dormant_referenced_source(pass, mint.mint_block_height)
                        .await?;
                }
            }
        }
        self.create_pass_and_consume_prev(mint, &source_owner, leader_btc_owner)
            .await?;
        Ok(Some(path))
    }

    async fn reject_v2(
        &self,
        mint: &PassMintInscriptionInfo,
        code: MintValidationErrorCode,
        reason: String,
    ) -> Result<Option<MintOperationPath>, String> {
        self.record_invalid_mint_from_mint_info(mint, MintStateValidationError { code, reason })
            .await?;
        Ok(None)
    }

    // Cross-owner inheritance freezes only the referenced source Active. It must not supersede
    // an unreferenced source pass or any target pass, unlike a same-owner remint.
    async fn dormant_referenced_source(
        &self,
        pass: &MinerPassInfo,
        height: u32,
    ) -> Result<(), String> {
        self.energy_manager
            .on_pass_dormant(&pass.inscription_id, height)
            .await?;
        self.storage.update_state_at_height(
            &pass.inscription_id,
            MinerPassState::Dormant,
            MinerPassState::Active,
            height,
        )?;
        self.push_block_mutation(PassBlockMutation::StateTransition {
            inscription_id: pass.inscription_id.to_string(),
            from_state: MinerPassState::Active.as_str().into(),
            to_state: MinerPassState::Dormant.as_str().into(),
            owner: pass.owner.to_string(),
            satpoint: pass.satpoint.to_string(),
        });
        Ok(())
    }
}
