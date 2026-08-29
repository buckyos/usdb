mod cmd;
mod usdb_indexer_service;

use clap::Parser;
use cmd::Cli;
use usdb_indexer_service::UsdbIndexerService;

#[tokio::main]
async fn main() {
    let log_config = usdb_util::current_process_log_config!(usdb_util::USDB_INDEXER_CLI_TOOL_NAME)
        .enable_file(false)
        .enable_console(true);
    let _log_handle = usdb_util::init_log(log_config).unwrap_or_else(|error| {
        eprintln!("Failed to initialize usdb-indexer CLI logging: {error}");
        std::process::exit(1);
    });

    let cli = Cli::parse();
    let service = UsdbIndexerService::new(&cli.url)
        .await
        .map_err(|e| {
            let msg = format!("Failed to create USDB indexer client: {}", e);
            println!("{}", msg);
            std::process::exit(1);
        })
        .unwrap();

    if let Err(e) = service.process_command(cli).await {
        let msg = format!("Error processing command: {}", e);
        println!("{}", msg);
        std::process::exit(1);
    }
}
