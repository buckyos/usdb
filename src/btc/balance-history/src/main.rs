#![allow(dead_code)]

#[macro_use]
extern crate log;

use balance_history::config::BalanceHistoryConfig;
use balance_history::db::{self, BalanceHistoryDB};
use balance_history::index;
use balance_history::output::IndexOutput;
use balance_history::runtime::run_service;
use balance_history::{status, tool, web_server};
use clap::{Parser, Subcommand};
use std::path::PathBuf;
use std::sync::Arc;
use usdb_util::LogConfig;

#[derive(Parser, Debug)]
#[command(name = "balance-history")]
#[command(author = "buckyos")]
#[command(version = "0.1.0")]
#[command(about = "Bitcoin Balance History Indexer", long_about = None)]
#[command(long_about = None)]
struct BalanceHistoryCli {
    #[command(subcommand)]
    command: Option<BalanceHistoryCommands>,

    /// Override service root directory (default: ~/.usdb/balance-history)
    #[arg(long)]
    root_dir: Option<PathBuf>,

    /// Skip process lock acquisition (for isolated integration tests)
    #[arg(long, default_value_t = false)]
    skip_process_lock: bool,

    /// Run the service in daemon mode
    #[arg(short, long)]
    daemon: bool,

    /// Specify the maximum block height to index addresses up to, defaults to the latest block height
    #[arg(short, long)]
    max_block_height: Option<u32>,
}

use clap::Args;

#[derive(Args, Debug, Clone)]
#[group(required = true, multiple = false)]
struct InstallSnapshotSource {
    /// Specify the snapshot file to install, if the file is relative, it is relative to the service directory ${root}/snapshots/
    #[arg(short, long)]
    file: Option<String>,

    /// Specify the expected hash of the snapshot file for verification, which in directory ${root}/snapshots/snapshot_{block_height}.db
    #[arg(short, long)]
    block_height: Option<u32>,
}

#[derive(Args, Debug, Clone)]
struct SnapshotKeygenArgs {
    /// Logical signer identifier written into manifest.signing_key_id.
    #[arg(long)]
    key_id: String,

    /// Output directory for generated key files. Relative paths are resolved against root_dir.
    #[arg(long)]
    out_dir: Option<PathBuf>,

    /// Overwrite existing key files in the output directory.
    #[arg(long, default_value_t = false)]
    force: bool,
}

#[derive(Subcommand, Debug, Clone)]
#[command(rename_all = "kebab-case")]
enum BalanceHistoryCommands {
    /// Delete the database files, DANGEROUS: This will remove all indexed data!
    /// Use with caution.
    // #[command(alias = "c")]
    ClearDb {},

    IndexAddress {},

    /// Create a snapshot of the specified block height
    CreateSnapshot {
        /// Specify the target block height for the snapshot
        #[arg(short, long)]
        block_height: u32,

        /// Include UTXO data in the snapshot, default is true
        #[arg(short, long, default_value_t = true)]
        with_utxo: bool,
    },

    VerifySnapshot {},

    InstallSnapshot {
        #[clap(flatten)]
        source: InstallSnapshotSource,

        /// Optional sidecar manifest file describing the expected installed state.
        /// If omitted, the installer will look for `<snapshot>.manifest.json` next to the snapshot DB.
        #[arg(long)]
        manifest: Option<String>,
    },

    /// Generate one snapshot signing key and matching public-key export files.
    SnapshotKeygen {
        #[clap(flatten)]
        args: SnapshotKeygenArgs,
    },

    Verify {
        /// Specify the target address to verify
        #[arg(short, long)]
        address: Option<String>,

        /// Specify the target script hash to verify
        #[arg(short, long)]
        script_hash: Option<String>,

        /// Specify the target block height to verify. If omitted, verify against the current stable height in balance-history DB.
        /// #[arg(short, long)]
        height: Option<u32>,

        /// Specify the starting address or script hash to verify from
        #[arg(long, alias = "start")]
        from: Option<String>,
    },

    /// Serve balance-history browser static web files
    ServeWeb {
        /// HTTP listen port for the web server
        #[arg(long, default_value_t = 8098)]
        port: u16,

        /// Web root directory for static assets
        #[arg(long, default_value = "web/balance-history-browser")]
        web_root: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = BalanceHistoryCli::parse();
    let root_dir = cli
        .root_dir
        .clone()
        .unwrap_or_else(|| usdb_util::get_service_dir(usdb_util::BALANCE_HISTORY_SERVICE_NAME));

    match cli.command {
        Some(BalanceHistoryCommands::ClearDb {}) => {
            // Init file logging
            let file_name = format!("{}_clear_db", usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(true);
            usdb_util::init_log(config);

            println!("Will clear database files in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };

            if let Err(e) = tool::clear_db_files(&config.db_dir()) {
                error!("Failed to clear database files: {}", e);
                std::process::exit(1);
            }
            println!("Database files cleared successfully.");
            return;
        }
        Some(BalanceHistoryCommands::IndexAddress {}) => {
            // Init file logging
            let file_name = format!("{}_index_address", usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(false);
            usdb_util::init_log(config);

            println!("Indexing addresses in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);

            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            let address_index =
                index::AddressIndexer::new(&root_dir, config.clone(), output.clone()).unwrap();
            if let Err(e) = address_index.build_index() {
                output.eprintln(&format!("Failed to build address index: {}", e));
                std::process::exit(1);
            }

            println!("Address index built successfully.");
            return;
        }
        Some(BalanceHistoryCommands::CreateSnapshot {
            block_height,
            with_utxo,
        }) => {
            // Init file logging
            let file_name = format!("{}_snapshot", usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(false);
            usdb_util::init_log(config);

            println!("Generating snapshot in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            let db = match BalanceHistoryDB::open(
                config.clone(),
                db::BalanceHistoryDBMode::BestEffort,
            ) {
                Ok(database) => database,
                Err(e) => {
                    error!("Failed to initialize database: {}", e);
                    output.println(&format!("Failed to initialize database: {}", e));
                    std::process::exit(1);
                }
            };
            let db = Arc::new(db);

            let snapshot_indexer =
                index::SnapshotIndexer::new(config.clone(), db.clone(), output.clone());
            if let Err(e) = snapshot_indexer.run(block_height, with_utxo) {
                error!("Failed to generate snapshot: {}", e);
                output.println(&format!("Failed to generate snapshot: {}", e));
                std::process::exit(1);
            }

            println!("Snapshot generated successfully.");
            return;
        }
        Some(BalanceHistoryCommands::InstallSnapshot { source, manifest }) => {
            // Init file logging
            let file_name = format!(
                "{}_install_snapshot",
                usdb_util::BALANCE_HISTORY_SERVICE_NAME
            );
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(false);
            usdb_util::init_log(config);

            println!("Installing snapshot in directory: {:?}", root_dir);

            let file_path = if let Some(ref f) = source.file {
                let mut file_path = std::path::PathBuf::from(f);
                if file_path.is_relative() {
                    file_path = root_dir.clone();
                    file_path.push("snapshots");
                    file_path.push(&source.file.as_ref().unwrap());
                    println!("Resolved relative snapshot file path to: {:?}", file_path);
                }
                file_path
            } else if let Some(block_height) = source.block_height {
                let mut file_path = root_dir.clone();
                file_path.push("snapshots");
                file_path.push(format!("snapshot_{}.db", block_height));
                println!(
                    "Using snapshot file for block height {}: {:?}",
                    block_height, file_path
                );
                file_path
            } else {
                error!("No snapshot file or block height specified for installation.");
                println!("No snapshot file or block height specified for installation.");
                std::process::exit(1);
            };

            if !file_path.exists() {
                error!("Snapshot file does not exist: {:?}", file_path);
                println!("Snapshot file does not exist: {:?}", file_path);
                std::process::exit(1);
            }

            let manifest_path = if let Some(ref manifest) = manifest {
                let mut path = PathBuf::from(manifest);
                if path.is_relative() {
                    path = root_dir.join("snapshots").join(manifest);
                    println!("Resolved relative snapshot manifest path to: {:?}", path);
                }
                Some(path)
            } else {
                let auto_manifest = index::manifest_path_for_snapshot_file(&file_path);
                if auto_manifest.exists() {
                    println!(
                        "Using snapshot manifest discovered next to snapshot file: {:?}",
                        auto_manifest
                    );
                    Some(auto_manifest)
                } else {
                    None
                }
            };

            if let Some(ref manifest_path) = manifest_path {
                if !manifest_path.exists() {
                    error!("Snapshot manifest does not exist: {:?}", manifest_path);
                    println!("Snapshot manifest does not exist: {:?}", manifest_path);
                    std::process::exit(1);
                }
            }

            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            let db = match BalanceHistoryDB::open(
                config.clone(),
                db::BalanceHistoryDBMode::BestEffort,
            ) {
                Ok(database) => database,
                Err(e) => {
                    output.eprintln(&format!("Failed to initialize database: {}", e));
                    std::process::exit(1);
                }
            };
            let db = Arc::new(db);

            let data = index::SnapshotData {
                file: file_path.clone(),
                manifest_file: manifest_path,
            };
            let snapshot_installer =
                index::SnapshotInstaller::new(config.clone(), db, output.clone());
            if let Err(e) = snapshot_installer.install(data) {
                output.eprintln(&format!("Failed to install snapshot: {}", e));
                std::process::exit(1);
            }

            println!("Snapshot installed successfully.");
            return;
        }
        Some(BalanceHistoryCommands::SnapshotKeygen { args }) => {
            let file_name = format!(
                "{}_snapshot_keygen",
                usdb_util::BALANCE_HISTORY_SERVICE_NAME
            );
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(true);
            usdb_util::init_log(config);

            let out_dir = if let Some(path) = args.out_dir.as_ref() {
                if path.is_absolute() {
                    path.clone()
                } else {
                    root_dir.join(path)
                }
            } else {
                root_dir.clone()
            };

            let output = tool::generate_snapshot_key_files(&out_dir, &args.key_id, args.force)
                .unwrap_or_else(|e| {
                    error!("Failed to generate snapshot signing key files: {}", e);
                    println!("Failed to generate snapshot signing key files: {}", e);
                    std::process::exit(1);
                });

            println!("Snapshot signing key generated successfully.");
            println!("signing_key_file={}", output.signing_key_file.display());
            println!("public_key_file={}", output.public_key_file.display());
            println!("trusted_keys_file={}", output.trusted_keys_file.display());
            return;
        }
        Some(BalanceHistoryCommands::VerifySnapshot {}) => {
            // Init file logging
            let file_name = format!(
                "{}_verify_snapshot",
                usdb_util::BALANCE_HISTORY_SERVICE_NAME
            );
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(false);
            usdb_util::init_log(config);

            println!("Verifying snapshot in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            // Load Address DB
            let db = match db::AddressDB::new(&root_dir) {
                Ok(database) => database,
                Err(e) => {
                    output.eprintln(&format!("Failed to open address database: {}", e));
                    std::process::exit(1);
                }
            };
            let address_db = Arc::new(db);

            let block_height = 400_000; // Example block height
            let db = match db::SnapshotDB::open_by_height(&root_dir, block_height, false) {
                Ok(database) => database,
                Err(e) => {
                    output.eprintln(&format!("Failed to open snapshot database: {}", e));
                    std::process::exit(1);
                }
            };
            let snapshot_db = Arc::new(db);

            let electrs_client = match usdb_util::ElectrsClient::new(&config.electrs.rpc_url()) {
                Ok(client) => client,
                Err(e) => {
                    output.eprintln(&format!("Failed to create electrs client: {}", e));
                    std::process::exit(1);
                }
            };
            let electrs_client = Arc::new(electrs_client);

            let verifier = index::SnapshotVerifier::new(
                config.clone(),
                electrs_client,
                address_db,
                snapshot_db,
            );
            if let Err(e) = verifier.verify(1).await {
                output.eprintln(&format!("Failed to verify snapshot: {}", e));
                std::process::exit(1);
            }

            println!("Snapshot verified successfully.");
            return;
        }
        Some(BalanceHistoryCommands::Verify {
            address,
            script_hash,
            height,
            from,
        }) => {
            // Init file logging
            let file_name = format!("{}_verify", usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(true);
            usdb_util::init_log(config);

            println!("Verifying balance history in directory: {:?}", root_dir);
            let config = match BalanceHistoryConfig::load(&root_dir) {
                Ok(cfg) => cfg,
                Err(e) => {
                    error!("Failed to load config: {}", e);
                    println!("Failed to load config: {}", e);
                    std::process::exit(1);
                }
            };
            let config = Arc::new(config);
            let status = status::SyncStatusManager::new();
            let status = Arc::new(status);
            let output = IndexOutput::new(status);
            let output = Arc::new(output);

            // Load balance history DB
            let db = match BalanceHistoryDB::open_for_read(
                config.clone(),
                db::BalanceHistoryDBMode::BestEffort,
            ) {
                Ok(database) => database,
                Err(e) => {
                    output.eprintln(&format!("Failed to initialize database: {}", e));
                    std::process::exit(1);
                }
            };
            let db = Arc::new(db);

            let electrs_client = match usdb_util::ElectrsClient::new(&config.electrs.rpc_url()) {
                Ok(client) => client,
                Err(e) => {
                    output.eprintln(&format!("Failed to create electrs client: {}", e));
                    std::process::exit(1);
                }
            };
            let electrs_client = Arc::new(electrs_client);

            let verifier = index::BalanceHistoryVerifier::new(
                config.clone(),
                electrs_client,
                db,
                output.clone(),
            );

            let script_hash = if let Some(addr_str) = address {
                match usdb_util::address_string_to_script_hash(&addr_str, &config.btc.network) {
                    Ok(sh) => Some(sh),
                    Err(e) => {
                        output
                            .eprintln(&format!("Failed to convert address to script hash: {}", e));
                        std::process::exit(1);
                    }
                }
            } else if let Some(sh_str) = script_hash {
                match usdb_util::parse_script_hash(&sh_str) {
                    Ok(sh) => Some(sh),
                    Err(e) => {
                        output.eprintln(&format!("Failed to parse script hash: {}", e));
                        std::process::exit(1);
                    }
                }
            } else {
                None
            };

            let from = if let Some(from_str) = from {
                match usdb_util::parse_script_hash_any(&from_str, &config.btc.network) {
                    Ok(sh) => Some(sh),
                    Err(e) => {
                        output.eprintln(&format!(
                            "Failed to parse 'from' script hash or address: {}",
                            e
                        ));
                        std::process::exit(1);
                    }
                }
            } else {
                None
            };

            tokio::task::spawn_blocking(move || {
                if script_hash.is_some() {
                    if let Some(height) = height {
                        output.println(&format!(
                            "Verifying balance history for script_hash {} at height {}...",
                            script_hash.as_ref().unwrap(),
                            height
                        ));
                        if let Err(e) = verifier.verify_address_at_height(&script_hash.unwrap(), height) {
                            output.eprintln(&format!("Failed to verify balance history: {}", e));
                            std::process::exit(1);
                        }
                        println!("Balance history verified successfully for script_hash {} at height {}.", script_hash.as_ref().unwrap(), height);
                        return;
                    } else {
                        output.println(&format!(
                            "Verifying script_hash {} at current stable height...",
                            script_hash.unwrap()
                        ));
                        if let Err(e) = verifier.verify_address_latest(&script_hash.unwrap()) {
                            output.eprintln(&format!("Failed to verify balance history: {}", e));
                            std::process::exit(1);
                        }
                        println!(
                            "Balance history verified successfully for script_hash {} at current stable height.",
                            script_hash.as_ref().unwrap()
                        );
                    }
                } else {
                    if height.is_some() {
                        output.println(&format!(
                            "Verifying balance history at height {}...",
                            height.unwrap()
                        ));
                        if let Err(e) = verifier.verify_at_height(height.unwrap(), from) {
                            output.eprintln(&format!("Failed to verify balance history: {}", e));
                            std::process::exit(1);
                        }
                    } else {
                        output.println(
                            "Verifying entire balance history at current stable height...",
                        );
                        if let Err(e) = verifier.verify_latest(from) {
                            output.eprintln(&format!("Failed to verify balance history: {}", e));
                            std::process::exit(1);
                        }
                    }
                }
            })
            .await
            .unwrap();

            println!("Balance history verified successfully.");
            return;
        }
        Some(BalanceHistoryCommands::ServeWeb { port, web_root }) => {
            let file_name = format!("{}_web", usdb_util::BALANCE_HISTORY_SERVICE_NAME);
            let config = LogConfig::new(usdb_util::BALANCE_HISTORY_SERVICE_NAME)
                .with_service_root_dir(root_dir.clone())
                .with_file_name(&file_name)
                .enable_console(true);
            usdb_util::init_log(config);

            let web_root = std::path::PathBuf::from(web_root);
            if let Err(e) = web_server::serve_static_files(port, &web_root) {
                error!("Failed to start web server: {}", e);
                println!("Failed to start web server: {}", e);
                std::process::exit(1);
            }

            return;
        }
        None => {}
    }

    if cli.daemon {
        // Proceed to daemonize and run the main process
        tool::daemonize_process(usdb_util::BALANCE_HISTORY_SERVICE_NAME);
    }

    run_service(root_dir, cli.max_block_height, cli.skip_process_lock).await;
    println!("Balance History service exited.");
}
