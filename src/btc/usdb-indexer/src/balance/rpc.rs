use balance_history::{AddressBalance, RpcClient as BalanceHistoryRpcClient};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::task::JoinSet;
use usdb_util::USDBScriptHash;

pub type BalanceRpcFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

pub trait BalanceRpcBackend: Send + Sync {
    fn get_addresses_balances<'a>(
        &'a self,
        script_hashes: Vec<USDBScriptHash>,
        block_height: Option<u32>,
        block_range: Option<std::ops::Range<u32>>,
    ) -> BalanceRpcFuture<'a, Result<Vec<Vec<AddressBalance>>, String>>;
}

pub struct BalanceHistoryBackend {
    client: BalanceHistoryRpcClient,
}

impl BalanceHistoryBackend {
    pub fn new(rpc_url: &str) -> Result<Self, String> {
        let client = BalanceHistoryRpcClient::new(rpc_url).map_err(|e| {
            let msg = format!("Failed to create BalanceHistoryRpcClient: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self { client })
    }
}

impl BalanceRpcBackend for BalanceHistoryBackend {
    fn get_addresses_balances<'a>(
        &'a self,
        script_hashes: Vec<USDBScriptHash>,
        block_height: Option<u32>,
        block_range: Option<std::ops::Range<u32>>,
    ) -> BalanceRpcFuture<'a, Result<Vec<Vec<AddressBalance>>, String>> {
        Box::pin(async move {
            self.client
                .get_addresses_balances(script_hashes, block_height, block_range)
                .await
        })
    }
}

pub trait BalanceRpcLoader: Send + Sync {
    // Note about semantics:
    // - This API currently uses balance-history `get_address_balance(block_height=H)`.
    // - That upstream API returns the latest record at or before H (not strictly at H).
    // - Therefore returned AddressBalance.block_height may be < H when no mutation happened at H.
    // - Exact-at-height delta semantics are provided by balance-history delta RPC, not this API.
    fn load_balances<'a>(
        &'a self,
        active_addresses: Vec<USDBScriptHash>,
        block_height: u32,
    ) -> BalanceRpcFuture<'a, Result<Vec<(USDBScriptHash, AddressBalance)>, String>>;

    fn load_total_balance<'a>(
        &'a self,
        active_addresses: Vec<USDBScriptHash>,
        block_height: u32,
    ) -> BalanceRpcFuture<'a, Result<u64, String>>;
}

fn collect_batch_balances(
    block_height: u32,
    batch_index: usize,
    addresses: &[USDBScriptHash],
    balances: Vec<Vec<AddressBalance>>,
) -> Result<Vec<(USDBScriptHash, AddressBalance)>, String> {
    if balances.len() != addresses.len() {
        let msg = format!(
            "Address balance batch size mismatch: module=balance_rpc_loader, block_height={}, batch_index={}, requested={}, got={}",
            block_height,
            batch_index,
            addresses.len(),
            balances.len()
        );
        error!("{}", msg);
        return Err(msg);
    }

    let mut entries = Vec::with_capacity(addresses.len());
    for (script_hash, items) in addresses.iter().zip(balances.into_iter()) {
        if items.len() != 1 {
            let msg = format!(
                "Expected exactly one balance item: module=balance_rpc_loader, block_height={}, batch_index={}, script_hash={}, got={}",
                block_height,
                batch_index,
                script_hash,
                items.len()
            );
            error!("{}", msg);
            return Err(msg);
        }

        entries.push((*script_hash, items.into_iter().next().unwrap()));
    }

    Ok(entries)
}

pub struct SerialBalanceLoader<B: BalanceRpcBackend> {
    backend: Arc<B>,
    batch_size: usize,
}

impl<B: BalanceRpcBackend> SerialBalanceLoader<B> {
    pub fn new(backend: Arc<B>, batch_size: usize) -> Result<Self, String> {
        if batch_size == 0 {
            let msg = "Invalid batch_size for serial balance loader: must be > 0".to_string();
            error!("{}", msg);
            return Err(msg);
        }

        Ok(Self {
            backend,
            batch_size,
        })
    }
}

impl<B: BalanceRpcBackend + 'static> BalanceRpcLoader for SerialBalanceLoader<B> {
    fn load_balances<'a>(
        &'a self,
        active_addresses: Vec<USDBScriptHash>,
        block_height: u32,
    ) -> BalanceRpcFuture<'a, Result<Vec<(USDBScriptHash, AddressBalance)>, String>> {
        Box::pin(async move {
            let mut all_entries = Vec::with_capacity(active_addresses.len());
            for (batch_index, batch) in active_addresses.chunks(self.batch_size).enumerate() {
                let batch_addresses = batch.to_vec();
                let balances = self
                    .backend
                    .get_addresses_balances(batch_addresses.clone(), Some(block_height), None)
                    .await?;

                let entries =
                    collect_batch_balances(block_height, batch_index, &batch_addresses, balances)?;
                all_entries.extend(entries);
            }

            Ok(all_entries)
        })
    }

    fn load_total_balance<'a>(
        &'a self,
        active_addresses: Vec<USDBScriptHash>,
        block_height: u32,
    ) -> BalanceRpcFuture<'a, Result<u64, String>> {
        Box::pin(async move {
            let entries = self.load_balances(active_addresses, block_height).await?;
            Ok(entries
                .into_iter()
                .fold(0u64, |acc, (_, item)| acc.saturating_add(item.balance)))
        })
    }
}

struct BatchQueryResult {
    batch_index: usize,
    addresses: Vec<USDBScriptHash>,
    balances: Vec<Vec<AddressBalance>>,
}

pub struct ConcurrentBalanceLoader<B: BalanceRpcBackend> {
    backend: Arc<B>,
    batch_size: usize,
    concurrency: usize,
    timeout_ms: u64,
    max_retries: u32,
}

impl<B: BalanceRpcBackend> ConcurrentBalanceLoader<B> {
    pub fn new(
        backend: Arc<B>,
        batch_size: usize,
        concurrency: usize,
        timeout_ms: u64,
        max_retries: u32,
    ) -> Result<Self, String> {
        if batch_size == 0 {
            let msg = "Invalid batch_size for concurrent balance loader: must be > 0".to_string();
            error!("{}", msg);
            return Err(msg);
        }
        if concurrency == 0 {
            let msg = "Invalid concurrency for concurrent balance loader: must be > 0".to_string();
            error!("{}", msg);
            return Err(msg);
        }
        if timeout_ms == 0 {
            let msg = "Invalid timeout_ms for concurrent balance loader: must be > 0".to_string();
            error!("{}", msg);
            return Err(msg);
        }

        Ok(Self {
            backend,
            batch_size,
            concurrency,
            timeout_ms,
            max_retries,
        })
    }

    async fn query_batch_with_retry(
        backend: Arc<B>,
        block_height: u32,
        batch_index: usize,
        addresses: Vec<USDBScriptHash>,
        timeout_ms: u64,
        max_retries: u32,
    ) -> Result<BatchQueryResult, String> {
        let max_attempts = max_retries.saturating_add(1);

        for attempt in 1..=max_attempts {
            let query_fut =
                backend.get_addresses_balances(addresses.clone(), Some(block_height), None);
            let query_result =
                tokio::time::timeout(Duration::from_millis(timeout_ms), query_fut).await;

            match query_result {
                Ok(Ok(balances)) => {
                    return Ok(BatchQueryResult {
                        batch_index,
                        addresses,
                        balances,
                    });
                }
                Ok(Err(e)) => {
                    if attempt >= max_attempts {
                        let msg = format!(
                            "Balance RPC failed after retries: module=balance_rpc_loader, block_height={}, batch_index={}, attempts={}, error={}",
                            block_height, batch_index, attempt, e
                        );
                        error!("{}", msg);
                        return Err(msg);
                    }

                    warn!(
                        "Balance RPC failed, retrying: module=balance_rpc_loader, block_height={}, batch_index={}, attempt={}, max_attempts={}, error={}",
                        block_height, batch_index, attempt, max_attempts, e
                    );
                }
                Err(_) => {
                    if attempt >= max_attempts {
                        let msg = format!(
                            "Balance RPC timeout after retries: module=balance_rpc_loader, block_height={}, batch_index={}, attempts={}, timeout_ms={}",
                            block_height, batch_index, attempt, timeout_ms
                        );
                        error!("{}", msg);
                        return Err(msg);
                    }

                    warn!(
                        "Balance RPC timeout, retrying: module=balance_rpc_loader, block_height={}, batch_index={}, attempt={}, max_attempts={}, timeout_ms={}",
                        block_height, batch_index, attempt, max_attempts, timeout_ms
                    );
                }
            }

            let backoff_ms = 100u64 * attempt as u64;
            tokio::time::sleep(Duration::from_millis(backoff_ms)).await;
        }

        unreachable!("retry loop must return on success or final failure")
    }
}

impl<B: BalanceRpcBackend + 'static> BalanceRpcLoader for ConcurrentBalanceLoader<B> {
    fn load_balances<'a>(
        &'a self,
        active_addresses: Vec<USDBScriptHash>,
        block_height: u32,
    ) -> BalanceRpcFuture<'a, Result<Vec<(USDBScriptHash, AddressBalance)>, String>> {
        Box::pin(async move {
            let batches: Vec<Vec<USDBScriptHash>> = active_addresses
                .chunks(self.batch_size)
                .map(|chunk| chunk.to_vec())
                .collect();
            if batches.is_empty() {
                return Ok(Vec::new());
            }

            let max_concurrency = self.concurrency.min(batches.len()).max(1);
            let mut join_set = JoinSet::new();
            let mut next_batch_index = 0usize;
            let mut all_entries = Vec::with_capacity(active_addresses.len());

            while next_batch_index < batches.len() && join_set.len() < max_concurrency {
                let batch_index = next_batch_index;
                let addresses = batches[batch_index].clone();
                let backend = Arc::clone(&self.backend);
                let timeout_ms = self.timeout_ms;
                let max_retries = self.max_retries;
                join_set.spawn(async move {
                    Self::query_batch_with_retry(
                        backend,
                        block_height,
                        batch_index,
                        addresses,
                        timeout_ms,
                        max_retries,
                    )
                    .await
                });
                next_batch_index += 1;
            }

            while let Some(task_result) = join_set.join_next().await {
                let batch = task_result.map_err(|e| {
                    let msg = format!(
                        "Balance query task join failed: module=balance_rpc_loader, block_height={}, error={}",
                        block_height, e
                    );
                    error!("{}", msg);
                    msg
                })??;

                let entries = collect_batch_balances(
                    block_height,
                    batch.batch_index,
                    &batch.addresses,
                    batch.balances,
                )?;
                all_entries.extend(entries);

                if next_batch_index < batches.len() {
                    let batch_index = next_batch_index;
                    let addresses = batches[batch_index].clone();
                    let backend = Arc::clone(&self.backend);
                    let timeout_ms = self.timeout_ms;
                    let max_retries = self.max_retries;
                    join_set.spawn(async move {
                        Self::query_batch_with_retry(
                            backend,
                            block_height,
                            batch_index,
                            addresses,
                            timeout_ms,
                            max_retries,
                        )
                        .await
                    });
                    next_batch_index += 1;
                }
            }

            Ok(all_entries)
        })
    }

    fn load_total_balance<'a>(
        &'a self,
        active_addresses: Vec<USDBScriptHash>,
        block_height: u32,
    ) -> BalanceRpcFuture<'a, Result<u64, String>> {
        Box::pin(async move {
            let entries = self.load_balances(active_addresses, block_height).await?;
            Ok(entries
                .into_iter()
                .fold(0u64, |acc, (_, item)| acc.saturating_add(item.balance)))
        })
    }
}

#[cfg(test)]
pub enum MockResponse {
    Immediate(Result<Vec<Vec<AddressBalance>>, String>),
    Delayed {
        delay_ms: u64,
        result: Result<Vec<Vec<AddressBalance>>, String>,
    },
}

#[cfg(test)]
pub struct MockBalanceBackend {
    responses: std::sync::Mutex<std::collections::VecDeque<MockResponse>>,
    calls: std::sync::Mutex<Vec<(usize, Option<u32>)>>,
}

#[cfg(test)]
impl MockBalanceBackend {
    pub fn new(responses: Vec<MockResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(std::collections::VecDeque::from(responses)),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    pub fn call_count(&self) -> usize {
        self.calls.lock().unwrap().len()
    }

    pub fn last_call(&self) -> Option<(usize, Option<u32>)> {
        self.calls.lock().unwrap().last().cloned()
    }
}

#[cfg(test)]
impl BalanceRpcBackend for MockBalanceBackend {
    fn get_addresses_balances<'a>(
        &'a self,
        script_hashes: Vec<USDBScriptHash>,
        block_height: Option<u32>,
        _block_range: Option<std::ops::Range<u32>>,
    ) -> BalanceRpcFuture<'a, Result<Vec<Vec<AddressBalance>>, String>> {
        self.calls
            .lock()
            .unwrap()
            .push((script_hashes.len(), block_height));

        let ret = self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_else(|| MockResponse::Immediate(Err("No mock response queued".to_string())));

        Box::pin(async move {
            match ret {
                MockResponse::Immediate(result) => result,
                MockResponse::Delayed { delay_ms, result } => {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                    result
                }
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::ScriptBuf;
    use std::sync::Arc;
    use usdb_util::ToUSDBScriptHash;

    fn script_hash(tag: u8) -> USDBScriptHash {
        ScriptBuf::from(vec![tag; 32]).to_usdb_script_hash()
    }

    fn make_balance(balance: u64, block_height: u32) -> AddressBalance {
        AddressBalance {
            block_height,
            balance,
            delta: 0,
        }
    }

    fn responses_for_equal_batches(block_height: u32, delayed: bool) -> Vec<MockResponse> {
        let payloads = vec![
            vec![
                vec![make_balance(100, block_height)],
                vec![make_balance(200, block_height)],
            ],
            vec![
                vec![make_balance(300, block_height)],
                vec![make_balance(400, block_height)],
            ],
            vec![
                vec![make_balance(500, block_height)],
                vec![make_balance(600, block_height)],
            ],
        ];

        payloads
            .into_iter()
            .enumerate()
            .map(|(i, result)| {
                if delayed {
                    let delay_ms = match i {
                        0 => 80,
                        1 => 10,
                        _ => 40,
                    };
                    MockResponse::Delayed {
                        delay_ms,
                        result: Ok(result),
                    }
                } else {
                    MockResponse::Immediate(Ok(result))
                }
            })
            .collect()
    }

    fn responses_for_last_partial_batch(block_height: u32, delayed: bool) -> Vec<MockResponse> {
        let payloads = vec![
            vec![
                vec![make_balance(11, block_height)],
                vec![make_balance(22, block_height)],
            ],
            vec![
                vec![make_balance(33, block_height)],
                vec![make_balance(44, block_height)],
            ],
            vec![vec![make_balance(55, block_height)]],
        ];

        payloads
            .into_iter()
            .enumerate()
            .map(|(i, result)| {
                if delayed {
                    let delay_ms = match i {
                        0 => 90,
                        1 => 20,
                        _ => 60,
                    };
                    MockResponse::Delayed {
                        delay_ms,
                        result: Ok(result),
                    }
                } else {
                    MockResponse::Immediate(Ok(result))
                }
            })
            .collect()
    }

    #[tokio::test]
    async fn test_concurrent_loader_matches_serial_loader_on_multi_batches() {
        let block_height = 123u32;
        let addresses = vec![
            script_hash(1),
            script_hash(2),
            script_hash(3),
            script_hash(4),
            script_hash(5),
            script_hash(6),
        ];

        let serial_backend = Arc::new(MockBalanceBackend::new(responses_for_equal_batches(
            block_height,
            false,
        )));
        let concurrent_backend = Arc::new(MockBalanceBackend::new(responses_for_equal_batches(
            block_height,
            true,
        )));

        let serial_loader = SerialBalanceLoader::new(serial_backend.clone(), 2).unwrap();
        let concurrent_loader =
            ConcurrentBalanceLoader::new(concurrent_backend.clone(), 2, 3, 5_000, 0).unwrap();

        let serial_total = serial_loader
            .load_total_balance(addresses.clone(), block_height)
            .await
            .unwrap();
        let concurrent_total = concurrent_loader
            .load_total_balance(addresses, block_height)
            .await
            .unwrap();

        assert_eq!(serial_total, 2_100);
        assert_eq!(concurrent_total, serial_total);
        assert_eq!(serial_backend.call_count(), 3);
        assert_eq!(concurrent_backend.call_count(), 3);
    }

    #[tokio::test]
    async fn test_concurrent_loader_matches_serial_loader_with_partial_last_batch() {
        let block_height = 456u32;
        let addresses = vec![
            script_hash(11),
            script_hash(12),
            script_hash(13),
            script_hash(14),
            script_hash(15),
        ];

        let serial_backend = Arc::new(MockBalanceBackend::new(responses_for_last_partial_batch(
            block_height,
            false,
        )));
        let concurrent_backend = Arc::new(MockBalanceBackend::new(
            responses_for_last_partial_batch(block_height, true),
        ));

        let serial_loader = SerialBalanceLoader::new(serial_backend.clone(), 2).unwrap();
        let concurrent_loader =
            ConcurrentBalanceLoader::new(concurrent_backend.clone(), 2, 3, 5_000, 0).unwrap();

        let serial_total = serial_loader
            .load_total_balance(addresses.clone(), block_height)
            .await
            .unwrap();
        let concurrent_total = concurrent_loader
            .load_total_balance(addresses, block_height)
            .await
            .unwrap();

        assert_eq!(serial_total, 165);
        assert_eq!(concurrent_total, serial_total);
        assert_eq!(serial_backend.call_count(), 3);
        assert_eq!(concurrent_backend.call_count(), 3);
    }
}
