use super::content::MinerPassState;
use super::energy_formula::{calc_growth_delta, calc_penalty_from_delta};
use crate::config::ConfigManagerRef;
use crate::storage::{PassEnergyRecord, PassEnergyStorage, PassEnergyValue};
use balance_history::{AddressBalance, RpcClient as BalanceHistoryRpcClient};
use ord::InscriptionId;
use std::future::Future;
use std::ops::Range;
use std::pin::Pin;
use std::sync::Arc;
use usdb_util::USDBScriptHash;

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

fn calc_incremental_growth(
    owner_balance: u64,
    active_block_height: u32,
    from_block_height: u32,
    to_block_height: u32,
) -> u64 {
    if to_block_height <= from_block_height {
        return 0;
    }

    let growth_at_to = calc_growth_delta(
        owner_balance,
        to_block_height.saturating_sub(active_block_height),
    );
    let growth_at_from = calc_growth_delta(
        owner_balance,
        from_block_height.saturating_sub(active_block_height),
    );
    growth_at_to.saturating_sub(growth_at_from)
}

pub struct PassEnergyManager {
    config: ConfigManagerRef,
    storage: PassEnergyStorage,
    balance_provider: Arc<dyn BalanceProvider>,
    #[cfg(test)]
    force_strict_settle_consistency_for_test: std::sync::atomic::AtomicBool,
}

impl PassEnergyManager {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        let storage = PassEnergyStorage::new(&config.data_dir())?;
        let balance_provider = Arc::new(RpcBalanceProvider::new(
            &config.config().balance_history.rpc_url,
        )?);

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
            #[cfg(test)]
            force_strict_settle_consistency_for_test: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn strict_settle_consistency_enabled(&self) -> bool {
        #[cfg(test)]
        {
            self.force_strict_settle_consistency_for_test
                .load(std::sync::atomic::Ordering::Relaxed)
        }
        #[cfg(not(test))]
        {
            true
        }
    }

    #[cfg(test)]
    pub fn set_force_strict_settle_consistency_for_test(&self, enabled: bool) {
        self.force_strict_settle_consistency_for_test
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    #[cfg(test)]
    pub fn get_pending_block_height_for_test(&self) -> Result<Option<u32>, String> {
        self.storage.get_pending_block_height()
    }

    #[cfg(test)]
    pub fn get_synced_block_height_for_test(&self) -> Result<Option<u32>, String> {
        self.storage.get_synced_block_height()
    }

    pub fn begin_block_sync(&self, block_height: u32) -> Result<(), String> {
        // Mark this block as pending before any energy writes.
        // If process crashes mid-block, startup reconcile will roll back from this height.
        if let Some(pending_height) = self.storage.get_pending_block_height()? {
            let msg = format!(
                "Previous pending energy block sync found before starting new block: pending_height={}, new_block_height={}",
                pending_height, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        self.storage.set_pending_block_height(block_height)?;
        Ok(())
    }

    pub fn finalize_block_sync(&self, block_height: u32) -> Result<(), String> {
        // Finalize pending -> synced in one atomic RocksDB batch.
        let pending_height = self.storage.get_pending_block_height()?;
        if pending_height != Some(block_height) {
            let msg = format!(
                "Energy pending block mismatch when finalizing block sync: pending_height={:?}, finalize_height={}",
                pending_height, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        self.storage.finalize_block_sync(block_height)?;
        Ok(())
    }

    pub fn abort_pending_block_sync(&self, block_height: u32) -> Result<(), String> {
        // Use this when block processing fails before finalize_block_sync succeeds.
        // At this point energy writes for the block may already exist, but the pending marker
        // still proves the block never became a durable energy commit, so we can safely
        // delete all records from this height and clear the pending marker.
        let pending_height = self.storage.get_pending_block_height()?;
        if pending_height != Some(block_height) {
            let msg = format!(
                "Energy pending block mismatch when aborting block sync: pending_height={:?}, abort_height={}",
                pending_height, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        warn!(
            "Aborting pending energy block sync: module=energy, block_height={}",
            block_height
        );
        self.storage.clear_records_from_height(block_height)?;
        self.storage.clear_pending_block_height()?;
        Ok(())
    }

    pub fn rollback_to_pass_synced_height(&self, pass_synced_height: u32) -> Result<(), String> {
        // Use this when energy finalize already succeeded, but the enclosing block commit did not
        // become durable on the pass/SQLite side. In that window the pending marker is gone, so the
        // only safe recovery is to realign energy state against the last durable pass synced height.
        warn!(
            "Rolling back energy state to pass synced height: module=energy, pass_synced_height={}",
            pass_synced_height
        );
        self.reconcile_with_pass_synced_height(pass_synced_height)
    }

    pub fn reconcile_with_pass_synced_height(&self, pass_synced_height: u32) -> Result<(), String> {
        // Step 1: recover incomplete block sync attempts from pending marker.
        if let Some(pending_height) = self.storage.get_pending_block_height()? {
            warn!(
                "Recovering stale pending energy sync marker: module=energy, pending_height={}, pass_synced_height={}",
                pending_height, pass_synced_height
            );
            self.storage.clear_records_from_height(pending_height)?;
            self.storage.clear_pending_block_height()?;
        }

        // Step 2: truncate any future energy records beyond pass synced height.
        if let Some(max_height) = self.storage.get_max_record_block_height()? {
            if max_height > pass_synced_height {
                warn!(
                    "Detected future energy records beyond pass synced height, truncating: module=energy, max_energy_height={}, pass_synced_height={}",
                    max_height, pass_synced_height
                );
                self.storage
                    .clear_records_from_height(pass_synced_height.saturating_add(1))?;
            }
        }

        // Step 3: reconcile synced height marker.
        match self.storage.get_synced_block_height()? {
            Some(energy_synced_height) if energy_synced_height > pass_synced_height => {
                warn!(
                    "Energy synced height is ahead of pass synced height, correcting marker: module=energy, energy_synced_height={}, pass_synced_height={}",
                    energy_synced_height, pass_synced_height
                );
                self.storage.set_synced_block_height(pass_synced_height)?;
            }
            Some(energy_synced_height) if energy_synced_height < pass_synced_height => {
                let msg = format!(
                    "Energy synced height is behind pass synced height: module=energy, energy_synced_height={}, pass_synced_height={}. Please clear energy storage and resync from genesis.",
                    energy_synced_height, pass_synced_height
                );
                error!("{}", msg);
                return Err(msg);
            }
            Some(_) => {}
            None => {
                warn!(
                    "Energy synced height marker is missing, initializing from pass synced height: module=energy, pass_synced_height={}",
                    pass_synced_height
                );
                self.storage.set_synced_block_height(pass_synced_height)?;
            }
        }

        Ok(())
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
            inscription_id,
            block_height,
            owner_address,
            balance.balance,
            balance.delta,
            inherited_energy
        );
        Ok(())
    }

    // Get pass energy/state at query block height.
    // This query always returns deterministic energy at the target height:
    // - It reads latest record <= query height.
    // - For active passes, it projects growth to query height.
    pub async fn get_pass_energy(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyResult>, String> {
        let record = self.get_pass_energy_record_at_or_before(inscription_id, block_height)?;
        Ok(record.map(|r| self.project_energy_record_no_balance_change(&r, block_height)))
    }

    // Record-query API: exact stored record at block height.
    pub fn get_pass_energy_record_exact(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyRecord>, String> {
        let value = self
            .storage
            .get_pass_energy_record(inscription_id, block_height)?;
        Ok(value.map(|v: PassEnergyValue| PassEnergyRecord {
            inscription_id: inscription_id.clone(),
            block_height,
            state: v.state,
            active_block_height: v.active_block_height,
            owner_address: v.owner_address,
            owner_balance: v.owner_balance,
            owner_delta: v.owner_delta,
            energy: v.energy,
        }))
    }

    // Record-query API: latest stored record at or before block height.
    pub fn get_pass_energy_record_at_or_before(
        &self,
        inscription_id: &InscriptionId,
        block_height: u32,
    ) -> Result<Option<PassEnergyRecord>, String> {
        self.storage
            .find_last_pass_energy_record(inscription_id, block_height)
    }

    // Project one stored energy record to query height under the assumption that
    // no additional owner-balance change records were written after record.block_height.
    pub fn project_energy_record_no_balance_change(
        &self,
        record: &PassEnergyRecord,
        query_block_height: u32,
    ) -> PassEnergyResult {
        if query_block_height <= record.block_height || record.state != MinerPassState::Active {
            return PassEnergyResult {
                energy: record.energy,
                state: record.state.clone(),
            };
        }

        let growth_at_query = calc_growth_delta(
            record.owner_balance,
            query_block_height.saturating_sub(record.active_block_height),
        );
        let growth_at_record = calc_growth_delta(
            record.owner_balance,
            record
                .block_height
                .saturating_sub(record.active_block_height),
        );
        let incremental_growth = growth_at_query.saturating_sub(growth_at_record);

        PassEnergyResult {
            energy: record.energy.saturating_add(incremental_growth),
            state: record.state.clone(),
        }
    }

    pub fn get_pass_energy_records_by_page_in_height_range(
        &self,
        inscription_id: &InscriptionId,
        from_block_height: u32,
        to_block_height: u32,
        page: usize,
        page_size: usize,
    ) -> Result<Vec<PassEnergyRecord>, String> {
        self.get_pass_energy_records_by_page_in_height_range_with_order(
            inscription_id,
            from_block_height,
            to_block_height,
            page,
            page_size,
            false,
        )
    }

    pub fn get_pass_energy_records_by_page_in_height_range_with_order(
        &self,
        inscription_id: &InscriptionId,
        from_block_height: u32,
        to_block_height: u32,
        page: usize,
        page_size: usize,
        desc: bool,
    ) -> Result<Vec<PassEnergyRecord>, String> {
        if desc {
            self.storage
                .get_pass_energy_records_by_page_in_height_range_desc(
                    inscription_id,
                    from_block_height,
                    to_block_height,
                    page,
                    page_size,
                )
        } else {
            self.storage
                .get_pass_energy_records_by_page_in_height_range(
                    inscription_id,
                    from_block_height,
                    to_block_height,
                    page,
                    page_size,
                )
        }
    }

    pub fn count_pass_energy_records_in_height_range(
        &self,
        inscription_id: &InscriptionId,
        from_block_height: u32,
        to_block_height: u32,
    ) -> Result<u64, String> {
        self.storage.count_pass_energy_records_in_height_range(
            inscription_id,
            from_block_height,
            to_block_height,
        )
    }

    #[cfg(test)]
    pub fn insert_pass_energy_record_for_test(
        &self,
        record: &PassEnergyRecord,
    ) -> Result<(), String> {
        self.storage.insert_pass_energy_record(record)
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
            let energy_delta = calc_incremental_growth(
                last_record.owner_balance,
                last_record.active_block_height,
                last_record.block_height,
                balance_record.block_height,
            );

            let mut new_energy = last_record.energy.saturating_add(energy_delta);

            // Keep active height for non-negative deltas.
            // Only negative delta starts a new growth window after penalty is applied.
            let active_block_height = if balance_record.delta < 0 {
                balance_record.block_height
            } else {
                last_record.active_block_height
            };

            // Apply protocol-defined punishment on negative balance delta.
            if balance_record.delta < 0 {
                let energy_delta = calc_penalty_from_delta(balance_record.delta);
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
            let energy_delta = calc_incremental_growth(
                last_record.owner_balance,
                last_record.active_block_height,
                last_record.block_height,
                block_height,
            );
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

    // Apply one active owner balance update (already loaded by balance settlement)
    // to energy storage without any extra balance-history RPC.
    pub fn apply_active_balance_change(
        &self,
        inscription_id: &InscriptionId,
        owner_address: &USDBScriptHash,
        block_height: u32,
        owner_balance: u64,
        owner_delta: i64,
    ) -> Result<bool, String> {
        self.apply_active_balance_change_internal(
            inscription_id,
            owner_address,
            block_height,
            owner_balance,
            owner_delta,
            self.strict_settle_consistency_enabled(),
        )
    }

    fn apply_active_balance_change_internal(
        &self,
        inscription_id: &InscriptionId,
        owner_address: &USDBScriptHash,
        block_height: u32,
        owner_balance: u64,
        owner_delta: i64,
        strict_settle_consistency: bool,
    ) -> Result<bool, String> {
        if owner_delta == 0 {
            return Ok(false);
        }

        let last_record = self
            .storage
            .find_last_pass_energy_record(inscription_id, block_height)?
            .ok_or_else(|| {
                let msg = format!(
                    "No previous energy record found when applying active balance change: inscription_id={}, block_height={}, owner={}",
                    inscription_id, block_height, owner_address
                );
                error!("{}", msg);
                msg
            })?;

        if last_record.state != MinerPassState::Active {
            let msg = format!(
                "Energy state mismatch when applying active balance change: inscription_id={}, block_height={}, owner={}, state={}",
                inscription_id,
                block_height,
                owner_address,
                last_record.state.as_str()
            );
            error!("{}", msg);
            return Err(msg);
        }
        if last_record.owner_address != *owner_address {
            let msg = format!(
                "Energy owner mismatch when applying active balance change: inscription_id={}, block_height={}, expected_owner={}, actual_owner={}",
                inscription_id, block_height, last_record.owner_address, owner_address
            );
            error!("{}", msg);
            return Err(msg);
        }
        if last_record.block_height > block_height {
            let msg = format!(
                "Energy record height is ahead of active balance update height: inscription_id={}, record_height={}, update_height={}",
                inscription_id, last_record.block_height, block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        // This height has already been materialized. Verify consistency and skip.
        if last_record.block_height == block_height {
            if last_record.owner_balance != owner_balance || last_record.owner_delta != owner_delta
            {
                let msg = format!(
                    "Active balance change conflicts with existing record at same block: inscription_id={}, block_height={}, owner={}, record_balance={}, update_balance={}, record_delta={}, update_delta={}",
                    inscription_id,
                    block_height,
                    owner_address,
                    last_record.owner_balance,
                    owner_balance,
                    last_record.owner_delta,
                    owner_delta
                );
                if strict_settle_consistency {
                    error!("{}", msg);
                    return Err(msg);
                }
                warn!("{}; skip in test mode", msg);
                return Ok(false);
            }
            return Ok(false);
        }

        // If settle balance direction conflicts with last recorded balance, skip this update.
        // This avoids poisoning records when test/staging data sources are inconsistent.
        if (owner_delta > 0 && owner_balance < last_record.owner_balance)
            || (owner_delta < 0 && owner_balance > last_record.owner_balance)
        {
            let msg = format!(
                "Active balance change has inconsistent settle direction: inscription_id={}, block_height={}, owner={}, record_balance={}, update_balance={}, update_delta={}",
                inscription_id,
                block_height,
                owner_address,
                last_record.owner_balance,
                owner_balance,
                owner_delta
            );
            if strict_settle_consistency {
                error!("{}", msg);
                return Err(msg);
            }
            warn!("{}; skip in test mode", msg);
            return Ok(false);
        }

        let growth_delta = calc_incremental_growth(
            last_record.owner_balance,
            last_record.active_block_height,
            last_record.block_height,
            block_height,
        );
        let mut next_energy = last_record.energy.saturating_add(growth_delta);

        let next_active_height = if owner_delta < 0 {
            block_height
        } else {
            last_record.active_block_height
        };
        if owner_delta < 0 {
            let penalty = calc_penalty_from_delta(owner_delta);
            next_energy = next_energy.saturating_sub(penalty);
        }

        let record = PassEnergyRecord {
            inscription_id: inscription_id.clone(),
            block_height,
            state: MinerPassState::Active,
            active_block_height: next_active_height,
            owner_address: *owner_address,
            owner_balance,
            owner_delta,
            energy: next_energy,
        };
        self.storage.insert_pass_energy_record(&record)?;
        Ok(true)
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

        let stored = self.get_pass_energy_record_exact(inscription_id, block_height)?;
        let expected_state = MinerPassState::Dormant;
        let expected_energy = finalized.energy;
        if stored.as_ref().map(|v| (&v.state, v.energy)) != Some((&expected_state, expected_energy))
        {
            let msg = format!(
                "Dormant energy snapshot mismatch: inscription_id={}, block_height={}, stored={:?}, expected={:?}",
                inscription_id,
                block_height,
                stored,
                (expected_state, expected_energy)
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
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::{SystemTime, UNIX_EPOCH};
    use usdb_util::ToUSDBScriptHash;

    struct TestBalanceProvider {
        at_height: Vec<AddressBalance>,
        at_range: Vec<AddressBalance>,
    }

    impl BalanceProvider for TestBalanceProvider {
        fn get_balance_at_height<'a>(
            &'a self,
            _address: USDBScriptHash,
            _block_height: u32,
        ) -> BalanceProviderFuture<'a, Vec<AddressBalance>> {
            let ret = self.at_height.clone();
            Box::pin(async move { Ok(ret) })
        }

        fn get_balance_at_range<'a>(
            &'a self,
            _address: USDBScriptHash,
            _block_range: std::ops::Range<u32>,
        ) -> BalanceProviderFuture<'a, Vec<AddressBalance>> {
            let ret = self.at_range.clone();
            Box::pin(async move { Ok(ret) })
        }
    }

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

    #[tokio::test]
    async fn test_get_pass_energy_returns_latest_value_at_height() {
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
            .get_pass_energy(&inscription_id, 115)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e115.state, MinerPassState::Dormant);
        assert_eq!(e115.energy, 111);

        let e120 = manager
            .get_pass_energy(&inscription_id, 120)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(e120.state, MinerPassState::Dormant);
        assert_eq!(e120.energy, 222);

        let e80 = manager.get_pass_energy(&inscription_id, 80).await.unwrap();
        assert!(e80.is_none());

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_get_pass_energy_projects_active_energy_to_query_height() {
        let root_dir = test_root_dir("at_or_before_project_active");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(3, 0);
        let owner = test_script_hash(4);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 200_000,
                owner_delta: 0,
                energy: 500,
            })
            .unwrap();

        let projected = manager
            .get_pass_energy(&inscription_id, 105)
            .await
            .unwrap()
            .unwrap();
        let expected = 500 + calc_growth_delta(200_000, 5);
        assert_eq!(projected.state, MinerPassState::Active);
        assert_eq!(projected.energy, expected);

        let via_same_api = manager
            .get_pass_energy(&inscription_id, 105)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(via_same_api, projected);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_zero_delta_skips_without_new_record() {
        // Zero delta should be treated as no-op and must not write a new record.
        let root_dir = test_root_dir("apply_delta_zero_skip");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(5, 0);
        let owner = test_script_hash(5);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 300_000,
                owner_delta: 0,
                energy: 1_000,
            })
            .unwrap();

        let changed = manager
            .apply_active_balance_change(&inscription_id, &owner, 120, 300_000, 0)
            .unwrap();
        assert!(!changed);
        assert!(
            manager
                .get_pass_energy_record_exact(&inscription_id, 120)
                .unwrap()
                .is_none()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_positive_delta_writes_record() {
        // Positive delta keeps active_block_height unchanged and only adds growth.
        let root_dir = test_root_dir("apply_positive_delta_write");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(6, 0);
        let owner = test_script_hash(6);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 300_000,
                owner_delta: 0,
                energy: 10_000,
            })
            .unwrap();

        let changed = manager
            .apply_active_balance_change(&inscription_id, &owner, 120, 320_000, 20_000)
            .unwrap();
        assert!(changed);

        let record = manager
            .get_pass_energy_record_exact(&inscription_id, 120)
            .unwrap()
            .unwrap();
        let expected_energy = 10_000 + calc_growth_delta(300_000, 20);
        assert_eq!(record.energy, expected_energy);
        assert_eq!(record.owner_balance, 320_000);
        assert_eq!(record.owner_delta, 20_000);
        assert_eq!(record.active_block_height, 100);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_uses_incremental_growth_from_last_record() {
        // Growth must be computed from (last_record_height -> current_height), not from active start.
        let root_dir = test_root_dir("apply_positive_delta_incremental_growth");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(16, 0);
        let owner = test_script_hash(16);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 110,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 300_000,
                owner_delta: 0,
                energy: 10_000 + calc_growth_delta(300_000, 10),
            })
            .unwrap();

        let changed = manager
            .apply_active_balance_change(&inscription_id, &owner, 120, 320_000, 20_000)
            .unwrap();
        assert!(changed);

        let record = manager
            .get_pass_energy_record_exact(&inscription_id, 120)
            .unwrap()
            .unwrap();
        let expected_increment = calc_growth_delta(300_000, 20) - calc_growth_delta(300_000, 10);
        let expected_energy = (10_000 + calc_growth_delta(300_000, 10)) + expected_increment;
        assert_eq!(record.energy, expected_energy);
        assert_eq!(record.owner_balance, 320_000);
        assert_eq!(record.owner_delta, 20_000);
        assert_eq!(record.active_block_height, 100);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_negative_delta_resets_active_height_and_applies_penalty() {
        // Negative delta resets active_block_height and applies protocol penalty.
        let root_dir = test_root_dir("apply_negative_delta_penalty");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(7, 0);
        let owner = test_script_hash(7);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 400_000,
                owner_delta: 0,
                energy: 10_000,
            })
            .unwrap();

        let changed = manager
            .apply_active_balance_change(&inscription_id, &owner, 120, 350_000, -50_000)
            .unwrap();
        assert!(changed);

        let record = manager
            .get_pass_energy_record_exact(&inscription_id, 120)
            .unwrap()
            .unwrap();
        let expected_energy = 10_000u64
            .saturating_add(calc_growth_delta(400_000, 20))
            .saturating_sub(calc_penalty_from_delta(-50_000));
        assert_eq!(record.energy, expected_energy);
        assert_eq!(record.owner_balance, 350_000);
        assert_eq!(record.owner_delta, -50_000);
        assert_eq!(record.active_block_height, 120);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_same_height_conflict_relaxed_mode_skips() {
        // Relaxed mode should skip same-height conflicting updates without mutating existing record.
        let root_dir = test_root_dir("apply_same_height_conflict_relaxed");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(10, 0);
        let owner = test_script_hash(10);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 120,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 320_000,
                owner_delta: 20_000,
                energy: 12_345,
            })
            .unwrap();

        let changed = manager
            .apply_active_balance_change_internal(
                &inscription_id,
                &owner,
                120,
                330_000,
                30_000,
                false,
            )
            .unwrap();
        assert!(!changed);

        let record = manager
            .get_pass_energy_record_exact(&inscription_id, 120)
            .unwrap()
            .unwrap();
        assert_eq!(record.owner_balance, 320_000);
        assert_eq!(record.owner_delta, 20_000);
        assert_eq!(record.energy, 12_345);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[tokio::test]
    async fn test_update_pass_energy_projects_incremental_growth_without_new_balance_records() {
        // When no new balance record exists, projected energy must only add one-step increment per height gap.
        let root_dir = test_root_dir("update_pass_energy_incremental_projection");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let storage = PassEnergyStorage::new(&config.data_dir()).unwrap();
        let provider = Arc::new(TestBalanceProvider {
            at_height: vec![],
            at_range: vec![],
        });
        let manager = PassEnergyManager::new_with_deps(config, storage, provider);

        let inscription_id = test_inscription_id(17, 0);
        let owner = test_script_hash(17);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 110,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 200_000,
                owner_delta: 0,
                energy: 777 + calc_growth_delta(200_000, 10),
            })
            .unwrap();

        let result = manager
            .update_pass_energy(&inscription_id, 111)
            .await
            .unwrap();
        let expected_increment = calc_growth_delta(200_000, 11) - calc_growth_delta(200_000, 10);
        assert_eq!(
            result.energy,
            777 + calc_growth_delta(200_000, 10) + expected_increment
        );
        assert_eq!(result.state, MinerPassState::Active);
        assert!(
            manager
                .get_pass_energy_record_exact(&inscription_id, 111)
                .unwrap()
                .is_none()
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_same_height_conflict_strict_mode_errors() {
        // Strict mode must fail on same-height conflicting updates.
        let root_dir = test_root_dir("apply_same_height_conflict_strict");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(11, 0);
        let owner = test_script_hash(11);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 120,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 320_000,
                owner_delta: 20_000,
                energy: 99,
            })
            .unwrap();

        let err = manager
            .apply_active_balance_change_internal(
                &inscription_id,
                &owner,
                120,
                330_000,
                30_000,
                true,
            )
            .unwrap_err();
        assert!(err.contains("conflicts with existing record at same block"));

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_direction_conflict_strict_and_relaxed() {
        // Direction conflict should be skipped in relaxed mode and rejected in strict mode.
        let root_dir = test_root_dir("apply_direction_conflict_modes");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(12, 0);
        let owner = test_script_hash(12);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 400_000,
                owner_delta: 0,
                energy: 7_000,
            })
            .unwrap();

        let changed = manager
            .apply_active_balance_change_internal(
                &inscription_id,
                &owner,
                120,
                350_000,
                10_000,
                false,
            )
            .unwrap();
        assert!(!changed);
        assert!(
            manager
                .get_pass_energy_record_exact(&inscription_id, 120)
                .unwrap()
                .is_none()
        );

        let err = manager
            .apply_active_balance_change_internal(
                &inscription_id,
                &owner,
                120,
                350_000,
                10_000,
                true,
            )
            .unwrap_err();
        assert!(err.contains("inconsistent settle direction"));

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_fails_without_previous_record() {
        // A balance delta cannot be applied without a previous energy baseline record.
        let root_dir = test_root_dir("apply_without_previous_record");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(13, 0);
        let owner = test_script_hash(13);
        let err = manager
            .apply_active_balance_change(&inscription_id, &owner, 120, 100_000, 10_000)
            .unwrap_err();
        assert!(
            err.contains("No previous energy record found when applying active balance change")
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_fails_when_last_record_not_active() {
        // Only active pass records are eligible for active balance delta application.
        let root_dir = test_root_dir("apply_state_mismatch");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(14, 0);
        let owner = test_script_hash(14);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Dormant,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 200_000,
                owner_delta: 0,
                energy: 1234,
            })
            .unwrap();

        let err = manager
            .apply_active_balance_change(&inscription_id, &owner, 120, 210_000, 10_000)
            .unwrap_err();
        assert!(err.contains("Energy state mismatch when applying active balance change"));

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_fails_when_owner_mismatch() {
        // Owner must exactly match last active energy record owner.
        let root_dir = test_root_dir("apply_owner_mismatch");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(15, 0);
        let owner = test_script_hash(15);
        let another_owner = test_script_hash(16);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 100,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 250_000,
                owner_delta: 0,
                energy: 4321,
            })
            .unwrap();

        let err = manager
            .apply_active_balance_change(&inscription_id, &another_owner, 120, 260_000, 10_000)
            .unwrap_err();
        assert!(err.contains("Energy owner mismatch when applying active balance change"));

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_fails_when_update_height_is_before_first_record() {
        // Querying at a height lower than the first record yields no baseline record.
        // The `record_height > update_height` branch in apply_active_balance_change_internal
        // is a defensive guard for unexpected storage behavior.
        let root_dir = test_root_dir("apply_before_first_record");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(17, 0);
        let owner = test_script_hash(17);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 130,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 260_000,
                owner_delta: 0,
                energy: 5432,
            })
            .unwrap();

        let err = manager
            .apply_active_balance_change(&inscription_id, &owner, 120, 270_000, 10_000)
            .unwrap_err();
        assert!(
            err.contains("No previous energy record found when applying active balance change")
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_apply_active_balance_change_same_height_same_values_is_idempotent() {
        // Same-height same-value update should be accepted as idempotent no-op.
        let root_dir = test_root_dir("apply_same_height_idempotent");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(18, 0);
        let owner = test_script_hash(18);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 120,
                state: MinerPassState::Active,
                active_block_height: 100,
                owner_address: owner,
                owner_balance: 333_000,
                owner_delta: 33_000,
                energy: 7777,
            })
            .unwrap();

        let changed = manager
            .apply_active_balance_change_internal(
                &inscription_id,
                &owner,
                120,
                333_000,
                33_000,
                true,
            )
            .unwrap();
        assert!(!changed);

        let record = manager
            .get_pass_energy_record_exact(&inscription_id, 120)
            .unwrap()
            .unwrap();
        assert_eq!(record.owner_balance, 333_000);
        assert_eq!(record.owner_delta, 33_000);
        assert_eq!(record.energy, 7777);

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_reconcile_truncates_pending_and_future_records() {
        let root_dir = test_root_dir("reconcile_truncate");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        let inscription_id = test_inscription_id(8, 0);
        let owner = test_script_hash(9);
        manager
            .storage
            .insert_pass_energy_record(&PassEnergyRecord {
                inscription_id: inscription_id.clone(),
                block_height: 120,
                state: MinerPassState::Active,
                active_block_height: 120,
                owner_address: owner,
                owner_balance: 10_000,
                owner_delta: 1,
                energy: 123,
            })
            .unwrap();
        manager.storage.set_pending_block_height(120).unwrap();
        manager.storage.set_synced_block_height(120).unwrap();

        manager.reconcile_with_pass_synced_height(100).unwrap();

        assert!(
            manager
                .storage
                .get_pass_energy_record(&inscription_id, 120)
                .unwrap()
                .is_none()
        );
        assert_eq!(manager.storage.get_pending_block_height().unwrap(), None);
        assert_eq!(
            manager.storage.get_synced_block_height().unwrap(),
            Some(100)
        );

        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn test_reconcile_fails_when_energy_synced_height_lags_pass_synced_height() {
        let root_dir = test_root_dir("reconcile_lagging_marker");
        let config = Arc::new(ConfigManager::load(Some(root_dir.clone())).unwrap());
        let manager = PassEnergyManager::new(config).unwrap();

        manager.storage.set_synced_block_height(90).unwrap();
        let err = manager.reconcile_with_pass_synced_height(100).unwrap_err();
        assert!(err.contains("Energy synced height is behind pass synced height"));

        std::fs::remove_dir_all(root_dir).unwrap();
    }
}
