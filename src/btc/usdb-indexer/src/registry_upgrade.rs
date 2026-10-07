//! Offline preflight/apply used by the stopped-node upgrade transaction.
use crate::storage::{MinerPassStorage, PassEnergyStorage, rules_upgrade};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, path::PathBuf};
use usdb_util::{BtcActivationRegistryCatalog, BtcRuleTimeline, IndexerRulesBinding};

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Inspect existing databases, or apply exactly a saved registry-upgrade preflight.
    RegistryUpgrade(RegistryUpgrade),
}

#[derive(Debug, Args)]
pub(crate) struct RegistryUpgrade {
    /// Existing directory containing miner_pass.db and energy/ (not the service root).
    #[arg(long)]
    data_dir: PathBuf,
    #[arg(long)]
    catalog: PathBuf,
    /// Complete target IndexerRulesBinding JSON, checked against the catalog.
    #[arg(long)]
    target_binding: PathBuf,
    /// Mutate metadata only after comparing the supplied preflight with both databases.
    #[arg(long, requires = "expected_report")]
    apply: bool,
    #[arg(long, requires = "apply")]
    expected_report: Option<PathBuf>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Report {
    schema_version: String,
    adoption: rules_upgrade::Adoption,
}

impl RegistryUpgrade {
    pub(crate) fn run(&self) -> Result<String, String> {
        let read = |path: &PathBuf| {
            std::fs::read_to_string(path)
                .map_err(|e| format!("Cannot read {}: {e}", path.display()))
        };
        let catalog = BtcActivationRegistryCatalog::from_json(&read(&self.catalog)?)
            .map_err(|e| e.to_string())?;
        let target: IndexerRulesBinding =
            usdb_util::parse_json_strict(&read(&self.target_binding)?)
                .map_err(|e| format!("Invalid target binding: {e}"))?;
        if target
            != IndexerRulesBinding::new(catalog.current_registry(), target.index_origin_height)
        {
            return Err(format!(
                "Target binding differs from catalog: target={target:?}"
            ));
        }
        let timelines: HashMap<_, _> = catalog
            .registry_ids()
            .iter()
            .map(|id| {
                Ok((
                    id.clone(),
                    BtcRuleTimeline::new_with_indexer_support(
                        catalog.registry_by_id(id)?,
                        crate::index::rules::validate_indexer_rules,
                    )?,
                ))
            })
            .collect::<Result<_, usdb_util::ActivationRegistryError>>()
            .map_err(|e| e.to_string())?;
        let expected = self
            .expected_report
            .as_ref()
            .map(|p| {
                usdb_util::parse_json_strict::<Report>(&read(p)?)
                    .map_err(|e| format!("Invalid saved registry preflight: {e}"))
            })
            .transpose()?;
        // Always inspect existing databases before obtaining writable handles. Missing or
        // corrupt data must never be initialized as an apparently successful adoption.
        let check = |pass: &MinerPassStorage,
                     energy: &PassEnergyStorage|
         -> Result<Report, String> {
            let intent =
                rules_upgrade::inspect_registry(pass, energy, &catalog, &timelines, &target)?;
            let report = match intent {
                Some(adoption) => Report {
                    schema_version: "usdb-indexer-registry-preflight:v1".into(),
                    adoption,
                },
                None => {
                    let adoption = if let Some(expected) = &expected {
                        if expected.adoption.target != target {
                            return Err("Saved preflight targets another registry".into());
                        }
                        rules_upgrade::verify_completed(pass, energy, &expected.adoption)?;
                        expected.adoption.clone()
                    } else {
                        rules_upgrade::inspect_current(
                            pass,
                            energy,
                            &target,
                            &timelines[&target.activation_registry_id],
                        )?
                    };
                    Report {
                        schema_version: "usdb-indexer-registry-preflight:v1".into(),
                        adoption,
                    }
                }
            };
            if expected.as_ref().is_some_and(|saved| saved != &report) {
                return Err(format!(
                    "Registry boundary changed since preflight: saved={expected:?}, actual={report:?}"
                ));
            }
            Ok(report)
        };
        let report = {
            let pass = MinerPassStorage::open_read_only(&self.data_dir)?;
            let energy = PassEnergyStorage::open_read_only(&self.data_dir)?;
            check(&pass, &energy)?
        };
        if self.apply {
            let pass = MinerPassStorage::open_existing(&self.data_dir)?;
            let energy = PassEnergyStorage::open_existing(&self.data_dir)?;
            check(&pass, &energy)?;
            rules_upgrade::adopt_registry(&pass, &energy, &catalog, &timelines, &target, |_| {
                Ok(())
            })?;
            rules_upgrade::verify_completed(&pass, &energy, &report.adoption)?;
        }
        serde_json::to_string(&report).map_err(|e| e.to_string())
    }
}
