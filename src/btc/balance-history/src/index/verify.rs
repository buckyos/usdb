use crate::config::BalanceHistoryConfigRef;
use crate::db::{AddressDBRef, BalanceHistoryDBRef, SnapshotDBRef};
use crate::output::IndexOutputRef;
use bitcoincore_rpc::bitcoin::ScriptBuf;
use bitcoincore_rpc::bitcoin::address::Address;
use usdb_util::{ElectrsClientRef, ToUSDBScriptHash, USDBScriptHash};

pub struct BalanceHistoryVerifier {
    config: BalanceHistoryConfigRef,
    electrs_client: ElectrsClientRef,
    db: BalanceHistoryDBRef,
    output: IndexOutputRef,
}

impl BalanceHistoryVerifier {
    pub fn new(
        config: BalanceHistoryConfigRef,
        electrs_client: ElectrsClientRef,
        db: BalanceHistoryDBRef,
        output: IndexOutputRef,
    ) -> Self {
        Self {
            config,
            electrs_client,
            db,
            output,
        }
    }

    pub fn verify_latest(&self, start: Option<USDBScriptHash>) -> Result<(), String> {
        info!("Starting full balance history verification for latest block height");

        let mut script_hashes = vec![];
        let mut balances = vec![];

        const BATCH_SIZE: usize = 256;
        let mut total = 0u64;
        self.output.start_index(u32::MAX as u64, 0);
        self.db.traverse_latest(start, 1, |entries| {
            assert!(
                entries.len() == 1,
                "Expected exactly one snapshot entry for latest block height, found {}",
                entries.len()
            );

            script_hashes.push(entries[0].script_hash.clone());
            balances.push(entries[0].balance);

            if script_hashes.len() >= BATCH_SIZE {
                // Verify batch
                if let Err(e) = self.verify_address_latest_balance_batch_sync(&script_hashes, &balances) {
                    warn!("Failed to verify address batch: {}", e);
                    self.db.flush_with_primary()?;

                    // Sleep for a while to allow indexer to catch up
                    std::thread::sleep(std::time::Duration::from_secs(10));

                    // Retry once after flushing
                    self.verify_address_latest_balance_batch_sync(&script_hashes, &balances)?;
                }

                script_hashes.clear();
                balances.clear();

                // Use prefix of 8 bytes as progress indicator, from FFFFFFFF... to 00000000...

                let hash = entries[0].script_hash.as_ref() as &[u8];
                let pos = u32::MAX - u32::from_be_bytes(hash[0..4].try_into().unwrap());

                self.output.update_current_height(pos as u64);
                self.output.set_index_message(&format!(
                    "Verifying balance history [{} - {}]",
                    total,
                    total + BATCH_SIZE as u64
                ));
                total += BATCH_SIZE as u64;
            }

            Ok(())
        })
    }

    pub fn verify_at_height(
        &self,
        target_block_height: u32,
        start: Option<USDBScriptHash>,
    ) -> Result<(), String> {
        info!(
            "Starting full balance history verification at block height {}",
            target_block_height
        );

        self.db
            .traverse_at_height(start, target_block_height, 1, |entries| {
                assert!(
                    entries.len() == 1,
                    "Expected exactly one snapshot entry for block height {}, found {}",
                    target_block_height,
                    entries.len()
                );

                let entry = &entries[0];
                self.verify_address_balance_at_height_sync(
                    &entry.script_hash,
                    target_block_height,
                    entry.balance,
                )
            })
    }

    pub fn verify_address_latest(
        &self,
        script_hash: &USDBScriptHash,
    ) -> Result<(), String> {
        self.output.println(&format!(
            "Starting latest balance history verification for script_hash: {}",
            script_hash
        ));

        let entry = self.db.get_latest_balance(script_hash)?;
        let latest_block_height = self.db.get_btc_block_height()?;

        self.verify_address_latest_balance_sync(script_hash, latest_block_height, entry.balance)
    }

    pub fn verify_address_at_height(
        &self,
        script_hash: &USDBScriptHash,
        block_height: u32,
    ) -> Result<(), String> {
        self.output.println(&format!(
            "Starting full balance history verification for script_hash: {} up to block height {}",
            script_hash, block_height
        ));

        let history = tokio::runtime::Handle::current().block_on(async {
            self.electrs_client
                .calc_balance_history(script_hash, block_height)
                .await
        })?;

        let address = Address::from_script(&history.script_buf, self.config.btc.network())
            .map_err(|e| {
                let msg = format!(
                    "Failed to parse address from script for script_hash {}: {}",
                    script_hash, e
                );
                error!("{}", msg);
                msg
            })?;

        for data in history.history {
            let entry = self
                .db
                .get_balance_at_block_height(script_hash, data.block_height)?;
            if entry.balance != data.balance || entry.delta != data.delta {
                let msg = format!(
                    "Balance history mismatch for script_hash {} at block height {}: expected (delta={}, balance={}), got (delta={}, balance={}), address {}",
                    script_hash,
                    data.block_height,
                    entry.delta,
                    entry.balance,
                    data.delta,
                    data.balance,
                    address
                );
                error!("{}", msg);

                let all = self.db.get_all_balance(script_hash)?;
                error!(
                    "Full balance history for script_hash {}: {:?}",
                    script_hash, all
                );

                return Err(msg);
            }
        }

        info!(
            "Completed full balance history verification for script_hash: {} up to block height {} address {}",
            script_hash, block_height, address
        );

        Ok(())
    }

    fn verify_address_balance_at_height_sync(
        &self,
        script_hash: &USDBScriptHash,
        block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        tokio::runtime::Handle::current().block_on(async {
            self.verify_address_balance_at_height(script_hash, block_height, balance)
                .await
        })
    }

    async fn verify_address_balance_at_height(
        &self,
        script_hash: &USDBScriptHash,
        block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        /*
        let script = self.address_db.get_address(script_hash)?.ok_or_else(|| {
            let msg = format!("Address not found for script_hash: {}", script_hash);
            error!("{}", msg);
            msg
        })?;
        let addr = Address::from_script(&script, self.config.btc.network()).map_err(|e| {
            let msg = format!(
                "Failed to parse address from script for script_hash {}: {}",
                script_hash, e
            );
            error!("{}", msg);
            msg
        })?;
        info!(
            "Loaded address for script_hash {} -> addr {}",
            script_hash, addr
        );
        */

        let electrs_balance = self
            .electrs_client
            .calc_balance(script_hash, block_height)
            .await?;

        let address = Address::from_script(&electrs_balance.script_buf, self.config.btc.network())
            .map_err(|e| {
                let msg = format!(
                    "Failed to parse address from script for script_hash {}: {}",
                    script_hash, e
                );
                error!("{}", msg);
                msg
            })?;

        if electrs_balance.balance != balance {
            let msg = format!(
                "Balance mismatch for script_hash {} at block height {}: expected {}, got {}, address {}",
                script_hash, block_height, balance, electrs_balance.balance, address
            );
            error!("{}", msg);

            let all = self.db.get_all_balance(script_hash)?;
            error!(
                "Full balance history for script_hash {}: {:?}",
                script_hash, all
            );
            return Err(msg);
        }

        info!(
            "Balance history verification successful for script_hash {} at block height {}: balance={}, address={}",
            script_hash, block_height, balance, address
        );
        Ok(())
    }

    fn verify_address_latest_balance_sync(
        &self,
        script_hash: &USDBScriptHash,
        latest_block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        tokio::runtime::Handle::current()
            .block_on(async { self.verify_address_latest_balance(script_hash, latest_block_height, balance).await })
    }

    async fn verify_address_latest_balance(
        &self,
        script_hash: &USDBScriptHash,
        latest_block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        let electrs_balance = self.electrs_client.get_balance(script_hash).await?;

        if electrs_balance != balance {
            let msg = format!(
                "Balance mismatch for script_hash {}: expected {}, got {}, used latest block height {}",
                script_hash, balance, electrs_balance, latest_block_height
            );
            error!("{}", msg);

            let all = self.db.get_all_balance(script_hash)?;
            error!(
                "Full balance history for script_hash {}: {:?}",
                script_hash, all
            );
            return Err(msg);
        }

        info!(
            "Balance history verification successful for script_hash {}: balance={}, used latest block height={}",
            script_hash, balance, latest_block_height
        );
        Ok(())
    }

    fn verify_address_latest_balance_batch_sync(
        &self,
        script_hashes: &[USDBScriptHash],
        balances: &[u64],
    ) -> Result<(), String> {
        tokio::runtime::Handle::current().block_on(async {
            self.verify_address_latest_balance_batch(script_hashes, balances)
                .await
        })
    }

    async fn verify_address_latest_balance_batch(
        &self,
        script_hashes: &[USDBScriptHash],
        balances: &[u64],
    ) -> Result<(), String> {
        let electrs_balances = self.electrs_client.get_balances(script_hashes).await?;

        for i in 0..script_hashes.len() {
            if electrs_balances[i] != balances[i] {
                let msg = format!(
                    "Balance mismatch for script_hash {}: expected {}, got {}",
                    script_hashes[i], balances[i], electrs_balances[i]
                );
                error!("{}", msg);

                let all = self.db.get_all_balance(&script_hashes[i])?;
                error!(
                    "Full balance history for script_hash {}: {:?}",
                    script_hashes[i], all
                );
                return Err(msg);
            }
        }

        /*
        for i in 0..script_hashes.len() {
            info!(
                "Balance history verification successful for script_hash {}: balance={}",
                script_hashes[i], balances[i]
            );
        }
        */
        Ok(())
    }
}

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

        let entries = self.snapshot_db.get_entries_by_page(index as u32, 1)?;
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
        let ret = self
            .electrs_client
            .calc_balance(&snapshot_entry.script_hash, snapshot_entry.block_height)
            .await?;

        let addr =
            Address::from_script(&ret.script_buf, self.config.btc.network()).map_err(|e| {
                let msg = format!(
                    "Failed to parse address from script for script_hash {}: {}",
                    snapshot_entry.script_hash, e
                );
                error!("{}", msg);
                msg
            })?;

        assert!(
            ret.balance == snapshot_entry.balance,
            "Balance mismatch for script_hash {}: expected {}, got {}, address {}",
            snapshot_entry.script_hash,
            snapshot_entry.balance,
            ret.balance,
            addr
        );

        info!(
            "Snapshot verification successful for index {}: script_hash={}, balance={}, address={}",
            index, snapshot_entry.script_hash, snapshot_entry.balance, addr
        );

        Ok(())
    }

    fn load_address_by_script_hash(
        &self,
        script_hash: &USDBScriptHash,
    ) -> Result<ScriptBuf, String> {
        let addr_entry = self.address_db.get_address(script_hash)?;
        match addr_entry {
            Some(entry) => {
                debug!(
                    "Loaded address for script hash {} -> {}",
                    script_hash,
                    entry.to_usdb_script_hash()
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
}
