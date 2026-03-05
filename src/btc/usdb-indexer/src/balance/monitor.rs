use crate::config::ConfigManagerRef;
use crate::storage::{ActiveBalanceSnapshot, MinerPassStorageRef};
use ord::InscriptionId;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

use super::{
    BalanceHistoryBackend, BalanceRpcLoader, ConcurrentBalanceLoader, SerialBalanceLoader,
};

pub struct BalanceMonitor {
    miner_pass_storage: MinerPassStorageRef,
    rpc_loader: Arc<dyn BalanceRpcLoader>,
    active_address_page_size: usize,
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
        })
    }

    #[cfg(test)]
    fn new_with_loader(
        miner_pass_storage: MinerPassStorageRef,
        rpc_loader: Arc<dyn BalanceRpcLoader>,
        active_address_page_size: usize,
    ) -> Self {
        assert!(active_address_page_size > 0);
        Self {
            miner_pass_storage,
            rpc_loader,
            active_address_page_size,
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

        let total_balance = self
            .rpc_loader
            .load_total_balance(active_addresses, block_height)
            .await?;

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
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

    use super::super::{
        ConcurrentBalanceLoader, MockBalanceBackend, MockResponse, SerialBalanceLoader,
    };

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
        let backend = Arc::new(MockBalanceBackend::new(vec![]));
        let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
        let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024);

        let snapshot = monitor.settle_active_balance(100).await.unwrap();
        assert_eq!(snapshot.block_height, 100);
        assert_eq!(snapshot.active_address_count, 0);
        assert_eq!(snapshot.total_balance, 0);

        let stored = storage.get_active_balance_snapshot(100).unwrap().unwrap();
        assert_eq!(stored, snapshot);
        assert_eq!(backend.call_count(), 0);

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

        let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
            vec![
                vec![balance_history::AddressBalance {
                    block_height: 100,
                    balance: 1_500,
                    delta: 10,
                }],
                vec![balance_history::AddressBalance {
                    block_height: 100,
                    balance: 2_500,
                    delta: 20,
                }],
            ],
        ))]));
        let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
        let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024);

        let snapshot = monitor.settle_active_balance(100).await.unwrap();
        assert_eq!(snapshot.active_address_count, 2);
        assert_eq!(snapshot.total_balance, 4_000);

        let call = backend.last_call().unwrap();
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

        let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
            vec![],
        ))]));
        let loader = Arc::new(SerialBalanceLoader::new(backend, 1024).unwrap());
        let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024);

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

        let backend = Arc::new(MockBalanceBackend::new(vec![MockResponse::Immediate(Ok(
            vec![vec![]],
        ))]));
        let loader = Arc::new(SerialBalanceLoader::new(backend, 1024).unwrap());
        let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024);

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

        let backend = Arc::new(MockBalanceBackend::new(vec![]));
        let loader = Arc::new(SerialBalanceLoader::new(backend.clone(), 1024).unwrap());
        let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024);

        let err = monitor.settle_active_balance(100).await.unwrap_err();
        assert!(err.contains("Future miner pass data exists"));
        assert_eq!(backend.call_count(), 0);
        assert!(storage.get_active_balance_snapshot(100).unwrap().is_none());

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_settle_active_balance_retry_on_rpc_error() {
        let dir = test_data_dir("retry_rpc_error");
        let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
        storage
            .add_new_mint_pass(&make_pass(51, 0, script_hash(6), 80))
            .unwrap();

        let backend = Arc::new(MockBalanceBackend::new(vec![
            MockResponse::Immediate(Err("temporary rpc failure".to_string())),
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 3_000,
                delta: 30,
            }]])),
        ]));
        let loader =
            Arc::new(ConcurrentBalanceLoader::new(backend.clone(), 1024, 1, 10_000, 1).unwrap());
        let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024);

        let snapshot = monitor.settle_active_balance(100).await.unwrap();
        assert_eq!(snapshot.total_balance, 3_000);
        assert_eq!(backend.call_count(), 2);

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn test_settle_active_balance_retry_on_timeout() {
        let dir = test_data_dir("retry_timeout");
        let storage = Arc::new(MinerPassStorage::new(&dir).unwrap());
        storage
            .add_new_mint_pass(&make_pass(61, 0, script_hash(7), 80))
            .unwrap();

        let backend = Arc::new(MockBalanceBackend::new(vec![
            MockResponse::Delayed {
                delay_ms: 50,
                result: Ok(vec![vec![balance_history::AddressBalance {
                    block_height: 100,
                    balance: 1_000,
                    delta: 10,
                }]]),
            },
            MockResponse::Immediate(Ok(vec![vec![balance_history::AddressBalance {
                block_height: 100,
                balance: 2_000,
                delta: 20,
            }]])),
        ]));
        let loader =
            Arc::new(ConcurrentBalanceLoader::new(backend.clone(), 1024, 1, 10, 1).unwrap());
        let monitor = BalanceMonitor::new_with_loader(storage.clone(), loader, 1024);

        let snapshot = monitor.settle_active_balance(100).await.unwrap();
        assert_eq!(snapshot.total_balance, 2_000);
        assert_eq!(backend.call_count(), 2);

        drop(monitor);
        drop(storage);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
