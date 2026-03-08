mod cmd;
mod usdb_indexer_service;

use clap::Parser;
use cmd::Cli;
use usdb_indexer_service::UsdbIndexerService;

#[tokio::main]
async fn main() {
    let log_config = usdb_util::LogConfig::new(usdb_util::USDB_INDEXER_CLI_TOOL_NAME)
        .enable_file(false)
        .enable_console(true);
    usdb_util::init_log(log_config);

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
