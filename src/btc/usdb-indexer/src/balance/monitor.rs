use crate::config::ConfigManagerRef;
use crate::storage::{ActiveBalanceSnapshot, MinerPassStorageRef};
use balance_history::RpcClient as BalanceHistoryRpcClient;
use ord::InscriptionId;
use usdb_util::USDBScriptHash;

const ACTIVE_ADDRESS_PAGE_SIZE: usize = 1024;
const BALANCE_QUERY_BATCH_SIZE: usize = 1024;

pub struct BalanceMonitor {
    miner_pass_storage: MinerPassStorageRef,
    balance_history_client: BalanceHistoryRpcClient,
}

impl BalanceMonitor {
    pub fn new(
        config: ConfigManagerRef,
        miner_pass_storage: MinerPassStorageRef,
    ) -> Result<Self, String> {
        let balance_history_client = BalanceHistoryRpcClient::new(
            &config.config().balance_history.rpc_url,
        )
        .map_err(|e| {
            let msg = format!("Failed to create BalanceHistoryRpcClient: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self {
            miner_pass_storage,
            balance_history_client,
        })
    }

    fn load_active_addresses(&self, block_height: u32) -> Result<Vec<USDBScriptHash>, String> {
        let mut page = 0usize;
        let mut owner_to_pass = std::collections::HashMap::<USDBScriptHash, InscriptionId>::new();

        loop {
            let active_passes = self
                .miner_pass_storage
                .get_all_active_pass_by_page_at_height(
                    page,
                    ACTIVE_ADDRESS_PAGE_SIZE,
                    block_height,
                )?;
            if active_passes.is_empty() {
                break;
            }

            for pass in active_passes.iter() {
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

            if active_passes.len() < ACTIVE_ADDRESS_PAGE_SIZE {
                break;
            }

            page += 1;
        }

        let mut active_addresses: Vec<_> = owner_to_pass.into_keys().collect();
        active_addresses.sort_unstable_by_key(|a| a.to_string());

        Ok(active_addresses)
    }

    async fn load_total_balance(
        &self,
        active_addresses: &[USDBScriptHash],
        block_height: u32,
    ) -> Result<u64, String> {
        let mut total_balance = 0u64;

        for (batch_index, batch) in active_addresses
            .chunks(BALANCE_QUERY_BATCH_SIZE)
            .enumerate()
        {
            let batch_addresses = batch.to_vec();
            let ret = self
                .balance_history_client
                .get_addresses_balances(batch_addresses.clone(), Some(block_height), None)
                .await?;

            if ret.len() != batch_addresses.len() {
                let msg = format!(
                    "Address balance batch size mismatch: module=balance_monitor, block_height={}, batch_index={}, requested={}, got={}",
                    block_height,
                    batch_index,
                    batch_addresses.len(),
                    ret.len()
                );
                error!("{}", msg);
                return Err(msg);
            }

            for (script_hash, balances) in batch_addresses.into_iter().zip(ret.into_iter()) {
                if balances.len() != 1 {
                    let msg = format!(
                        "Expected exactly one balance item: module=balance_monitor, block_height={}, batch_index={}, script_hash={}, got={}",
                        block_height,
                        batch_index,
                        script_hash,
                        balances.len()
                    );
                    error!("{}", msg);
                    return Err(msg);
                }

                let balance = balances.into_iter().next().unwrap();
                total_balance = total_balance.saturating_add(balance.balance);
            }
        }

        Ok(total_balance)
    }

    pub async fn settle_active_balance(
        &self,
        block_height: u32,
    ) -> Result<ActiveBalanceSnapshot, String> {
        let active_addresses = self.load_active_addresses(block_height)?;
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

        let total_balance = if active_addresses.is_empty() {
            0
        } else {
            self.load_total_balance(&active_addresses, block_height)
                .await?
        };

        self.miner_pass_storage.upsert_active_balance_snapshot(
            block_height,
            total_balance,
            active_address_count,
        )?;

        let snapshot = ActiveBalanceSnapshot {
            block_height,
            total_balance,
            active_address_count,
        };

        info!(
            "Active balance settled: module=balance_monitor, block_height={}, active_address_count={}, total_balance={}",
            snapshot.block_height, snapshot.active_address_count, snapshot.total_balance
        );

        Ok(snapshot)
    }
}
