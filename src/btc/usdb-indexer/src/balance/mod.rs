
use std::collections::HashMap;
use usdb_util::{BtcClient, USDBScriptHash};

pub struct BalanceMonitor {
    active_addresses: HashMap<USDBScriptHash, u64>,
    btc_client: BtcClient,
}