use crate::config::BalanceHistoryConfigRef;
use crate::db::{AddressDBRef, BalanceHistoryDBRef, SnapshotDBRef};
use bitcoincore_rpc::bitcoin::ScriptBuf;
use bitcoincore_rpc::bitcoin::address::Address;
use usdb_util::{ElectrsClientRef, ToUSDBScriptHash, USDBScriptHash};

pub struct BalanceHistoryVerifier {
    config: BalanceHistoryConfigRef,
    electrs_client: ElectrsClientRef,
    address_db: AddressDBRef,
    db: BalanceHistoryDBRef,
}

impl BalanceHistoryVerifier {
    pub fn new(
        config: BalanceHistoryConfigRef,
        electrs_client: ElectrsClientRef,
        address_db: AddressDBRef,
        db: BalanceHistoryDBRef,
    ) -> Self {
        Self {
            config,
            electrs_client,
            address_db,
            db,
        }
    }

    pub fn verify_latest(&self) -> Result<(), String> {
        info!("Starting full balance history verification for latest block height");

        self.db.traverse_latest(1, |entries| {
            assert!(
                entries.len() == 1,
                "Expected exactly one snapshot entry for latest block height, found {}",
                entries.len()
            );

            let entry = &entries[0];
            self.verify_address_at_height_sync(
                &entry.script_hash,
                entry.block_height,
                entry.balance,
            )
        })
    }

    pub fn verify_at_height(&self, target_block_height: u32) -> Result<(), String> {
        info!(
            "Starting full balance history verification at block height {}",
            target_block_height
        );

        self.db
            .traverse_at_height(target_block_height, 1, |entries| {
                assert!(
                    entries.len() == 1,
                    "Expected exactly one snapshot entry for block height {}, found {}",
                    target_block_height,
                    entries.len()
                );

                let entry = &entries[0];
                self.verify_address_at_height_sync(
                    &entry.script_hash,
                    target_block_height,
                    entry.balance,
                )
            })
    }

    pub fn verify_address(&self, script_hash: &USDBScriptHash) -> Result<(), String> {
        let block_height = self.db.get_btc_block_height()?;
        info!(
            "Starting full balance history verification for script_hash: {} up to block height {}",
            script_hash, block_height
        );

        let history = tokio::runtime::Handle::current().block_on(async {
            self.electrs_client
                .calc_balance_history(script_hash, block_height)
                .await
        })?;

        for (height, delta, balance) in history {
            let entry = self.db.get_balance_at_block_height(script_hash, height)?;
            if entry.balance != balance || entry.delta != delta {
                let msg = format!(
                    "Balance history mismatch for script_hash {} at block height {}: expected (delta={}, balance={}), got (delta={}, balance={})",
                    script_hash, height, entry.delta, entry.balance, delta, balance
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        info!(
            "Completed full balance history verification for script_hash: {} up to block height {}",
            script_hash, block_height
        );

        Ok(())
    }

    fn verify_address_at_height_sync(
        &self,
        script_hash: &USDBScriptHash,
        block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        tokio::runtime::Handle::current().block_on(async {
            self.verify_address_at_height(script_hash, block_height, balance)
                .await
        })
    }

    async fn verify_address_at_height(
        &self,
        script_hash: &USDBScriptHash,
        block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        info!(
            "Starting balance history verification for script_hash: {}",
            script_hash
        );

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

        let electrs_balance = self
            .electrs_client
            .calc_balance(script_hash, block_height)
            .await?;

        if balance != electrs_balance {
            let msg = format!(
                "Balance mismatch for script_hash {} at block height {}: expected {}, got {}",
                script_hash, block_height, balance, electrs_balance
            );
            error!("{}", msg);
            return Err(msg);
        }

        info!(
            "Balance history verification successful for script_hash {} at block height {}: balance={}",
            script_hash, block_height, balance
        );
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

        let address = match Address::from_script(&script, self.config.btc.network()) {
            Ok(addr) => {
                format!("addr:{}", addr)
            }
            Err(_) => {
                // Non-standard address, use script representation
                format!("script_hash:{}", script)
            }
        };
        info!(
            "Loaded address {} -> {}",
            snapshot_entry.script_hash, address
        );

        let ret = self
            .electrs_client
            .calc_balance(&snapshot_entry.script_hash, snapshot_entry.block_height)
            .await?;
        assert!(
            ret == snapshot_entry.balance,
            "Balance mismatch for script_hash {}: expected {}, got {}",
            snapshot_entry.script_hash,
            snapshot_entry.balance,
            ret
        );

        info!(
            "Snapshot verification successful for index {}: script_hash={}, balance={}",
            index, snapshot_entry.script_hash, snapshot_entry.balance
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
