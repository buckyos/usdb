#[cfg(any(test, feature = "miner-pass-conformance"))]
mod conformance;
mod content;
pub(crate) mod economic_rules;
mod effective_energy;
mod energy;
pub(crate) mod energy_formula;
mod energy_settlement;
mod indexer;
mod pass;
mod pass_commit;
mod rule_transition;
pub(crate) mod rules;
#[cfg(test)]
mod test;
mod transfer;

pub use content::*;
pub(crate) use effective_energy::*;
pub(crate) use energy_formula::*;
pub use indexer::*;
pub(crate) use pass_commit::*;

#[cfg(test)]
#[path = "../../../../../tests/assumeutxo_indexer_inputs.rs"]
mod assumeutxo_indexer_inputs;

#[cfg(test)]
#[path = "../../../../../tests/assumeutxo_service_fixture.rs"]
mod assumeutxo_service_fixture;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_evidence.rs"]
mod miner_pass_evidence;

#[cfg(test)]
#[path = "../../../../../tests/common/http_rpc.rs"]
pub(crate) mod test_http_rpc;

#[cfg(test)]
#[path = "../../../../../tests/common/miner_pass_evidence.rs"]
pub(crate) mod test_miner_evidence;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_eligibility.rs"]
mod miner_pass_eligibility;

#[cfg(test)]
#[path = "../../../../../tests/common/miner_pass_state.rs"]
pub(crate) mod test_miner_state;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_activation.rs"]
mod miner_pass_activation;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_startup.rs"]
mod miner_pass_startup;

#[cfg(test)]
#[path = "../../../../../tests/common/miner_pass_rules.rs"]
pub(crate) mod test_miner_rules;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_upgrade_dispatch.rs"]
mod miner_pass_upgrade_dispatch;

#[cfg(test)]
#[path = "../../../../../tests/common/miner_pass_pipeline.rs"]
pub(crate) mod test_miner_pipeline;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_upgrade_energy.rs"]
mod miner_pass_upgrade_energy;

#[cfg(test)]
#[path = "../../../../../tests/common/miner_pass_energy_reference.rs"]
pub(crate) mod test_energy_reference;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_upgrade_recovery.rs"]
mod miner_pass_upgrade_recovery;

#[cfg(test)]
#[path = "../../../../../tests/common/miner_pass_upgrade.rs"]
pub(crate) mod test_miner_upgrade;

#[cfg(test)]
#[path = "../../../../../tests/miner_pass_upgrade_queries.rs"]
mod miner_pass_upgrade_queries;
#[cfg(test)]
#[path = "../../../../../tests/common/miner_pass_queries.rs"]
pub(crate) mod test_miner_queries;
