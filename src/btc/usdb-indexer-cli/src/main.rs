mod cmd;
mod usdb_indexer_service;

use clap::Parser;
use cmd::Cli;
use std::process::ExitCode;
use usdb_indexer_service::UsdbIndexerService;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let log_config = usdb_util::current_cli_log_config!(usdb_util::USDB_INDEXER_CLI_TOOL_NAME);
    let log_handle = usdb_util::init_log(log_config).unwrap_or_else(|error| {
        eprintln!("Failed to initialize usdb-indexer CLI logging: {error}");
        std::process::exit(1);
    });

    let result = async {
        let service = UsdbIndexerService::new(&cli.url).await?;
        service.process_command(cli).await
    }
    .await;

    let exit_code = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("USDB indexer CLI command failed: {error}");
            ExitCode::FAILURE
        }
    };
    log_handle.shutdown();
    exit_code
}
