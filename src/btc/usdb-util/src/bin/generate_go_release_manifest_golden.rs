use std::env;
use std::error::Error;
use std::fs;
use std::io::{Error as IoError, ErrorKind};
use usdb_util::embedded_cross_chain_release_manifest;

fn main() -> Result<(), Box<dyn Error>> {
    let manifest = embedded_cross_chain_release_manifest()?;
    let output = format!("{}\n", serde_json::to_string_pretty(manifest)?);

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
                        "generated Go release manifest artifact differs from {}",
                        path.to_string_lossy()
                    ),
                )
                .into());
            }
        }
        _ => {
            return Err(IoError::new(
                ErrorKind::InvalidInput,
                "usage: generate_go_release_manifest_golden [--check] [output-path]",
            )
            .into());
        }
    }
    Ok(())
}
