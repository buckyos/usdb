
use bitcoincore_rpc::bitcoin::address::{NetworkChecked, Address};
use electrum_client::{Client, ElectrumApi, GetHistoryRes};

pub struct ElectrsClient {
    client: Client,
    server_url: String,
}

impl ElectrsClient {
    pub fn new(server_url: &str) -> Result<Self, String> {
        let client = Client::new(server_url).map_err(|e| {
            let msg = format!("Failed to create Electrs client: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self {
            client,
            server_url: server_url.to_string(),
        })
    }

    // Get address history
    pub async fn get_history(&self, address: &Address<NetworkChecked>) -> Result<Vec<GetHistoryRes>, String> {
        
        let his = self.client.script_get_history(&address.script_pubkey())
            .map_err(|e| {
                let msg = format!("Failed to get history for address {}: {}", address, e);
                error!("{}", msg);
                msg
            })?;

        Ok(his)
    }
}

pub type ElectrsClientRef = std::sync::Arc<ElectrsClient>;