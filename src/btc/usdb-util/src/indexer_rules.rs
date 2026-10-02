//! Immutable data identity shared by the indexer and offline checkpoint validation.
use serde::{Deserialize, Serialize};

/// Metadata key persisted in both the pass SQLite store and energy RocksDB store.
pub const INDEXER_RULES_BINDING_KEY: &str = "indexer_rules_binding";

/// The complete rule and source identity that may write one indexer dataset.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IndexerRulesBinding {
    /// Version of this local persistent identity format, independent of economic rules.
    pub schema_version: String,
    /// Actual Bitcoin source network; never replaced by a USDB network label.
    pub btc_network_id: String,
    /// USDB interpretation domain, or `legacy` for the original v2 registries.
    pub rules_scope: String,
    /// First Bitcoin height included in this derived dataset.
    pub index_origin_height: u32,
    /// Exact configured revision; switching datasets is explicit in this first implementation.
    pub activation_registry_id: String,
}

impl IndexerRulesBinding {
    /// Bind the selected catalog's current revision and the index origin.
    pub fn new(registry: &crate::BtcActivationRegistry, index_origin_height: u32) -> Self {
        Self {
            schema_version: "usdb-indexer-rules-binding:v1".to_string(),
            btc_network_id: registry.scope.network_id.clone(),
            rules_scope: registry.scope.rules_scope().to_string(),
            index_origin_height,
            activation_registry_id: registry.activation_registry_id(),
        }
    }

    /// Verify stored metadata before writing or restoring a dataset.
    /// Unbound historical data is accepted only for the frozen legacy rule domain.
    pub fn validate_stored(&self, stored: Option<&str>, has_data: bool) -> Result<(), String> {
        let result = match stored {
            Some(json) => serde_json::from_str::<Self>(json)
                .map_err(|error| format!("Invalid stored indexer rules binding: {error}"))
                .and_then(|actual| {
                    if actual == *self {
                        Ok(())
                    } else {
                        Err(format!("Indexer rules binding mismatch: expected={self:?}, actual={actual:?}; use a separate dataset"))
                    }
                }),
            None if has_data && self.rules_scope != "legacy" => Err(format!(
                "Unbound nonempty indexer dataset cannot be adopted by rules_scope={}; rebuild in a separate dataset",
                self.rules_scope
            )),
            None => Ok(()),
        };
        result.inspect_err(|error| error!("{error}"))
    }

    /// Encode the exact identity persisted in both stores and copied into checkpoints.
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|error| {
            let msg = format!("Failed to serialize indexer rules binding: {error}");
            error!("{msg}");
            msg
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dataset_binding_rejects_cross_scope_source_origin_and_revision() {
        let registry =
            crate::embedded_btc_activation_registry(bitcoincore_rpc::bitcoin::Network::Bitcoin)
                .unwrap();
        let expected = IndexerRulesBinding::new(registry, 100);
        let stored = expected.to_json().unwrap();
        assert!(expected.validate_stored(Some(&stored), true).is_ok());
        for field in [
            "rules_scope",
            "btc_network_id",
            "index_origin_height",
            "activation_registry_id",
        ] {
            let mut changed = serde_json::to_value(&expected).unwrap();
            changed[field] = if field == "index_origin_height" {
                101.into()
            } else {
                "another".into()
            };
            assert!(
                expected
                    .validate_stored(Some(&changed.to_string()), true)
                    .is_err(),
                "{field}"
            );
        }
        assert!(expected.validate_stored(None, true).is_ok());
        let mut scoped = expected;
        scoped.rules_scope = "isolated-test".to_string();
        assert!(scoped.validate_stored(None, true).is_err());
        assert!(scoped.validate_stored(None, false).is_ok());
        assert!(scoped.validate_stored(Some("{}"), false).is_err());
    }
}
