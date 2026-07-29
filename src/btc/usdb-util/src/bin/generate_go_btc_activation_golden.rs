use bitcoincore_rpc::bitcoin::Network;
use serde::Serialize;
use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use usdb_util::{
    ACTIVATION_REGISTRY_SCHEMA_VERSION, ActivationStatus, ActiveVersionSet, BtcActivationRegistry,
    embedded_btc_activation_registry_catalog,
};

const GO_GOLDEN_SCHEMA_VERSION: &str = "uip-0008-go-btc-activation-golden:v3";

#[derive(Serialize)]
struct GoActivationGoldenArtifact {
    schema_version: &'static str,
    source_registry_schema_version: &'static str,
    registries: Vec<GoRegistryGolden>,
}

#[derive(Serialize)]
struct GoRegistryGolden {
    network_id: &'static str,
    revision: u32,
    current: bool,
    stable_lag_blocks: u32,
    activation_registry_id: String,
    activations: Vec<GoActivationGolden>,
}

#[derive(Serialize)]
struct GoActivationGolden {
    btc_height: u32,
    active_version_set: ActiveVersionSet,
    active_version_set_id: String,
}

fn registry_goldens(
    network: Network,
    network_id: &'static str,
) -> Result<Vec<GoRegistryGolden>, Box<dyn Error>> {
    let catalog = embedded_btc_activation_registry_catalog(network)?;
    catalog
        .registry_ids()
        .iter()
        .enumerate()
        .map(|(index, registry_id)| {
            registry_golden(
                catalog.registry_by_id(registry_id)?,
                network_id,
                u32::try_from(index + 1)?,
                registry_id == catalog.current_registry_id(),
            )
        })
        .collect()
}

fn registry_golden(
    registry: &BtcActivationRegistry,
    network_id: &'static str,
    revision: u32,
    current: bool,
) -> Result<GoRegistryGolden, Box<dyn Error>> {
    let heights = active_heights(registry)?;
    let activations = heights
        .into_iter()
        .map(|btc_height| {
            let active_version_set = registry.lookup_active_version_set(btc_height)?;
            let active_version_set_id = active_version_set.active_version_set_id();
            Ok(GoActivationGolden {
                btc_height,
                active_version_set,
                active_version_set_id,
            })
        })
        .collect::<Result<Vec<_>, Box<dyn Error>>>()?;

    Ok(GoRegistryGolden {
        network_id,
        revision,
        current,
        stable_lag_blocks: registry.stable_lag_blocks(),
        activation_registry_id: registry.activation_registry_id(),
        activations,
    })
}

fn active_heights(registry: &BtcActivationRegistry) -> Result<Vec<u32>, Box<dyn Error>> {
    let mut heights = BTreeSet::new();
    for record in &registry.records {
        if record.status == ActivationStatus::Active {
            heights.insert(u32::try_from(record.activation_height)?);
        }
    }
    Ok(heights.into_iter().collect())
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut registries = registry_goldens(Network::Bitcoin, "btc-mainnet")?;
    registries.extend(registry_goldens(Network::Regtest, "btc-regtest")?);
    let artifact = GoActivationGoldenArtifact {
        schema_version: GO_GOLDEN_SCHEMA_VERSION,
        source_registry_schema_version: ACTIVATION_REGISTRY_SCHEMA_VERSION,
        registries,
    };
    let output = format!("{}\n", serde_json::to_string_pretty(&artifact)?);

    let args = env::args_os().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [] => print!("{}", output),
        [path] => fs::write(path, output)?,
        [flag, path] if flag == "--check" => {
            let existing = fs::read_to_string(path)?;
            if existing != output {
                return Err(IoError::new(
                    ErrorKind::InvalidData,
                    format!(
                        "generated Go activation artifact differs from {}",
                        path.to_string_lossy()
                    ),
                )
                .into());
            }
        }
        _ => {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "usage: generate_go_btc_activation_golden [--check] [output-path]",
            )
            .into());
        }
    }
    Ok(())
}
