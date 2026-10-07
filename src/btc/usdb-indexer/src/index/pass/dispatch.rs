//! Lifecycle dispatch uses event-height rules, never the stored mint payload version.
use ord::InscriptionId;
use ordinals::SatPoint;
use usdb_util::{BtcRuleContext, BtcScriptHash};

use super::{InvalidPassMintInscriptionInfo, MinerPassManager};
use crate::index::rules::{PassStateRules, require_event_context};

impl MinerPassManager {
    /// Bind state operations to this manager's configured rule domain before writing.
    pub(super) fn require_rule_context(
        &self,
        height: u32,
        rules: &BtcRuleContext,
    ) -> Result<(), String> {
        let config = &self.config.config().usdb;
        require_event_context(rules, height, self.config.config().bitcoin.network())?;
        let expected_scope = config
            .rules_scope
            .as_deref()
            .unwrap_or(usdb_util::LEGACY_RULES_SCOPE);
        if rules.scope().rules_scope() != expected_scope
            || config
                .activation_registry_id
                .as_deref()
                .is_some_and(|id| id != rules.activation_registry_id())
        {
            let msg = format!(
                "MinerPass event rule domain mismatch: block_height={height}, expected_scope={expected_scope}, actual_scope={}, expected_registry_id={:?}, actual_registry_id={}",
                rules.scope().rules_scope(),
                config.activation_registry_id,
                rules.activation_registry_id()
            );
            error!("{msg}");
            return Err(msg);
        }
        Ok(())
    }

    // Both currently compiled contracts retain the v2 lifecycle of accepted passes.
    // Future lifecycle changes must add an explicit route rather than a fallback.
    fn require_v2_lifecycle(&self, height: u32, rules: &BtcRuleContext) -> Result<(), String> {
        self.require_rule_context(height, rules)?;
        match PassStateRules::at(rules)? {
            PassStateRules::V2 => Ok(()),
            #[cfg(any(test, feature = "miner-pass-conformance"))]
            PassStateRules::ConformanceNoNewCollab => Ok(()),
        }
    }

    /// Route a transfer of an accepted pass under current event rules.
    pub(crate) async fn on_pass_transfer_with_rules(
        &self,
        inscription_id: &InscriptionId,
        new_owner: &BtcScriptHash,
        satpoint: &SatPoint,
        height: u32,
        rules: &BtcRuleContext,
    ) -> Result<(), String> {
        self.require_v2_lifecycle(height, rules)?;
        self.on_pass_transfer(inscription_id, new_owner, satpoint, height)
            .await
    }

    /// Route burn handling without reconsidering the pass's historical mint admission.
    pub(crate) async fn on_pass_burned_with_rules(
        &self,
        inscription_id: &InscriptionId,
        height: u32,
        rules: &BtcRuleContext,
    ) -> Result<(), String> {
        self.require_v2_lifecycle(height, rules)?;
        self.on_pass_burned(inscription_id, height).await
    }

    /// Record parser rejection under the selected state representation.
    pub(crate) async fn on_invalid_mint_pass_with_rules(
        &self,
        mint: &InvalidPassMintInscriptionInfo,
        rules: &BtcRuleContext,
    ) -> Result<(), String> {
        self.require_v2_lifecycle(mint.mint_block_height, rules)?;
        self.on_invalid_mint_pass(mint).await
    }
}
