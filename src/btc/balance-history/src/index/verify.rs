use crate::config::BalanceHistoryConfigRef;
use crate::db::{AddressDB, AddressDBRef, SnapshotDB, SnapshotDBRef};
use bitcoincore_rpc::bitcoin::address::{Address, NetworkChecked};
use bitcoincore_rpc::bitcoin::{Script, ScriptBuf, ScriptHash};
use usdb_util::{ElectrsClient, ElectrsClientRef};

pub struct SnapshotVerifier {
    config: BalanceHistoryConfigRef,
    electrs_client: ElectrsClientRef,
    address_db: AddressDBRef,
    snapshot_db: SnapshotDBRef,
}

impl SnapshotVerifier {
    pub fn new(
        config: BalanceHistoryConfigRef,
        electrs_client: ElectrsClientRef,
        address_db: AddressDBRef,
        snapshot_db: SnapshotDBRef,
    ) -> Self {
        Self {
            config,
            electrs_client,
            address_db,
            snapshot_db,
        }
    }

    pub async fn verify(&self, index: u64) -> Result<(), String> {
        info!("Starting snapshot verification");

        let entries = self.snapshot_db.get_entries(index, 1)?;
        assert!(
            entries.len() == 1,
            "Expected exactly one snapshot entry for index {}, found {}",
            index,
            entries.len()
        );

        let snapshot_entry = &entries[0];
        info!(
            "Verifying snapshot at index {}: address={}, balance={}",
            index, snapshot_entry.script_hash, snapshot_entry.balance
        );

        // Calculate balance from electrs
        let script = self
            .load_address_by_script_hash(&snapshot_entry.script_hash)
            .map_err(|e| {
                let msg = format!(
                    "Failed to load address by script hash {}: {}",
                    snapshot_entry.script_hash, e
                );
                error!("{}", msg);
                msg
            })?;
        let address = Address::from_script(&script, self.config.btc.network()).map_err(|e| {
            let msg = format!("Failed to create address from script: {}", e);
            error!("{}", msg);
            msg
        })?;
        info!(
            "Loaded address {} -> {}",
            snapshot_entry.script_hash, address
        );

        let ret = self
            .calc_balance_from_electrs(&script, snapshot_entry.block_height)
            .await?;
        assert!(
            ret == snapshot_entry.balance,
            "Balance mismatch for address {}: expected {}, got {}",
            address,
            snapshot_entry.balance,
            ret
        );

        info!(
            "Snapshot verification successful for index {}: address={}, balance={}",
            index, snapshot_entry.script_hash, snapshot_entry.balance
        );
        Ok(())
    }

    fn load_address_by_script_hash(&self, script_hash: &ScriptHash) -> Result<ScriptBuf, String> {
        let addr_entry = self.address_db.get_address(script_hash)?;
        match addr_entry {
            Some(entry) => {
                info!(
                    "Loaded address for script hash {} -> {}",
                    script_hash, entry.script_hash()
                );
                Ok(entry)
            }
            None => {
                let msg = format!("Address not found for script hash {}", script_hash);
                error!("{}", msg);
                Err(msg)
            }
        }
    }

    async fn calc_balance_from_electrs(
        &self,
        address: &Script,
        block_height: u32,
    ) -> Result<u64, String> {
        let history = self.electrs_client.get_history_by_script(address).await?;

        let mut balance: i64 = 0;
        for item in history {
            if item.height > block_height as i32 {
                break;
            }
            // Load tx from btc client
            let tx = self.electrs_client.expand_tx(&item.tx_hash).await?;

            let delta = tx.amount_delta_from_tx(address)?;
            balance += delta;
            assert!(
                balance >= 0,
                "Balance went negative for address {}",
                address
            );
        }

        info!(
            "Calculated balance for address {} at block height {}: {}",
            address, block_height, balance
        );
        Ok(balance as u64)
    }
}
