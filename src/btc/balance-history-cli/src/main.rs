mod balance_history_service;
mod cmd;

use balance_history_service::BalanceHistoryService;
use clap::Parser;
use cmd::Cli;
use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let log_config = usdb_util::current_cli_log_config!(usdb_util::BALANCE_HISTORY_CLI_TOOL_NAME);
    let log_handle = usdb_util::init_log(log_config).unwrap_or_else(|error| {
        eprintln!("Failed to initialize balance-history CLI logging: {error}");
        std::process::exit(1);
    });

    let result = async {
        let service = BalanceHistoryService::new(&cli.url).await?;
        service.process_command(cli).await
    }
    .await;

    let exit_code = match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            log::error!("Balance-history CLI command failed: {error}");
            ExitCode::FAILURE
        }
    };
    log_handle.shutdown();
    exit_code
}
