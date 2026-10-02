//! Exact sat balances before each transaction, separate from whole-block energy settlement.

use std::collections::HashMap;

use balance_history::RpcClient as BalanceHistoryRpcClient;
use bitcoincore_rpc::bitcoin::BlockHash;
use usdb_util::{BTCRpcClient, BlockPrevouts, BtcScriptHash, ToBtcScriptHash, is_core_unspendable};

const MAX_MONEY: i128 = 21_000_000 * 100_000_000;

/// Which exact chain state supplied the balance baseline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceBaselineSide {
    /// Balances after H-1, anchored to the processing block's parent.
    BeforeBlock,
    /// Balances after H; reverse the full checked block delta before replaying its transactions.
    AfterBlock,
}

/// Exact query result for selected owners at one canonical block boundary.
#[derive(Debug)]
pub struct BalanceBaseline {
    /// Whether the baseline is before or after the processing block.
    pub side: BalanceBaselineSide,
    /// Exact query height, not the height of the last address change record.
    pub height: u32,
    /// Canonical BTC hash at the query height.
    pub block_hash: BlockHash,
    /// Explicit balance for every owner being queried; absent keys are never implicit zeros.
    pub balances: HashMap<BtcScriptHash, u64>,
}

/// Immutable transaction-before balances for selected owners in one exact block.
/// Multiple inscriptions in the same transaction read the same balance; history eligibility is
/// intentionally separate and must still advance in ordered-event order.
#[derive(Debug)]
pub struct TransactionBalanceContext {
    height: u32,
    block_hash: BlockHash,
    before: HashMap<BtcScriptHash, Vec<u64>>,
}

impl TransactionBalanceContext {
    /// Reconstruct the initial balance and replay every transaction, including ordinary payments,
    /// coinbase and invalid inscriptions. Arithmetic/inconsistent anchors are availability errors.
    pub fn from_baseline(
        inputs: &BlockPrevouts,
        baseline: BalanceBaseline,
    ) -> Result<Self, String> {
        let result = (|| {
            let block = inputs.block();
            let expected = match baseline.side {
                BalanceBaselineSide::BeforeBlock => (
                    inputs
                        .height()
                        .checked_sub(1)
                        .ok_or("No pre-genesis baseline")?,
                    block.header.prev_blockhash,
                ),
                BalanceBaselineSide::AfterBlock => (inputs.height(), block.block_hash()),
            };
            if (baseline.height, baseline.block_hash) != expected {
                return Err("Balance baseline disagrees with processing block anchor".to_string());
            }
            // A sparse delta per transaction avoids scanning all owners for each input/output.
            let mut deltas = Vec::with_capacity(block.txdata.len());
            for tx in &block.txdata {
                let mut delta: HashMap<BtcScriptHash, i128> = HashMap::new();
                if !tx.is_coinbase() {
                    for input in &tx.input {
                        let prev = &inputs.get(&input.previous_output)?.txout;
                        let owner = prev.script_pubkey.to_btc_script_hash();
                        if baseline.balances.contains_key(&owner) {
                            add_delta(&mut delta, owner, -i128::from(prev.value.to_sat()))?;
                        }
                    }
                }
                // The genesis coinbase was never inserted into Core's UTXO set.
                if inputs.height() != 0 {
                    for output in &tx.output {
                        if is_core_unspendable(&output.script_pubkey) {
                            continue;
                        }
                        let owner = output.script_pubkey.to_btc_script_hash();
                        if baseline.balances.contains_key(&owner) {
                            add_delta(&mut delta, owner, i128::from(output.value.to_sat()))?;
                        }
                    }
                }
                deltas.push(delta);
            }
            let mut before = HashMap::new();
            for (owner, balance) in baseline.balances {
                let mut value = i128::from(balance);
                checked_balance(value)?;
                if baseline.side == BalanceBaselineSide::AfterBlock {
                    for delta in &deltas {
                        value = value
                            .checked_sub(delta.get(&owner).copied().unwrap_or(0))
                            .ok_or("Reverse balance overflow")?;
                    }
                }
                let mut balances = Vec::with_capacity(deltas.len());
                for delta in &deltas {
                    balances.push(checked_balance(value)?);
                    value = value
                        .checked_add(delta.get(&owner).copied().unwrap_or(0))
                        .ok_or("Forward balance overflow")?;
                }
                checked_balance(value)?;
                before.insert(owner, balances);
            }
            Ok(Self {
                height: inputs.height(),
                block_hash: block.block_hash(),
                before,
            })
        })();
        result.map_err(|err| {
            let msg = format!(
                "Transaction balance evidence unavailable: height={}, error={err}",
                inputs.height()
            );
            error!("{msg}");
            msg
        })
    }

    /// Load an exact H post-block baseline and reverse the full block delta. Using H deliberately
    /// supports an AssumeUTXO query floor at H without requiring nonexistent H-1 history.
    /// Exact-height queries must return one explicit record, even for a zero balance.
    pub async fn load_at_block_end(
        core: &BTCRpcClient,
        history: &BalanceHistoryRpcClient,
        inputs: &BlockPrevouts,
        owners: Vec<BtcScriptHash>,
    ) -> Result<Self, String> {
        let result = async {
            let height = inputs.height();
            let hash = inputs.block().block_hash();
            if core.get_block_hash(height)? != hash {
                return Err("Balance block no longer canonical".to_string());
            }
            let reference = history.get_state_ref_at_height(height).await?;
            if reference.block_height != height
                || reference.stable_block_hash != hash.to_string()
                || reference.consensus_identity.stable_height != height
                || reference.consensus_identity.stable_block_hash != hash.to_string()
                || reference.snapshot_id
                    != usdb_util::build_consensus_snapshot_id(&reference.consensus_identity)
                || reference.snapshot_id_hash_algo != usdb_util::CONSENSUS_SNAPSHOT_ID_HASH_ALGO
                || reference.snapshot_id_version != usdb_util::CONSENSUS_SNAPSHOT_ID_VERSION
            {
                return Err("Balance History reference disagrees with Core block".to_string());
            }
            let records = history
                .get_addresses_balances(owners.clone(), Some(height), None)
                .await?;
            if records.len() != owners.len() {
                return Err("Incomplete balance response".to_string());
            }
            let mut balances = HashMap::new();
            for (owner, records) in owners.into_iter().zip(records) {
                let balance = match records.as_slice() {
                    [record] if record.block_height <= height => record.balance,
                    _ => return Err("Unexpected exact-height balance records".to_string()),
                };
                if balances.insert(owner, balance).is_some() {
                    return Err("Duplicate balance owner".to_string());
                }
            }
            if history.get_state_ref_at_height(height).await? != reference
                || core.get_block_hash(height)? != hash
            {
                return Err("Balance chain reference changed during query".to_string());
            }
            Self::from_baseline(
                inputs,
                BalanceBaseline {
                    side: BalanceBaselineSide::AfterBlock,
                    height,
                    block_hash: hash,
                    balances,
                },
            )
        }
        .await;
        result.map_err(|err| {
            let msg = format!(
                "Balance baseline unavailable: height={}, error={err}",
                inputs.height()
            );
            error!("{msg}");
            msg
        })
    }

    /// Require the same canonical block as the mint evidence, reporting both anchors on mismatch.
    pub fn require_matching_block(&self, inputs: &BlockPrevouts) -> Result<(), String> {
        let evidence_height = inputs.height();
        let evidence_block_hash = inputs.block().block_hash();
        if self.height != evidence_height || self.block_hash != evidence_block_hash {
            let msg = format!(
                "Transaction balance context disagrees with evidence block: balance_height={}, balance_block_hash={}, evidence_height={evidence_height}, evidence_block_hash={evidence_block_hash}",
                self.height, self.block_hash
            );
            error!("{msg}");
            return Err(msg);
        }
        Ok(())
    }

    /// Read the exact balance before transaction `position`; unrequested owners are errors.
    pub fn balance_before(&self, owner: &BtcScriptHash, position: usize) -> Result<u64, String> {
        self.before.get(owner).and_then(|values| values.get(position)).copied().ok_or_else(|| {
            format!("Transaction balance not loaded: height={}, hash={}, owner={owner}, tx_position={position}",
                self.height, self.block_hash)
        })
    }
}

fn checked_balance(value: i128) -> Result<u64, String> {
    if !(0..=MAX_MONEY).contains(&value) {
        return Err(format!("Balance outside Bitcoin money range: {value}"));
    }
    Ok(value as u64)
}

fn add_delta(
    deltas: &mut HashMap<BtcScriptHash, i128>,
    owner: BtcScriptHash,
    change: i128,
) -> Result<(), String> {
    let value = deltas.entry(owner).or_default();
    *value = value
        .checked_add(change)
        .ok_or("Transaction balance delta overflow")?;
    Ok(())
}
