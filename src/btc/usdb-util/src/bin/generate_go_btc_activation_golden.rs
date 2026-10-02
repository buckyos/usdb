use bitcoincore_rpc::bitcoin::Network;
use serde::Serialize;
use std::collections::BTreeSet;
use std::env;
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use usdb_util::{
    ACTIVATION_REGISTRY_SCHEMA_VERSION, ActivationStatus, ActiveVersionSet, BtcActivationRegistry,
    BtcActivationRegistryCatalog, SCOPED_ACTIVATION_REGISTRY_SCHEMA_VERSION,
    embedded_btc_activation_registry_catalog,
};

const GO_GOLDEN_SCHEMA_VERSION: &str = "uip-0008-go-btc-activation-golden:v3";
const SCOPED_GO_GOLDEN_SCHEMA_VERSION: &str = "uip-0008-go-btc-activation-golden:v4";

#[derive(Serialize)]
struct GoActivationGoldenArtifact {
    schema_version: &'static str,
    source_registry_schema_version: &'static str,
    registries: Vec<GoRegistryGolden>,
}

#[derive(Serialize)]
struct GoRegistryGolden {
    network_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    rules_scope: Option<String>,
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
    network_id: &str,
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
        network_id: network_id.to_string(),
        rules_scope: registry.scope.rules_scope.clone(),
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

/// Parses catalogs only on explicit request; the default artifact stays byte compatible.
fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{}", usage_error());
        return Ok(());
    }
    let mut catalogs = Vec::new();
    let mut output_path = None;
    let mut check = false;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--catalog" {
            index += 1;
            catalogs.push(args.get(index).ok_or_else(usage_error)?.clone());
        } else if args[index] == "--check" {
            if check {
                return Err(usage_error().into());
            }
            check = true;
        } else if args[index].to_string_lossy().starts_with('-')
            || output_path.replace(args[index].clone()).is_some()
        {
            return Err(usage_error().into());
        }
        index += 1;
    }
    if check && output_path.is_none() {
        return Err(usage_error().into());
    }

    let scoped = !catalogs.is_empty();
    let registries = if scoped {
        let mut registries = Vec::new();
        let mut scopes = BTreeSet::new();
        for path in catalogs {
            let catalog = BtcActivationRegistryCatalog::from_json(&fs::read_to_string(path)?)?;
            let scope = &catalog.current_registry().scope;
            if !scopes.insert((scope.network_id.clone(), scope.rules_scope.clone())) {
                return Err(
                    IoError::new(ErrorKind::InvalidData, "duplicate scoped catalog").into(),
                );
            }
            for (index, id) in catalog.registry_ids().iter().enumerate() {
                let registry = catalog.registry_by_id(id)?;
                if registry.schema_version != SCOPED_ACTIVATION_REGISTRY_SCHEMA_VERSION {
                    return Err(IoError::new(
                        ErrorKind::InvalidData,
                        "external golden catalogs require registry schema v3",
                    )
                    .into());
                }
                registries.push(registry_golden(
                    registry,
                    &scope.network_id,
                    u32::try_from(index + 1)?,
                    id == catalog.current_registry_id(),
                )?);
            }
        }
        registries
    } else {
        let mut registries = registry_goldens(Network::Bitcoin, "btc-mainnet")?;
        registries.extend(registry_goldens(Network::Regtest, "btc-regtest")?);
        registries
    };
    let artifact = GoActivationGoldenArtifact {
        schema_version: if scoped {
            SCOPED_GO_GOLDEN_SCHEMA_VERSION
        } else {
            GO_GOLDEN_SCHEMA_VERSION
        },
        source_registry_schema_version: if scoped {
            SCOPED_ACTIVATION_REGISTRY_SCHEMA_VERSION
        } else {
            ACTIVATION_REGISTRY_SCHEMA_VERSION
        },
        registries,
    };
    let output = format!("{}\n", serde_json::to_string_pretty(&artifact)?);
    match output_path {
        None => print!("{}", output),
        Some(path) if check => {
            let existing = fs::read_to_string(&path)?;
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
        Some(path) => fs::write(path, output)?,
    }
    Ok(())
}

/// Returns one stable usage error for malformed command-line arguments.
fn usage_error() -> IoError {
    IoError::new(
        ErrorKind::InvalidInput,
        "usage: generate_go_btc_activation_golden [--catalog catalog.json]... [--check] [output-path]",
    )
}
