use crate::config::BalanceHistoryConfigRef;
use crate::db::{AddressDBRef, BalanceHistoryDBRef, SnapshotDBRef};
use crate::output::IndexOutputRef;
use bitcoincore_rpc::bitcoin::address::Address;
use bitcoincore_rpc::bitcoin::{Network, Script, ScriptBuf};
use usdb_util::{BtcScriptHash, ElectrsClientRef, ToBtcScriptHash, is_core_unspendable};

fn verify_electrs_script(
    expected_script_hash: &BtcScriptHash,
    script: &Script,
    network: Network,
) -> Result<String, String> {
    if is_core_unspendable(script) {
        return Err(format!(
            "Electrs returned a Bitcoin Core unspendable script for script_hash {}",
            expected_script_hash
        ));
    }

    let actual_script_hash = script.to_btc_script_hash();
    if actual_script_hash != *expected_script_hash {
        return Err(format!(
            "Electrs script hash mismatch: expected {}, got {}",
            expected_script_hash, actual_script_hash
        ));
    }

    Ok(Address::from_script(script, network)
        .map(|address| address.to_string())
        .unwrap_or_else(|_| format!("non-address-script:{}", actual_script_hash)))
}

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

    pub fn verify_latest(&self, start: Option<BtcScriptHash>) -> Result<(), String> {
        let stable_height = self.db.get_btc_block_height()?;
        info!(
            "Starting full balance history verification for stable block height {}",
            stable_height
        );

        let mut script_hashes = vec![];
        let mut balances = vec![];

        const BATCH_SIZE: usize = 256;
        let mut total = 0u64;
        self.output.start_index(u32::MAX as u64, 0);
        self.db.traverse_latest(start, 1, |entries| {
            if entries.len() != 1 {
                return Err(format!(
                    "Expected exactly one snapshot entry for stable block height {}, found {}",
                    stable_height,
                    entries.len()
                ));
            }

            script_hashes.push(entries[0].script_hash);
            balances.push(entries[0].balance);

            if script_hashes.len() >= BATCH_SIZE {
                // Verify batch
                if let Err(e) = self.verify_address_latest_balance_batch_sync(
                    &script_hashes,
                    &balances,
                    stable_height,
                ) {
                    warn!("Failed to verify address batch: {}", e);
                    self.db.flush_with_primary()?;

                    // Sleep for a while to allow electrs to catch up.
                    std::thread::sleep(std::time::Duration::from_secs(10));

                    // Retry once after flushing
                    self.verify_address_latest_balance_batch_sync(
                        &script_hashes,
                        &balances,
                        stable_height,
                    )?;
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
        })?;

        if !script_hashes.is_empty() {
            self.verify_address_latest_balance_batch_sync(
                &script_hashes,
                &balances,
                stable_height,
            )?;
        }

        Ok(())
    }

    pub fn verify_at_height(
        &self,
        target_block_height: u32,
        start: Option<BtcScriptHash>,
    ) -> Result<(), String> {
        info!(
            "Starting full balance history verification at block height {}",
            target_block_height
        );

        self.db
            .traverse_at_height(start, target_block_height, 1, |entries| {
                if entries.len() != 1 {
                    return Err(format!(
                        "Expected exactly one snapshot entry for block height {}, found {}",
                        target_block_height,
                        entries.len()
                    ));
                }

                let entry = &entries[0];
                self.verify_address_balance_at_height_sync(
                    &entry.script_hash,
                    target_block_height,
                    entry.balance,
                )
            })
    }

    pub fn verify_address_latest(&self, script_hash: &BtcScriptHash) -> Result<(), String> {
        self.output.println(&format!(
            "Starting stable-height balance verification for script_hash: {}",
            script_hash
        ));

        let entry = self.db.get_latest_balance(script_hash)?;
        let latest_block_height = self.db.get_btc_block_height()?;

        self.verify_address_latest_balance_sync(script_hash, latest_block_height, entry.balance)
    }

    pub fn verify_address_at_height(
        &self,
        script_hash: &BtcScriptHash,
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

        let script_label = verify_electrs_script(
            script_hash,
            history.script_buf.as_script(),
            self.config.btc.network(),
        )?;

        for data in history.history {
            let entry = self
                .db
                .get_balance_at_block_height(script_hash, data.block_height)?;
            if entry.balance != data.balance || entry.delta != data.delta {
                let msg = format!(
                    "Balance history mismatch for script_hash {} at block height {}: expected (delta={}, balance={}), got (delta={}, balance={}), script {}",
                    script_hash,
                    data.block_height,
                    entry.delta,
                    entry.balance,
                    data.delta,
                    data.balance,
                    script_label
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
            "Completed full balance history verification for script_hash: {} up to block height {} script {}",
            script_hash, block_height, script_label
        );

        Ok(())
    }

    fn verify_address_balance_at_height_sync(
        &self,
        script_hash: &BtcScriptHash,
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
        script_hash: &BtcScriptHash,
        block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        let electrs_balance = self
            .electrs_client
            .calc_balance(script_hash, block_height)
            .await?;

        let script_label = verify_electrs_script(
            script_hash,
            electrs_balance.script_buf.as_script(),
            self.config.btc.network(),
        )?;

        if electrs_balance.balance != balance {
            let msg = format!(
                "Balance mismatch for script_hash {} at block height {}: expected {}, got {}, script {}",
                script_hash, block_height, balance, electrs_balance.balance, script_label
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
            "Balance history verification successful for script_hash {} at block height {}: balance={}, script={}",
            script_hash, block_height, balance, script_label
        );
        Ok(())
    }

    fn verify_address_latest_balance_sync(
        &self,
        script_hash: &BtcScriptHash,
        latest_block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        tokio::runtime::Handle::current().block_on(async {
            self.verify_address_latest_balance(script_hash, latest_block_height, balance)
                .await
        })
    }

    async fn verify_address_latest_balance(
        &self,
        script_hash: &BtcScriptHash,
        latest_block_height: u32,
        balance: u64,
    ) -> Result<(), String> {
        let electrs_balance = self
            .electrs_client
            .calc_balance(script_hash, latest_block_height)
            .await?;

        let script_label = verify_electrs_script(
            script_hash,
            electrs_balance.script_buf.as_script(),
            self.config.btc.network(),
        )?;

        if electrs_balance.balance != balance {
            let msg = format!(
                "Balance mismatch for script_hash {} at stable block height {}: expected {}, got {}, script {}",
                script_hash, latest_block_height, balance, electrs_balance.balance, script_label
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
            "Balance history verification successful for script_hash {} at stable block height {}: balance={}, script={}",
            script_hash, latest_block_height, balance, script_label
        );
        Ok(())
    }

    fn verify_address_latest_balance_batch_sync(
        &self,
        script_hashes: &[BtcScriptHash],
        balances: &[u64],
        stable_height: u32,
    ) -> Result<(), String> {
        tokio::runtime::Handle::current().block_on(async {
            self.verify_address_latest_balance_batch(script_hashes, balances, stable_height)
                .await
        })
    }

    async fn verify_address_latest_balance_batch(
        &self,
        script_hashes: &[BtcScriptHash],
        balances: &[u64],
        stable_height: u32,
    ) -> Result<(), String> {
        if script_hashes.len() != balances.len() {
            return Err(format!(
                "Verifier batch length mismatch: script_hashes={}, balances={}",
                script_hashes.len(),
                balances.len()
            ));
        }

        for i in 0..script_hashes.len() {
            let electrs_balance = self
                .electrs_client
                .calc_balance(&script_hashes[i], stable_height)
                .await?;

            let script_label = verify_electrs_script(
                &script_hashes[i],
                electrs_balance.script_buf.as_script(),
                self.config.btc.network(),
            )?;

            if electrs_balance.balance != balances[i] {
                let msg = format!(
                    "Balance mismatch for script_hash {} at stable block height {}: expected {}, got {}, script {}",
                    script_hashes[i],
                    stable_height,
                    balances[i],
                    electrs_balance.balance,
                    script_label
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

        let entries = self
            .snapshot_db
            .get_balance_history_entries_by_page(index as u32, 1)?;
        if entries.len() != 1 {
            return Err(format!(
                "Expected exactly one snapshot entry for index {}, found {}",
                index,
                entries.len()
            ));
        }

        let snapshot_entry = &entries[0];
        info!(
            "Verifying snapshot at index {}: script_hash={}, balance={}",
            index, snapshot_entry.script_hash, snapshot_entry.balance
        );

        // Calculate balance from electrs
        let ret = self
            .electrs_client
            .calc_balance(&snapshot_entry.script_hash, snapshot_entry.block_height)
            .await?;

        let script_label = verify_electrs_script(
            &snapshot_entry.script_hash,
            ret.script_buf.as_script(),
            self.config.btc.network(),
        )?;

        if ret.balance != snapshot_entry.balance {
            return Err(format!(
                "Balance mismatch for script_hash {}: expected {}, got {}, script {}",
                snapshot_entry.script_hash, snapshot_entry.balance, ret.balance, script_label
            ));
        }

        info!(
            "Snapshot verification successful for index {}: script_hash={}, balance={}, script={}",
            index, snapshot_entry.script_hash, snapshot_entry.balance, script_label
        );

        Ok(())
    }

    fn load_address_by_script_hash(
        &self,
        script_hash: &BtcScriptHash,
    ) -> Result<ScriptBuf, String> {
        let addr_entry = self.address_db.get_address(script_hash)?;
        match addr_entry {
            Some(entry) => {
                debug!(
                    "Loaded address for script hash {} -> {}",
                    script_hash,
                    entry.to_btc_script_hash()
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verifier_accepts_non_address_spendable_script_by_hash() {
        let script = ScriptBuf::from(vec![0x51, 0x51]);
        let script_hash = script.to_btc_script_hash();

        let label =
            verify_electrs_script(&script_hash, script.as_script(), Network::Regtest).unwrap();
        assert_eq!(label, format!("non-address-script:{}", script_hash));
    }

    #[test]
    fn verifier_rejects_script_hash_mismatch_and_core_unspendable_script() {
        let script = ScriptBuf::from(vec![0x51, 0x51]);
        let different_hash = ScriptBuf::from(vec![0x51]).to_btc_script_hash();
        assert!(
            verify_electrs_script(&different_hash, script.as_script(), Network::Regtest)
                .unwrap_err()
                .contains("script hash mismatch")
        );

        let oversized = ScriptBuf::from(vec![0x51; 10_001]);
        let oversized_hash = oversized.to_btc_script_hash();
        assert!(
            verify_electrs_script(&oversized_hash, oversized.as_script(), Network::Regtest)
                .unwrap_err()
                .contains("unspendable")
        );
    }
}
