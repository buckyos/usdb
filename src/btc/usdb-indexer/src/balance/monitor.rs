use crate::config::ConfigManagerRef;
use crate::storage::{ActiveBalanceSnapshot, MinerPassStorageRef};
use balance_history::{AddressBalance, RpcClient as BalanceHistoryRpcClient};
use ord::InscriptionId;
use std::future::Future;
use std::pin::Pin;
use usdb_util::USDBScriptHash;

const ACTIVE_ADDRESS_PAGE_SIZE: usize = 1024;
const BALANCE_QUERY_BATCH_SIZE: usize = 1024;

pub(crate) trait BalanceHistoryClient {
    fn get_addresses_balances<'a>(
        &'a self,
        script_hashes: Vec<USDBScriptHash>,
        block_height: Option<u32>,
        block_range: Option<std::ops::Range<u32>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<AddressBalance>>, String>> + Send + 'a>>;
}

impl BalanceHistoryClient for BalanceHistoryRpcClient {
    fn get_addresses_balances<'a>(
        &'a self,
        script_hashes: Vec<USDBScriptHash>,
        block_height: Option<u32>,
        block_range: Option<std::ops::Range<u32>>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<AddressBalance>>, String>> + Send + 'a>> {
        Box::pin(async move {
            self.get_addresses_balances(script_hashes, block_height, block_range)
                .await
        })
    }
}

pub struct BalanceMonitor<C: BalanceHistoryClient = BalanceHistoryRpcClient> {
    miner_pass_storage: MinerPassStorageRef,
    balance_history_client: C,
}

impl BalanceMonitor<BalanceHistoryRpcClient> {
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
}

impl<C: BalanceHistoryClient> BalanceMonitor<C> {
    #[cfg(test)]
    fn new_with_client(miner_pass_storage: MinerPassStorageRef, balance_history_client: C) -> Self {
        Self {
            miner_pass_storage,
            balance_history_client,
        }
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
        self.miner_pass_storage
            .assert_no_data_after_block_height(block_height)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::MinerPassState;
    use crate::storage::{MinerPassInfo, MinerPassStorage};
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{OutPoint, ScriptBuf, Txid};
    use ord::InscriptionId;
    use ordinals::SatPoint;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

    struct MockBalanceHistoryClient {
        responses: Mutex<VecDeque<Result<Vec<Vec<AddressBalance>>, String>>>,
        calls: Mutex<Vec<(usize, Option<u32>)>>,
    }

    impl MockBalanceHistoryClient {
        fn new(responses: Vec<Result<Vec<Vec<AddressBalance>>, String>>) -> Self {
            Self {
                responses: Mutex::new(VecDeque::from(responses)),
                calls: Mutex::new(Vec::new()),
            }
        }

        fn call_count(&self) -> usize {
            self.calls.lock().unwrap().len()
        }

        fn last_call(&self) -> Option<(usize, Option<u32>)> {
            self.calls.lock().unwrap().last().cloned()
        }
    }

    impl BalanceHistoryClient for Arc<MockBalanceHistoryClient> {
        fn get_addresses_balances<'a>(
            &'a self,
            script_hashes: Vec<USDBScriptHash>,
            block_height: Option<u32>,
            _block_range: Option<std::ops::Range<u32>>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<Vec<AddressBalance>>, String>> + Send + 'a>>
        {
            self.calls
                .lock()
                .unwrap()
                .push((script_hashes.len(), block_height));

            let ret = self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| Err("No mock response queued".to_string()));
            Box::pin(async move { ret })
        }
    }

    fn test_data_dir(tag: &str) -> std::path::PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("usdb_balance_monitor_{tag}_{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script_hash(tag: u8) -> USDBScriptHash {
        ScriptBuf::from(vec![tag; 32]).to_usdb_script_hash()
    }

    fn inscription_id(tag: u8, index: u32) -> InscriptionId {
        InscriptionId {
            txid: Txid::from_slice(&[tag; 32]).unwrap(),
            index,
        }
    }

    fn satpoint(tag: u8, vout: u32, offset: u64) -> SatPoint {
        SatPoint {
            outpoint: OutPoint {
                txid: Txid::from_slice(&[tag; 32]).unwrap(),
                vout,
            },
            offset,
        }
    }

    fn make_pass(
        tag: u8,
        index: u32,
        owner: USDBScriptHash,
        mint_block_height: u32,
    ) -> MinerPassInfo {
        MinerPassInfo {
            inscription_id: inscription_id(tag, index),
            inscription_number: index as i32 + 1,
            mint_txid: Txid::from_slice(&[tag.wrapping_add(1); 32]).unwrap(),
            mint_block_height,
            mint_owner: owner,
            satpoint: satpoint(tag, index, 0),
            eth_main: "0x1111111111111111111111111111111111111111".to_string(),
            eth_collab: None,
            prev: Vec::new(),
            owner,
            state: MinerPassState::Active,
        }
    }

    #[tokio::test]
    async fn test_settle_active_balance_empty_active_addresses() {
        let dir = test_data_dir("empty");
        let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
        let mock = Arc::new(MockBalanceHistoryClient::new(vec![]));
        let monitor = BalanceMonitor::new_with_client(storage.clone(), mock.clone());

        let snapshot = monitor.settle_active_balance(100).await.unwrap();
        assert_eq!(snapshot.block_height, 100);
        assert_eq!(snapshot.active_address_count, 0);
        assert_eq!(snapshot.total_balance, 0);

        let stored = storage.get_active_balance_snapshot(100).unwrap().unwrap();
        assert_eq!(stored, snapshot);
        assert_eq!(mock.call_count(), 0);

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_settle_active_balance_sum_and_snapshot_written() {
        let dir = test_data_dir("sum");
        let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
        storage
            .add_new_mint_pass(&make_pass(11, 0, script_hash(1), 90))
            .unwrap();
        storage
            .add_new_mint_pass(&make_pass(12, 1, script_hash(2), 91))
            .unwrap();

        let mock = Arc::new(MockBalanceHistoryClient::new(vec![Ok(vec![
            vec![AddressBalance {
                block_height: 100,
                balance: 1_500,
                delta: 10,
            }],
            vec![AddressBalance {
                block_height: 100,
                balance: 2_500,
                delta: 20,
            }],
        ])]));
        let monitor = BalanceMonitor::new_with_client(storage.clone(), mock.clone());

        let snapshot = monitor.settle_active_balance(100).await.unwrap();
        assert_eq!(snapshot.active_address_count, 2);
        assert_eq!(snapshot.total_balance, 4_000);

        let call = mock.last_call().unwrap();
        assert_eq!(call.0, 2);
        assert_eq!(call.1, Some(100));

        let stored = storage.get_active_balance_snapshot(100).unwrap().unwrap();
        assert_eq!(stored.total_balance, 4_000);
        assert_eq!(stored.active_address_count, 2);

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_settle_active_balance_rpc_batch_size_mismatch() {
        let dir = test_data_dir("batch_mismatch");
        let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
        storage
            .add_new_mint_pass(&make_pass(21, 0, script_hash(3), 80))
            .unwrap();

        let mock = Arc::new(MockBalanceHistoryClient::new(vec![Ok(vec![])]));
        let monitor = BalanceMonitor::new_with_client(storage.clone(), mock);

        let err = monitor.settle_active_balance(100).await.unwrap_err();
        assert!(err.contains("Address balance batch size mismatch"));
        assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_settle_active_balance_balance_item_count_mismatch() {
        let dir = test_data_dir("item_mismatch");
        let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
        storage
            .add_new_mint_pass(&make_pass(31, 0, script_hash(4), 80))
            .unwrap();

        let mock = Arc::new(MockBalanceHistoryClient::new(vec![Ok(vec![vec![]])]));
        let monitor = BalanceMonitor::new_with_client(storage.clone(), mock);

        let err = monitor.settle_active_balance(100).await.unwrap_err();
        assert!(err.contains("Expected exactly one balance item"));
        assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_settle_active_balance_fail_on_future_data_guard() {
        let dir = test_data_dir("future_data_guard");
        let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
        storage
            .add_new_mint_pass(&make_pass(41, 0, script_hash(5), 120))
            .unwrap();

        let mock = Arc::new(MockBalanceHistoryClient::new(vec![]));
        let monitor = BalanceMonitor::new_with_client(storage.clone(), mock.clone());

        let err = monitor.settle_active_balance(100).await.unwrap_err();
        assert!(err.contains("Future miner pass data exists"));
        assert_eq!(mock.call_count(), 0);
        assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
