use crate::config::ConfigManagerRef;
use crate::storage::{ActiveBalanceSnapshot, MinerPassStorageRef};
use ord::InscriptionId;
use std::sync::Arc;
use std::time::Instant;
use usdb_util::USDBScriptHash;

use super::{
    BalanceHistoryBackend, BalanceRpcLoader, ConcurrentBalanceLoader, SerialBalanceLoader,
};

pub struct BalanceMonitor {
    miner_pass_storage: MinerPassStorageRef,
    rpc_loader: Arc<dyn BalanceRpcLoader>,
    active_address_page_size: usize,
    balance_query_batch_size: usize,
}

impl BalanceMonitor {
    pub fn new(
        config: ConfigManagerRef,
        miner_pass_storage: MinerPassStorageRef,
    ) -> Result<Self, String> {
        let active_address_page_size = config.config().usdb.active_address_page_size;
        if active_address_page_size == 0 {
            let msg = "Invalid config: usdb.active_address_page_size must be > 0".to_string();
            error!("{}", msg);
            return Err(msg);
        }

        let batch_size = config.config().usdb.balance_query_batch_size;
        if batch_size == 0 {
            let msg = "Invalid config: usdb.balance_query_batch_size must be > 0".to_string();
            error!("{}", msg);
            return Err(msg);
        }
        let concurrency = config.config().usdb.balance_query_concurrency;
        let timeout_ms = config.config().usdb.balance_query_timeout_ms;
        let max_retries = config.config().usdb.balance_query_max_retries;

        let backend = Arc::new(BalanceHistoryBackend::new(
            &config.config().balance_history.rpc_url,
        )?);

        // Prefer serial mode when concurrency=1 and no retries are needed.
        // Otherwise use concurrent mode with timeout/retry controls.
        let rpc_loader: Arc<dyn BalanceRpcLoader> = if concurrency == 1 && max_retries == 0 {
            Arc::new(SerialBalanceLoader::new(backend, batch_size)?)
        } else {
            Arc::new(ConcurrentBalanceLoader::new(
                backend,
                batch_size,
                concurrency,
                timeout_ms,
                max_retries,
            )?)
        };

        Ok(Self {
            miner_pass_storage,
            rpc_loader,
            active_address_page_size,
            balance_query_batch_size: batch_size,
        })
    }

    #[cfg(test)]
    pub(crate) fn new_with_loader(
        miner_pass_storage: MinerPassStorageRef,
        rpc_loader: Arc<dyn BalanceRpcLoader>,
        active_address_page_size: usize,
        balance_query_batch_size: usize,
    ) -> Self {
        assert!(active_address_page_size > 0);
        assert!(balance_query_batch_size > 0);
        Self {
            miner_pass_storage,
            rpc_loader,
            active_address_page_size,
            balance_query_batch_size,
        }
    }

    fn load_active_addresses(&self, block_height: u32) -> Result<Vec<USDBScriptHash>, String> {
        let mut page = 0usize;
        let mut owner_to_pass = std::collections::HashMap::<USDBScriptHash, InscriptionId>::new();

        loop {
            // Load active owners from pass history snapshot at target height.
            // This avoids using current-state rows when replaying historical blocks.
            let active_passes = self
                .miner_pass_storage
                .get_all_active_pass_by_page_from_history_at_height(
                    page,
                    self.active_address_page_size,
                    block_height,
                )?;
            if active_passes.is_empty() {
                break;
            }

            for pass in &active_passes {
                if let Some(existing_pass_id) =
                    owner_to_pass.insert(pass.owner, pass.inscription_id)
                {
                    let msg = format!(
                        "Duplicate active owner detected: module=balance_monitor, block_height={}, owner={}, existing_pass_id={}, duplicate_pass_id={}",
                        block_height, pass.owner, existing_pass_id, pass.inscription_id
                    );
                    error!("{}", msg);
                    return Err(msg);
                }
            }

            if active_passes.len() < self.active_address_page_size {
                break;
            }

            page += 1;
        }

        let mut active_addresses: Vec<_> = owner_to_pass.into_keys().collect();
        active_addresses.sort_unstable_by_key(|a| a.to_string());
        Ok(active_addresses)
    }

    pub async fn settle_active_balance(
        &self,
        block_height: u32,
    ) -> Result<ActiveBalanceSnapshot, String> {
        let settle_begin = Instant::now();
        let guard_begin = Instant::now();
        self.miner_pass_storage
            .assert_no_data_after_block_height(block_height)?;
        let guard_elapsed_ms = guard_begin.elapsed().as_millis();

        let load_begin = Instant::now();
        let active_addresses = self.load_active_addresses(block_height)?;
        let load_active_addresses_elapsed_ms = load_begin.elapsed().as_millis();
        let active_address_count = u32::try_from(active_addresses.len()).map_err(|e| {
            let msg = format!(
                "Too many active addresses to fit into u32: module=balance_monitor, block_height={}, count={}, error={}",
                block_height,
                active_addresses.len(),
                e
            );
            error!("{}", msg);
            msg
        })?;
        let batch_count = if active_addresses.is_empty() {
            0usize
        } else {
            active_addresses
                .len()
                .div_ceil(self.balance_query_batch_size)
        };

        let rpc_begin = Instant::now();
        let total_balance = self
            .rpc_loader
            .load_total_balance(active_addresses, block_height)
            .await?;
        let rpc_elapsed_ms = rpc_begin.elapsed().as_millis();

        let persist_begin = Instant::now();
        self.miner_pass_storage.upsert_active_balance_snapshot(
            block_height,
            total_balance,
            active_address_count,
        )?;
        let persist_elapsed_ms = persist_begin.elapsed().as_millis();

        let snapshot = ActiveBalanceSnapshot {
            block_height,
            total_balance,
            active_address_count,
        };
        let total_elapsed_ms = settle_begin.elapsed().as_millis();

        info!(
            "Active balance settled: module=balance_monitor, block_height={}, active_address_count={}, batch_count={}, total_balance={}, total_elapsed_ms={}, guard_elapsed_ms={}, load_active_addresses_elapsed_ms={}, rpc_elapsed_ms={}, persist_elapsed_ms={}",
            snapshot.block_height,
            snapshot.active_address_count,
            batch_count,
            snapshot.total_balance,
            total_elapsed_ms,
            guard_elapsed_ms,
            load_active_addresses_elapsed_ms,
            rpc_elapsed_ms,
            persist_elapsed_ms
        );

        Ok(snapshot)
    }
}
