use crate::config::ConfigManagerRef;
use crate::storage::{ActiveBalanceSnapshot, ActiveMinerPassInfo, MinerPassStorageRef};
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActivePassBalance {
    pub inscription_id: InscriptionId,
    pub owner: USDBScriptHash,
    pub block_height: u32,
    pub balance: u64,
    pub delta: i64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveBalanceSettlement {
    pub snapshot: ActiveBalanceSnapshot,
    pub active_pass_balances: Vec<ActivePassBalance>,
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

    fn load_active_passes(&self, block_height: u32) -> Result<Vec<ActiveMinerPassInfo>, String> {
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

        let mut active_passes: Vec<_> = owner_to_pass
            .into_iter()
            .map(|(owner, inscription_id)| ActiveMinerPassInfo {
                inscription_id,
                owner,
            })
            .collect();
        active_passes.sort_unstable_by_key(|v| v.owner.to_string());
        Ok(active_passes)
    }

    pub async fn settle_active_balance_with_details(
        &self,
        block_height: u32,
    ) -> Result<ActiveBalanceSettlement, String> {
        let settle_begin = Instant::now();
        let guard_begin = Instant::now();
        self.miner_pass_storage
            .assert_no_data_after_block_height(block_height)?;
        let guard_elapsed_ms = guard_begin.elapsed().as_millis();

        let load_begin = Instant::now();
        let active_passes = self.load_active_passes(block_height)?;
        let active_addresses: Vec<_> = active_passes.iter().map(|v| v.owner).collect();
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
        let balances = self
            .rpc_loader
            .load_balances(active_addresses, block_height)
            .await?;
        let rpc_elapsed_ms = rpc_begin.elapsed().as_millis();
        let mut balance_by_owner = std::collections::HashMap::with_capacity(balances.len());
        for (owner, balance) in balances {
            if let Some(existing) = balance_by_owner.insert(owner, balance) {
                let msg = format!(
                    "Duplicate owner balance returned by RPC: module=balance_monitor, block_height={}, owner={}, existing_balance={}",
                    block_height, owner, existing.balance
                );
                error!("{}", msg);
                return Err(msg);
            }
        }

        let mut active_pass_balances = Vec::with_capacity(active_passes.len());
        let mut total_balance = 0u64;
        for pass in active_passes {
            let owner_balance = balance_by_owner.remove(&pass.owner).ok_or_else(|| {
                let msg = format!(
                    "Missing owner balance returned by RPC: module=balance_monitor, block_height={}, owner={}, inscription_id={}",
                    block_height, pass.owner, pass.inscription_id
                );
                error!("{}", msg);
                msg
            })?;
            if owner_balance.block_height != block_height {
                let msg = format!(
                    "Unexpected owner balance height returned by RPC: module=balance_monitor, query_height={}, balance_height={}, owner={}, inscription_id={}",
                    block_height, owner_balance.block_height, pass.owner, pass.inscription_id
                );
                error!("{}", msg);
                return Err(msg);
            }

            total_balance = total_balance.saturating_add(owner_balance.balance);
            active_pass_balances.push(ActivePassBalance {
                inscription_id: pass.inscription_id,
                owner: pass.owner,
                block_height,
                balance: owner_balance.balance,
                delta: owner_balance.delta,
            });
        }
        if !balance_by_owner.is_empty() {
            let extra_count = balance_by_owner.len();
            let msg = format!(
                "RPC returned extra owner balances not in active pass set: module=balance_monitor, block_height={}, extra_count={}",
                block_height, extra_count
            );
            error!("{}", msg);
            return Err(msg);
        }

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

        let changed_owner_count = active_pass_balances.iter().filter(|v| v.delta != 0).count();
        info!(
            "Active balance settled: module=balance_monitor, block_height={}, active_address_count={}, changed_owner_count={}, batch_count={}, total_balance={}, total_elapsed_ms={}, guard_elapsed_ms={}, load_active_addresses_elapsed_ms={}, rpc_elapsed_ms={}, persist_elapsed_ms={}",
            snapshot.block_height,
            snapshot.active_address_count,
            changed_owner_count,
            batch_count,
            snapshot.total_balance,
            total_elapsed_ms,
            guard_elapsed_ms,
            load_active_addresses_elapsed_ms,
            rpc_elapsed_ms,
            persist_elapsed_ms
        );

        Ok(ActiveBalanceSettlement {
            snapshot,
            active_pass_balances,
        })
    }

    pub async fn settle_active_balance(
        &self,
        block_height: u32,
    ) -> Result<ActiveBalanceSnapshot, String> {
        let settled = self
            .settle_active_balance_with_details(block_height)
            .await?;
        Ok(settled.snapshot)
    }
}
