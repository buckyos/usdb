//! Compiled schema and state-machine executors selected by the BTC event context.
use bitcoincore_rpc::bitcoin::Network;
use usdb_util::{
    ActivationRegistryError, ActiveVersionSet, BtcRuleContext, INSCRIPTION_SCHEMA_VERSION_V1,
    PASS_STATE_MACHINE_VERSION_V2, VersionFamily,
};

/// Payload contracts are independent of the state-machine version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MintSchemaRules {
    V1,
    #[cfg(any(test, feature = "miner-pass-conformance"))]
    Conformance901,
    #[cfg(any(test, feature = "miner-pass-conformance"))]
    Conformance902,
}

impl MintSchemaRules {
    /// Resolve only an explicitly compiled schema, never the highest known version.
    pub(crate) fn at(rules: &BtcRuleContext) -> Result<Self, String> {
        Self::from_versions(rules.active_version_set()).map_err(|error| context_error(rules, error))
    }

    pub(crate) fn from_versions(
        versions: &ActiveVersionSet,
    ) -> Result<Self, ActivationRegistryError> {
        let family = VersionFamily::InscriptionSchemaVersion;
        let value = versions.require_string(family)?;
        match value {
            INSCRIPTION_SCHEMA_VERSION_V1 => Ok(Self::V1),
            #[cfg(any(test, feature = "miner-pass-conformance"))]
            CONFORMANCE_SCHEMA if conformance_scope(versions) => Ok(Self::Conformance901),
            #[cfg(any(test, feature = "miner-pass-conformance"))]
            CONFORMANCE_SCHEMA_STRUCTURED if conformance_scope(versions) => {
                Ok(Self::Conformance902)
            }
            _ => Err(ActivationRegistryError::VersionNotSupported {
                family,
                value: value.into(),
            }),
        }
    }

    /// Frozen wire version accepted by this contract; unrelated to fixture defaults.
    pub(crate) fn payload_version(self) -> u32 {
        match self {
            Self::V1 => 1,
            #[cfg(any(test, feature = "miner-pass-conformance"))]
            Self::Conformance901 => 901,
            #[cfg(any(test, feature = "miner-pass-conformance"))]
            Self::Conformance902 => 902,
        }
    }
}

/// State-machine executors operate on accepted state, without rechecking old mint schemas.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PassStateRules {
    V2,
    #[cfg(any(test, feature = "miner-pass-conformance"))]
    ConformanceNoNewCollab,
}

impl PassStateRules {
    /// Select the state-machine contract at the operation height.
    pub(crate) fn at(rules: &BtcRuleContext) -> Result<Self, String> {
        Self::from_versions(rules.active_version_set()).map_err(|error| context_error(rules, error))
    }

    pub(crate) fn from_versions(
        versions: &ActiveVersionSet,
    ) -> Result<Self, ActivationRegistryError> {
        let family = VersionFamily::PassStateMachineVersion;
        let value = versions.require_string(family)?;
        match value {
            PASS_STATE_MACHINE_VERSION_V2 => Ok(Self::V2),
            #[cfg(any(test, feature = "miner-pass-conformance"))]
            CONFORMANCE_STATE if conformance_scope(versions) => Ok(Self::ConformanceNoNewCollab),
            _ => Err(ActivationRegistryError::VersionNotSupported {
                family,
                value: value.into(),
            }),
        }
    }
}

/// Central capability gate used for startup, block execution and query version selection.
/// Normal binaries register only the existing schema-v1/state-v2 implementation.
pub(crate) fn validate_indexer_rules(
    versions: &ActiveVersionSet,
) -> Result<(), ActivationRegistryError> {
    #[cfg(any(test, feature = "miner-pass-conformance"))]
    if conformance_scope(versions) {
        return versions.validate_btc_indexer_with_formula_executors(
            |_, _, _| {
                MintSchemaRules::from_versions(versions)?;
                PassStateRules::from_versions(versions)?;
                super::energy_settlement::EnergyRules::from_versions(versions)?;
                Ok(())
            },
            |_, _| super::economic_rules::EconomicRules::from_versions(versions).map(|_| ()),
        );
    }
    versions.validate_btc_indexer()?;
    MintSchemaRules::from_versions(versions)?;
    PassStateRules::from_versions(versions)?;
    Ok(())
}

/// Refuse cross-height or cross-network evidence before any classification or state mutation.
pub(crate) fn require_event_context(
    rules: &BtcRuleContext,
    height: u32,
    network: Network,
) -> Result<(), String> {
    rules
        .scope()
        .validate_network(network)
        .map_err(|error| context_error(rules, error))?;
    if height != rules.btc_height() {
        let msg = format!(
            "MinerPass rule context height mismatch: event_height={height}, context_height={}, registry_id={}, network_id={}, rules_scope={}",
            rules.btc_height(),
            rules.activation_registry_id(),
            rules.scope().network_id,
            rules.scope().rules_scope()
        );
        error!("{msg}");
        return Err(msg);
    }
    Ok(())
}

fn context_error(rules: &BtcRuleContext, error: ActivationRegistryError) -> String {
    let msg = format!(
        "Failed to select MinerPass executor: block_height={}, registry_id={}, network_id={}, rules_scope={}, error={error}",
        rules.btc_height(),
        rules.activation_registry_id(),
        rules.scope().network_id,
        rules.scope().rules_scope()
    );
    error!("{msg}");
    msg
}

// Test executors are absent from ordinary binaries and restricted even in test builds.
#[cfg(any(test, feature = "miner-pass-conformance"))]
pub(crate) use crate::index::conformance::{
    CONFORMANCE_SCHEMA, CONFORMANCE_SCHEMA_STRUCTURED, CONFORMANCE_SCOPE, CONFORMANCE_STATE,
};
#[cfg(any(test, feature = "miner-pass-conformance"))]
pub(crate) fn conformance_scope(versions: &ActiveVersionSet) -> bool {
    versions.scope().is_some_and(|scope| {
        scope.network_id == "btc-regtest" && scope.rules_scope == CONFORMANCE_SCOPE
    })
}
