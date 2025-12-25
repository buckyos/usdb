mod client;
mod cmd;
mod balance_history_service;


use cmd::Cli;
use clap::{Parser};
use balance_history_service::BalanceHistoryService;

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let service = BalanceHistoryService::new(&cli.url).await.map_err(|e| {
        let msg = format!("Failed to create Balance History Service client: {}", e);
        println!("{}", msg);
        std::process::exit(1);
    }).unwrap();

    if let Err(e) = service.process_command(cli).await {
        let msg = format!("Error processing command: {}", e);
        println!("{}", msg);
        std::process::exit(1);
    }
}
