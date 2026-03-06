use super::content::MinerPassState;
use crate::config::ConfigManagerRef;
use crate::storage::{PassEnergyRecord, PassEnergyStorage};
use balance_history::{AddressBalance, RpcClient as BalanceHistoryRpcClient};
use ord::InscriptionId;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

// 0.001 btc threshold = 100_000 Satoshi
const ENERGY_BALANCE_THRESHOLD: u64 = 100_000; // in Satoshi 0.001 BTC

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassEnergyResult {
    pub energy: u64,
    pub state: MinerPassState,
}

type BalanceProviderFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send + 'a>>;

pub(crate) trait BalanceProvider: Send + Sync {
    fn get_balance_at_height<'a>(
        &'a self,
        address: USDBScriptHash,
        block_height: u32,
    ) -> BalanceProviderFuture<'a, Vec<AddressBalance>>;

    fn get_balance_at_range<'a>(
        &'a self,
        address: USDBScriptHash,
        block_range: Range<u32>,
    ) -> BalanceProviderFuture<'a, Vec<AddressBalance>>;
}

struct RpcBalanceProvider {
    client: BalanceHistoryRpcClient,
}

impl RpcBalanceProvider {
    fn new(rpc_url: &str) -> Result<Self, String> {
        let client = BalanceHistoryRpcClient::new(rpc_url).map_err(|e| {
            let msg = format!("Failed to create Balance History RPC client: {}", e);
            error!("{}", msg);
            msg
        })?;

        Ok(Self { client })
    }
}

impl BalanceProvider for RpcBalanceProvider {
    fn get_balance_at_height<'a>(
        &'a self,
        address: USDBScriptHash,
        block_height: u32,
    ) -> BalanceProviderFuture<'a, Vec<AddressBalance>> {
        Box::pin(async move {
            self.client
                .get_address_balance(address, Some(block_height), None)
                .await
        })
    }

    fn get_balance_at_range<'a>(
        &'a self,
        address: USDBScriptHash,
        block_range: Range<u32>,
    ) -> BalanceProviderFuture<'a, Vec<AddressBalance>> {
        Box::pin(async move {
            self.client
                .get_address_balance(address, None, Some(block_range))
                .await
        })
    }
}

pub struct PassEnergyManager {
    config: ConfigManagerRef,
    storage: PassEnergyStorage,
    balance_provider: Arc<dyn BalanceProvider>,
}

impl PassEnergyManager {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        let storage = PassEnergyStorage::new(&config.data_dir())?;
        let balance_provider =
            Arc::new(RpcBalanceProvider::new(&config.config().balance_history.rpc_url)?);

        Ok(Self::new_with_deps(config, storage, balance_provider))
    }

    pub(crate) fn new_with_deps(
        config: ConfigManagerRef,
        storage: PassEnergyStorage,
        balance_provider: Arc<dyn BalanceProvider>,
    ) -> Self {
        Self {
            config,
            storage,
            balance_provider,
        }
    }

    // Get the balance of an address at a specific block height, which may changed on or before that height
    async fn get_balance_at_height(
        &self,
        address: &USDBScriptHash,
        block_height: u32,
    ) -> Result<AddressBalance, String> {
        let mut balances = self
            .balance_provider
            .get_balance_at_height(*address, block_height)
            .await?;

        assert!(
            balances.len() <= 1,
            "Expected at most one balance entry for address at specific block height {}",
            address
        );
        if let Some(balance) = balances.pop() {
            Ok(balance)
        } else {
            // Should not happen, but return zero balance if not found
            let msg = format!(
                "No balance entry found for address {} at block height {}",
                address, block_height
            );
            warn!("{}", msg);

            Ok(AddressBalance {
                block_height,
                balance: 0,
                delta: 0,
            })
        }
    }

    async fn get_balance_at_range(
        &self,
        address: &USDBScriptHash,
        block_range: std::ops::Range<u32>,
    ) -> Result<Vec<AddressBalance>, String> {
        let balances = self
            .balance_provider
            .get_balance_at_range(*address, block_range)
            .await?;
        Ok(balances)
    }

    // When a new Miner Pass is created, initialize its energy record with zero energy at the block height
    pub async fn on_new_pass(
        &self,
        inscription_id: &InscriptionId,
        owner_address: &USDBScriptHash,
        block_height: u32,
        inherited_energy: u64,
    ) -> Result<(), String> {
        let balance = self
            .get_balance_at_height(owner_address, block_height)
            .await?;

        // Should exactly match the block height when inscription is created
        // Because the utxos must changed at that block height, so we must have balance record at that height
        if balance.block_height != block_height {
            let msg = format!(
                "Balance history not found for owner {} at block height {} when creating new Miner Pass {}",
                owner_address, block_height, inscription_id
            );
            warn!("{}", msg);

            // Should not happen, but continue anyway?
            // return Err(msg);
        }

        let record = PassEnergyRecord {
            inscription_id: inscription_id.clone(),
            block_height,

            state: MinerPassState::Active,
            active_block_height: block_height,
            owner_address: owner_address.clone(),
            owner_balance: balance.balance,
            owner_delta: balance.delta,
            energy: inherited_energy,
        };
        self.storage.insert_pass_energy_record(&record)?;

        info!(
            "New Miner Pass {} created at block height {} for owner {}, initial balance: {}, delta: {}, inherited energy: {}",
            inscription_id, block_height, owner_address, balance.balance, balance.delta, inherited_energy
        );
        Ok(())
    }

    // Get the energy and state of the pass at given block height
    // The record must exist for the pass at the block height
    pub async fn get_pass_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyResult>, String> {
        let ret = self
            .storage
            .get_pass_energy_record(inscription_id, block_height)?;

        let value = ret.map(|v| PassEnergyResult {
            energy: v.energy,
            state: v.state,
        });

        Ok(value)
    }

    // Get the latest energy and state snapshot for the pass at or before block_height.
    pub async fn get_pass_energy_at_or_before(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyResult>, String> {
        let ret = self
            .storage
            .find_last_pass_energy_record(inscription_id, block_height)?;

        let value = ret.map(|v| PassEnergyResult {
            energy: v.energy,
            state: v.state,
        });

        Ok(value)
    }

    // Kernel function to update the energy of a Miner Pass at given block height
    pub async fn update_pass_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<PassEnergyResult, String> {
        // First, get the last energy record for this pass at or before the given block height
        let mut last_record = self
            .storage
            .find_last_pass_energy_record(inscription_id, block_height)?
            .ok_or_else(|| {
                let msg = format!(
                    "No previous energy record found for inscription {} before block height {}",
                    inscription_id, block_height
                );
                error!("{}", msg);
                msg
            })?;
        assert!(
            last_record.block_height <= block_height,
            "Last record block height should be less than or equal to current block height"
        );

        // Check the pass state
        // If the pass is active, we need to update the energy based on owner's balance delta
        // If dormant or consumed, energy remains the same as last record, and we just record return the same energy
        if last_record.state != MinerPassState::Active {
            return Ok(PassEnergyResult {
                energy: last_record.energy,
                state: last_record.state,
            });
        }

        // For active passes, get the owner's balance records between last_record.block_height and block_height: [last_record.block_height + 1, block_height]
        let range = (last_record.block_height + 1)..(block_height + 1);
        let balances = self
            .get_balance_at_range(&last_record.owner_address, range)
            .await?;

        // Update energy based on balance changes records
        for balance_record in balances {
            // Calculate energy bonus between last_record.block_height and balance_record.block_height base on last_record.owner_balance
            // The R is related to the H, H = current block height - miner certificate's activation block height. The larger the H, the larger the R, but the R has an upper limit.
            let r: u32 = balance_record.block_height - last_record.active_block_height;
            assert!(r >= 1, "R should be at least 1");

            let energy_delta = if last_record.owner_balance >= ENERGY_BALANCE_THRESHOLD {
                last_record.owner_balance * 10000 * r as u64
            } else {
                0u64
            };

            let mut new_energy = last_record.energy.saturating_add(energy_delta);

            // The balance increased, so the active_block_height should not change,
            let active_block_height = if balance_record.delta > 0 {
                last_record.active_block_height
            } else {
                // The balance decreased, so we need to update active_block_height to this block height
                balance_record.block_height
            };

            // If the balance decreased, energy decreases by D * 10000 * 6 * 24 * 30 as punishment
            if balance_record.delta < 0 {
                let energy_delta = balance_record.delta.abs() as u64 * 10000 * 6 * 24 * 30;
                new_energy = new_energy.saturating_sub(energy_delta);
            }

            let new_energy = PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: balance_record.block_height,
                state: MinerPassState::Active,
                active_block_height,
                owner_address: last_record.owner_address.clone(),
                owner_balance: balance_record.balance,
                owner_delta: balance_record.delta,
                energy: new_energy,
            };
            self.storage.insert_pass_energy_record(&new_energy)?;
            last_record = new_energy;
        }

        let ret = if last_record.block_height < block_height {
            // No balance changes in between, just calculate energy up to block_height
            // This record should not save to storage, as there is no balance change record at this height
            let energy_delta = if last_record.owner_balance >= ENERGY_BALANCE_THRESHOLD {
                let r = block_height - last_record.active_block_height;
                last_record.owner_balance * 10000 * r as u64
            } else {
                0u64
            };
            let new_energy = last_record.energy.saturating_add(energy_delta);

            PassEnergyResult {
                energy: new_energy,
                state: last_record.state,
            }
        } else {
            PassEnergyResult {
                energy: last_record.energy,
                state: last_record.state,
            }
        };

        Ok(ret)
    }

    // Last update the pass energy when marking dormant
    pub async fn on_pass_dormant(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<(), String> {
        // Finalize active energy first, then persist a Dormant snapshot at block_height.
        let finalized = self
            .update_pass_energy(&inscription_id, block_height)
            .await?;

        let last_record = self
            .storage
            .find_last_pass_energy_record(inscription_id, block_height)?
            .ok_or_else(|| {
                let msg = format!(
                    "No energy record found for dormant transition: inscription_id={}, block_height={}",
                    inscription_id, block_height
                );
                error!("{}", msg);
                msg
            })?;

        let dormant_record = PassEnergyRecord {
            inscription_id: inscription_id.clone(),
            block_height,
            state: MinerPassState::Dormant,
            active_block_height: last_record.active_block_height,
            owner_address: last_record.owner_address.clone(),
            owner_balance: last_record.owner_balance,
            owner_delta: if last_record.block_height == block_height {
                last_record.owner_delta
            } else {
                0
            },
            energy: finalized.energy,
        };
        self.storage.insert_pass_energy_record(&dormant_record)?;

        let stored = self.get_pass_energy(inscription_id, block_height).await?;
        let expected = PassEnergyResult {
            energy: finalized.energy,
            state: MinerPassState::Dormant,
        };
        if stored.as_ref() != Some(&expected) {
            let msg = format!(
                "Dormant energy snapshot mismatch: inscription_id={}, block_height={}, stored={:?}, expected={:?}",
                inscription_id, block_height, stored, expected
            );
            error!("{}", msg);
            return Err(msg);
        }

        info!(
            "Miner Pass {} marked as Dormant at block height {}, final energy: {}",
            inscription_id, block_height, finalized.energy
        );

        Ok(())
    }

    // Clear energy to zero on consumed
    pub fn on_pass_consumed(
        &self,
        inscription_id: &InscriptionId,
        owner_address: &USDBScriptHash,
        block_height: u32,
    ) -> Result<(), String> {
        // Insert a new record with zero energy and state consumed
        let record = PassEnergyRecord {
            inscription_id: inscription_id.clone(),
            block_height,
            state: MinerPassState::Consumed,
            active_block_height: block_height,
            owner_address: owner_address.clone(),
            owner_balance: 0,
            owner_delta: 0,
            energy: 0,
        };
        self.storage.insert_pass_energy_record(&record)?;

        Ok(())
    }
}

pub type PassEnergyManagerRef = std::sync::Arc<PassEnergyManager>;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigManager;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{ScriptBuf, Txid};
    use std::collections::HashMap;
    use std::future::Future;
    use std::pin::Pin;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::ToUSDBScriptHash;

    fn test_root_dir(test_name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("usdb_indexer_energy_test_{}_{}", test_name, nanos))
    }

    fn test_script_hash(tag: u8) -> USDBScriptHash {
        let script = ScriptBuf::from(vec![tag; 32]);
        script.to_usdb_script_hash()
    }

    fn test_inscription_id(tag: u8, index: u32) -> InscriptionId {
        InscriptionId {
            txid: Txid::from_slice(&[tag; 32]).unwrap(),
            index,
        }
    }

    struct TimelineBalanceProvider {
        timelines: HashMap<USDBScriptHash, Vec<AddressBalance>>,
    }

    impl TimelineBalanceProvider {
        fn new(timelines: HashMap<USDBScriptHash, Vec<AddressBalance>>) -> Self {
            let mut normalized = HashMap::new();
            for (addr, mut points) in timelines {
                points.sort_unstable_by_key(|v| v.block_height);
                normalized.insert(addr, points);
            }
            Self {
                timelines: normalized,
            }
        }
    }

    impl BalanceProvider for TimelineBalanceProvider {
        fn get_balance_at_height<'a>(
            &'a self,
            address: USDBScriptHash,
            block_height: u32,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
            Box::pin(async move {
                let ret = self
                    .timelines
                    .get(&address)
                    .and_then(|points| {
                        points
                            .iter()
                            .rev()
                            .find(|v| v.block_height <= block_height)
                            .cloned()
                    })
                    .map(|v| vec![v])
                    .unwrap_or_default();
                Ok(ret)
            })
        }

        fn get_balance_at_range<'a>(
            &'a self,
            address: USDBScriptHash,
            block_range: Range<u32>,
        ) -> Pin<Box<dyn Future<Output = Result<Vec<AddressBalance>, String>> + Send + 'a>> {
            Box::pin(async move {
                let ret = self
                    .timelines
                    .get(&address)
                    .map(|points| {
                        points
                            .iter()
                            .filter(|v| block_range.contains(&v.block_height))
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                Ok(ret)
            })
        }
    }

    fn expected_energy_by_formula(
        initial_block_height: u32,
        initial_balance: u64,
        inherited_energy: u64,
        timeline_points: &[AddressBalance],
        target_height: u32,
    ) -> u64 {
        assert!(target_height >= initial_block_height);
        let mut energy = inherited_energy;
        let mut owner_balance = initial_balance;
        let mut active_block_height = initial_block_height;
        let mut last_processed_height = initial_block_height;

        for point in timeline_points {
            if point.block_height <= initial_block_height {
                continue;
            }
            if point.block_height > target_height {
                break;
            }

            let r = point.block_height - active_block_height;
            if owner_balance >= ENERGY_BALANCE_THRESHOLD {
                let delta = owner_balance * 10000 * r as u64;
                energy = energy.saturating_add(delta);
            }

            if point.delta < 0 {
                let penalty = point.delta.abs() as u64 * 10000 * 6 * 24 * 30;
                energy = energy.saturating_sub(penalty);
                active_block_height = point.block_height;
            }

            owner_balance = point.balance;
            last_processed_height = point.block_height;
        }

        if last_processed_height < target_height && owner_balance >= ENERGY_BALANCE_THRESHOLD {
            let r = target_height - active_block_height;
            let delta = owner_balance * 10000 * r as u64;
            energy = energy.saturating_add(delta);
        }

        energy
    }

    async fn setup_manager_with_timeline(
        test_name: &str,
        owner: USDBScriptHash,
        timeline_points: Vec<AddressBalance>,
        initial_height: u32,
        inherited_energy: u64,
    ) -> (PathBuf, PassEnergyManager, InscriptionId) {
        let root_dir = test_root_dir(test_name);
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let storage = PassEnergyStorage::new(&config.data_dir()).unwrap();

        let mut timelines = HashMap::new();
        timelines.insert(owner, timeline_points.clone());
        let provider = Arc::new(TimelineBalanceProvider::new(timelines));
        let manager = PassEnergyManager::new_with_deps(config, storage, provider);

        let inscription_id = test_inscription_id(9, 0);
        manager
            .on_new_pass(&inscription_id, &owner, initial_height, inherited_energy)
            .await
            .unwrap();

        (root_dir, manager, inscription_id)
    }

    #[tokio::test]
    async fn test_get_pass_energy_at_or_before_returns_latest_snapshot() {
        let root_dir = test_root_dir("at_or_before");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(1, 0);
        let owner = test_script_hash(2);

        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Dormant,
                active_block_height: 90,
                owner_address: owner,
                owner_balance: 1_000,
                owner_delta: 10,
                energy: 111,
            })
            .unwrap();
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 120,
                state: MinerPassState::Dormant,
                active_block_height: 90,
                owner_address: owner,
                owner_balance: 1_500,
                owner_delta: 20,
                energy: 222,
            })
            .unwrap();

        let e115 = manager
            .get_pass_energy_at_or_before(&inscription_id, 115)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e115.state, MinerPassState::Dormant);
        assert_eq!(e115.energy, 111);

        let e120 = manager
            .get_pass_energy_at_or_before(&inscription_id, 120)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e120.state, MinerPassState::Dormant);
        assert_eq!(e120.energy, 222);

        let e80 = manager
            .get_pass_energy_at_or_before(&inscription_id, 80)
            .await
            .unwrap();
        assert!(e80.is_none());

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_energy_timeline_no_change_growth_matches_formula() {
        let owner = test_script_hash(10);
        let timeline = vec![AddressBalance {
            block_height: 100,
            balance: 200_000,
            delta: 100,
        }];
        let (root_dir, manager, inscription_id) =
            setup_manager_with_timeline("timeline_no_change", owner, timeline.clone(), 100, 0)
                .await;

        for h in 100..=105 {
            let got = manager.update_pass_energy(&inscription_id, h).await.unwrap();
            let expected = expected_energy_by_formula(100, 200_000, 0, &timeline, h);
            assert_eq!(got.state, MinerPassState::Active);
            assert_eq!(got.energy, expected, "height={}", h);
        }

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_energy_timeline_below_threshold_has_no_growth() {
        let owner = test_script_hash(11);
        let timeline = vec![AddressBalance {
            block_height: 100,
            balance: 99_999,
            delta: 100,
        }];
        let (root_dir, manager, inscription_id) =
            setup_manager_with_timeline("timeline_below_threshold", owner, timeline.clone(), 100, 0)
                .await;

        for h in 100..=110 {
            let got = manager.update_pass_energy(&inscription_id, h).await.unwrap();
            let expected = expected_energy_by_formula(100, 99_999, 0, &timeline, h);
            assert_eq!(got.state, MinerPassState::Active);
            assert_eq!(got.energy, expected, "height={}", h);
        }

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_energy_timeline_mixed_deltas_matches_formula_and_is_idempotent() {
        let owner = test_script_hash(12);
        let timeline = vec![
            AddressBalance {
                block_height: 100,
                balance: 200_000,
                delta: 100,
            },
            AddressBalance {
                block_height: 103,
                balance: 210_000,
                delta: 10_000,
            },
            AddressBalance {
                block_height: 106,
                balance: 190_000,
                delta: -20_000,
            },
            AddressBalance {
                block_height: 108,
                balance: 220_000,
                delta: 30_000,
            },
        ];
        let (root_dir, manager, inscription_id) =
            setup_manager_with_timeline("timeline_mixed_deltas", owner, timeline.clone(), 100, 0)
                .await;

        for h in 100..=110 {
            let got_first = manager.update_pass_energy(&inscription_id, h).await.unwrap();
            let got_second = manager.update_pass_energy(&inscription_id, h).await.unwrap();
            let expected = expected_energy_by_formula(100, 200_000, 0, &timeline, h);

            assert_eq!(got_first.state, MinerPassState::Active);
            assert_eq!(got_first.energy, expected, "first_call_height={}", h);
            assert_eq!(got_second.energy, expected, "second_call_height={}", h);
        }

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_energy_timeline_dormant_then_consumed_freezes_and_zeroes_energy() {
        let owner = test_script_hash(13);
        let timeline = vec![
            AddressBalance {
                block_height: 100,
                balance: 200_000,
                delta: 100,
            },
            AddressBalance {
                block_height: 103,
                balance: 210_000,
                delta: 10_000,
            },
        ];
        let (root_dir, manager, inscription_id) =
            setup_manager_with_timeline("timeline_dormant_consumed", owner, timeline.clone(), 100, 0)
                .await;

        let expected_at_108 = expected_energy_by_formula(100, 200_000, 0, &timeline, 108);
        manager.on_pass_dormant(&inscription_id, 108).await.unwrap();

        for h in 108..=112 {
            let got = manager.update_pass_energy(&inscription_id, h).await.unwrap();
            assert_eq!(got.state, MinerPassState::Dormant);
            assert_eq!(got.energy, expected_at_108, "height={}", h);
        }

        manager.on_pass_consumed(&inscription_id, &owner, 112).unwrap();
        let consumed = manager.get_pass_energy(&inscription_id, 112).await.unwrap().unwrap();
        assert_eq!(consumed.state, MinerPassState::Consumed);
        assert_eq!(consumed.energy, 0);

        std::fs::remove_dir_all(root_dir).unwrap();
    }
}
