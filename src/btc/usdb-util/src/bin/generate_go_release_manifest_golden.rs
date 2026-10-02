use std::env;
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use usdb_util::{
    BtcActivationRegistryCatalog, CrossChainReleaseManifest, embedded_cross_chain_release_manifest,
};

/// Generates the frozen default manifest or an explicitly supplied audit fixture.
fn main() -> Result<(), Box<dyn Error>> {
    let args = env::args_os().skip(1).collect::<Vec<_>>();
    if args.len() == 1 && (args[0] == "--help" || args[0] == "-h") {
        println!("{}", usage_error());
        return Ok(());
    }
    let mut manifest_path = None;
    let mut catalog_paths = Vec::new();
    let mut output_path = None;
    let mut check = false;
    let mut index = 0;
    while index < args.len() {
        if args[index] == "--manifest" {
            index += 1;
            let path = args.get(index).ok_or_else(usage_error)?.clone();
            if manifest_path.replace(path).is_some() {
                return Err(usage_error().into());
            }
        } else if args[index] == "--catalog" {
            index += 1;
            catalog_paths.push(args.get(index).ok_or_else(usage_error)?.clone());
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
    if (check && output_path.is_none()) || (manifest_path.is_none() && !catalog_paths.is_empty()) {
        return Err(usage_error().into());
    }
    let manifest = match manifest_path {
        Some(path) => CrossChainReleaseManifest::from_json(&fs::read_to_string(path)?)?,
        None => embedded_cross_chain_release_manifest()?.clone(),
    };
    if !catalog_paths.is_empty() {
        let catalogs = catalog_paths
            .into_iter()
            .map(|path| {
                Ok(BtcActivationRegistryCatalog::from_json(
                    &fs::read_to_string(path)?,
                )?)
            })
            .collect::<Result<Vec<_>, Box<dyn Error>>>()?;
        manifest.validate_btc_catalog_bindings(&catalogs)?;
    }
    let output = format!("{}\n", serde_json::to_string_pretty(&manifest)?);
    match output_path {
        None => print!("{}", output),
        Some(path) if check => {
            if fs::read_to_string(&path)? != output {
                return Err(IoError::new(
                    ErrorKind::InvalidData,
                    format!(
                        "generated Go release manifest artifact differs from {}",
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
        "usage: generate_go_release_manifest_golden [--manifest path] [--catalog path]... [--check] [output-path]",
    )
}
