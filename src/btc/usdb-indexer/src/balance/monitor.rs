use crate::config::ConfigManagerRef;
use balance_history::RpcClient as BalanceHistoryRpcClient;
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use usdb_util::{BTCRpcClient, ToUSDBScriptHash, USDBScriptHash};

pub struct BalanceMonitor {
    active_addresses: Mutex<HashMap<USDBScriptHash, u64>>,
    btc_client: BTCRpcClient,
    balance_history_client: BalanceHistoryRpcClient,
}

impl BalanceMonitor {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        // Init btc client
        let btc_client = BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )?;

        let balance_history_client = BalanceHistoryRpcClient::new(
            &config.config().balance_history.rpc_url,
        )
        .map_err(|e| {
            let msg = format!("Failed to create BalanceHistoryRpcClient: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self {
            active_addresses: Mutex::new(HashMap::new()),
            btc_client,
            balance_history_client,
        })
    }

    pub async fn monitor(&mut self) {
        // Placeholder for monitoring logic
        // This would involve fetching new transactions, updating balances, and storing them in the database
    }

    async fn process_removes_address(
        &self,
        block_height: u32,
        removed_addresses: Vec<USDBScriptHash>,
    ) -> Result<(), String> {
        assert!(
            !removed_addresses.is_empty(),
            "process_removes_address should not be called with empty removed_addresses"
        );

        let mut active_addresses = self.active_addresses.lock().unwrap();

        // Load all removed addresses' balances at the previous block height from the balance history service and remove them from the active_addresses map

        let ret = self
            .balance_history_client
            .get_addresses_balances(removed_addresses.clone(), Some(block_height - 1), None)
            .await?;
        assert_eq!(
            removed_addresses.len(),
            ret.len(),
            "Expected balance history results for all removed addresses"
        );
        for (script_hash, balances) in removed_addresses.into_iter().zip(ret.into_iter()) {
            if balances.len() != 1 {
                let msg = format!(
                    "Expected exactly one balance for script hash {} at block height {}, got {}",
                    script_hash,
                    block_height,
                    balances.len()
                );
                error!("{}", msg);
                return Err(msg);
            }
            let balance = balances.into_iter().next().unwrap();
            info!(
                "Final balance for removed address {} at block height {}, balance at previous block height:{}",
                script_hash, block_height, balance.balance
            );

            // Remove the address from the active_addresses map and ensure it is removed
            if active_addresses.remove(&script_hash).is_none() {
                let msg = format!(
                    "Removed address {} is not found in active_addresses map when removing",
                    script_hash
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        Ok(())
    }

    async fn process_new_address(
        &self,
        block_height: u32,
        new_addresses: Vec<USDBScriptHash>,
    ) -> Result<(), String> {
        assert!(
            !new_addresses.is_empty(),
            "process_new_address should not be called with empty new_addresses"
        );
        let mut active_addresses = self.active_addresses.lock().unwrap();

        // Load all new addresses' balances at the current block height from the balance history service and add them to the active_addresses map

        let ret = self
            .balance_history_client
            .get_addresses_balances(new_addresses.clone(), Some(block_height), None)
            .await?;
        assert_eq!(
            new_addresses.len(),
            ret.len(),
            "Expected balance history results for all new addresses"
        );
        for (script_hash, balances) in new_addresses.into_iter().zip(ret.into_iter()) {
            if balances.len() != 1 {
                let msg = format!(
                    "Expected exactly one balance for script hash {} at block height {}, got {}",
                    script_hash,
                    block_height,
                    balances.len()
                );
                error!("{}", msg);
                return Err(msg);
            }
            let balance = balances.into_iter().next().unwrap();
            info!(
                "Initial balance for new address {} at block height {}: {}",
                script_hash, block_height, balance.balance
            );

            // Ensure the address is not already in the active_addresses map before inserting
            let ret = active_addresses.insert(script_hash.clone(), balance.balance);
            if ret.is_some() {
                let msg = format!(
                    "New address {} is already in active_addresses map when inserting at block height {}",
                    script_hash, block_height
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        Ok(())
    }

    // If the active_addresses set is not so large, we can directly load all deltas for all active addresses at the current block height from the balance history service, and log the total delta for this block height. Otherwise, we can scan the block for all input and output addresses, and only load the changed addresses' balances at the current block height from the balance history service, and log the total delta for this block height
    async fn process_delta(&self, block_height: u32) -> Result<i64, String> {
        let active_addresses = self.active_addresses.lock().unwrap();
        if active_addresses.is_empty() {
            // If there is no active address, we can skip processing the block
            debug!(
                "No active addresses to monitor at block height {}, skipping processing",
                block_height
            );
            return Ok(0);
        }

        // We direct load all deltas for all active addresses at the current block height from the balance history service, and log the total delta for this block height
        let list = active_addresses.keys().cloned().collect();
        let ret = self
            .balance_history_client
            .get_addresses_balances_delta(list, Some(block_height), None)
            .await?;
        assert_eq!(
            active_addresses.len(),
            ret.len(),
            "Expected balance history results for all active addresses"
        );

        let mut total_delta = 0;
        for (script_hash, balance_delta) in active_addresses.keys().cloned().zip(ret.into_iter()) {
            if balance_delta.is_empty() {
                // If there is no balance delta for the active address at the current block height, we can skip it
                continue;
            }

            let balance_delta = balance_delta.into_iter().next().flatten();
            if balance_delta.is_none() {
                // If there is no balance delta for the active address at the current block height, we can skip it
                continue;
            }

            let balance_delta = balance_delta.unwrap();
            info!(
                "Balance delta for script hash {} at block height {}: balance={}, delta={}",
                script_hash, block_height, balance_delta.balance, balance_delta.delta
            );
            total_delta += balance_delta.delta;
        }

        Ok(total_delta)
    }

    // If the active_addresses set is large, we can scan the block for all input and output addresses, and only load the changed addresses' balances at the current block height from the balance history service, and log the total delta for this block height
    async fn process_delta_by_utxo(&self, block_height: u32) -> Result<i64, String> {
        let active_addresses = self.active_addresses.lock().unwrap();
        if active_addresses.is_empty() {
            // If there is no active address, we can skip processing the block
            debug!(
                "No active addresses to monitor at block height {}, skipping processing",
                block_height
            );
            return Ok(0);
        }

        let block = self.btc_client.get_block(block_height)?;

        // Scan the block for all input and output addresses, and collect them into a set of changed addresses if the address is in the active_addresses set

        // Check all input and output addresses to check if they are in the active_addresses set
        let mut changed_addresses: HashSet<USDBScriptHash> = HashSet::new();
        for tx in block.txdata {
            if !tx.is_coinbase() {
                for input in tx.input {
                    let utxo = self.btc_client.get_utxo(&input.previous_output)?;
                    let script_hash = utxo.0.to_usdb_script_hash();
                    if active_addresses.contains_key(&script_hash) {
                        changed_addresses.insert(script_hash);
                    }
                }
            }

            for output in tx.output {
                let script_hash = output.script_pubkey.to_usdb_script_hash();
                if active_addresses.contains_key(&script_hash) {
                    changed_addresses.insert(script_hash);
                }
            }
        }

        let mut total_delta = 0;
        if !changed_addresses.is_empty() {
            // Load all changed addresses' balances at the current block height from the balance history service
            let list = changed_addresses.iter().cloned().collect();
            let ret = self
                .balance_history_client
                .get_addresses_balances_delta(list, Some(block_height), None)
                .await?;

            // Calculate the balance delta for each changed address and update the active_addresses map
            for (script_hash, balance_delta) in changed_addresses.into_iter().zip(ret.into_iter()) {
                if balance_delta.len() != 1 {
                    let msg = format!(
                        "Expected exactly one balance delta for script hash {}, got {}",
                        script_hash,
                        balance_delta.len()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }
                let balance_delta = balance_delta.into_iter().next().flatten();
                if balance_delta.is_none() {
                    let msg = format!(
                        "No balance delta found for script hash {} at block height {}",
                        script_hash, block_height
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                let balance_delta = balance_delta.unwrap();
                info!(
                    "Balance delta for script hash {} at block height {}: balance={}, delta={}",
                    script_hash, block_height, balance_delta.balance, balance_delta.delta
                );
                total_delta += balance_delta.delta;
            }
        }

        info!(
            "Total balance delta for block {}: {}",
            block_height, total_delta
        );

        Ok(total_delta)
    }
}
