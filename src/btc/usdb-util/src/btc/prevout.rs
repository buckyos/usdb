//! Block-scoped historical input values supplied by Core's block undo data, without txindex.

use std::collections::{HashMap, HashSet};

use bitcoincore_rpc::bitcoin::{
    Amount, Block, BlockHash, OutPoint, ScriptBuf, Transaction, TxOut, consensus,
};
use serde::Deserialize;

#[derive(Deserialize)]
struct BlockResponse<P = PreviousOutput> {
    hash: BlockHash,
    height: u32,
    confirmations: i64,
    tx: Vec<TransactionResponse<P>>,
}

#[derive(Deserialize)]
struct TransactionResponse<P> {
    hex: String,
    vin: Vec<InputResponse<P>>,
}

#[derive(Deserialize)]
struct InputResponse<P> {
    txid: Option<bitcoincore_rpc::bitcoin::Txid>,
    vout: Option<u32>,
    prevout: Option<P>,
}

#[derive(Deserialize)]
struct PreviousOutput {
    #[serde(with = "bitcoincore_rpc::bitcoin::amount::serde::as_btc")]
    value: Amount,
}

#[derive(Deserialize)]
struct FullPreviousOutput {
    #[serde(with = "bitcoincore_rpc::bitcoin::amount::serde::as_btc")]
    value: Amount,
    height: Option<u32>,
    generated: Option<bool>,
    #[serde(rename = "scriptPubKey")]
    script_pubkey: Option<PreviousScript>,
}

#[derive(Deserialize)]
struct PreviousScript {
    hex: String,
}

/// One spent coin supplied by Core undo, including its creation height.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpentPrevout {
    /// Exact amount and locking script, not an address inferred by an explorer.
    pub txout: TxOut,
    /// Height used to locate the creating transaction without a transaction index.
    pub height: u32,
    /// Whether the creating transaction is coinbase.
    pub coinbase: bool,
}

/// Complete spent-input evidence tied to exact block bytes and height.
/// Construction is restricted to checked Core responses; missing evidence is never zero.
#[derive(Debug, Clone)]
pub struct BlockPrevouts {
    height: u32,
    block: Block,
    prevouts: HashMap<OutPoint, SpentPrevout>,
}

impl BlockPrevouts {
    /// Return the canonical height used to obtain this evidence.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// Return the exact block whose input witnesses and prevouts were checked.
    pub fn block(&self) -> &Block {
        &self.block
    }

    /// Compare full block bytes and height before reusing this context.
    pub fn matches(&self, height: u32, block: &Block) -> bool {
        self.height == height && self.block == *block
    }

    /// Resolve a spent input of this block; unknown outpoints are availability errors.
    pub fn get(&self, outpoint: &OutPoint) -> Result<&SpentPrevout, String> {
        self.prevouts.get(outpoint).ok_or_else(|| {
            format!(
                "Spent prevout absent from evidence: height={}, outpoint={outpoint}",
                self.height
            )
        })
    }
}

/// Check a verbosity-3 response against the exact block being processed before accepting its undo.
/// Missing undo is an availability error; it must never become a zero amount or a live-UTXO lookup.
pub(super) fn parse_block_input_values(
    height: u32,
    block: &Block,
    response: serde_json::Value,
) -> Result<HashMap<OutPoint, Amount>, String> {
    let response: BlockResponse = serde_json::from_value(response)
        .map_err(|e| format!("Invalid block input response: {e}"))?;
    if response.hash != block.block_hash()
        || response.height != height
        || response.confirmations <= 0
        || response.tx.len() != block.txdata.len()
    {
        return Err(
            "Block input response does not match the requested canonical block".to_string(),
        );
    }
    let mut values = HashMap::new();
    let mut txids = HashSet::new();
    for (tx, entry) in block.txdata.iter().zip(response.tx) {
        let decoded: Transaction = consensus::encode::deserialize_hex(&entry.hex)
            .map_err(|e| format!("Invalid block input transaction: {e}"))?;
        if decoded != *tx || entry.vin.len() != tx.input.len() || !txids.insert(tx.compute_txid()) {
            return Err(
                "Block input transaction bytes/order do not match the requested block".to_string(),
            );
        }
        if tx.is_coinbase() {
            continue;
        }
        let mut input_total = 0u64;
        for (input, entry) in tx.input.iter().zip(entry.vin) {
            let outpoint = input.previous_output;
            if entry.txid != Some(outpoint.txid) || entry.vout != Some(outpoint.vout) {
                return Err(format!(
                    "Block input response outpoint mismatch: {outpoint}"
                ));
            }
            let amount = entry.prevout.ok_or_else(|| {
                format!("Block undo/prevout unavailable: height={height}, outpoint={outpoint}; retain unpruned block and undo data")
            })?.value;
            input_total = input_total
                .checked_add(amount.to_sat())
                .ok_or("Block input total overflow")?;
            if input_total > 21_000_000 * 100_000_000 || values.insert(outpoint, amount).is_some() {
                return Err(format!("Invalid or duplicate block input: {outpoint}"));
            }
        }
    }
    Ok(values)
}

/// Parse full undo evidence while preserving the legacy amount-only API's requirements.
pub(super) fn parse_block_prevouts(
    height: u32,
    block: &Block,
    response: serde_json::Value,
) -> Result<BlockPrevouts, String> {
    // Reuse exact block/transaction/input ordering, amount and duplicate-spend validation.
    if !block.check_merkle_root() {
        return Err("Block transactions disagree with header merkle root".to_string());
    }
    parse_block_input_values(height, block, response.clone())?;
    let response: BlockResponse<FullPreviousOutput> = serde_json::from_value(response)
        .map_err(|e| format!("Invalid spent prevout response: {e}"))?;
    let positions: HashMap<_, _> = block
        .txdata
        .iter()
        .enumerate()
        .map(|(i, tx)| (tx.compute_txid(), i))
        .collect();
    let mut prevouts = HashMap::new();
    for (position, (tx, entry)) in block.txdata.iter().zip(response.tx).enumerate() {
        let output_total = tx
            .output
            .iter()
            .try_fold(0u64, |sum, output| sum.checked_add(output.value.to_sat()))
            .ok_or("Block transaction output sum overflow")?;
        if output_total > 21_000_000 * 100_000_000 {
            return Err("Block transaction output sum exceeds money range".into());
        }
        if tx.is_coinbase() {
            continue;
        }
        let mut input_total = 0u64;
        for (input, entry) in tx.input.iter().zip(entry.vin) {
            let point = input.previous_output;
            let prev = entry
                .prevout
                .ok_or_else(|| format!("Missing undo: {point}"))?;
            let created = prev
                .height
                .ok_or_else(|| format!("Missing prevout creation height: {point}"))?;
            let coinbase = prev
                .generated
                .ok_or_else(|| format!("Missing prevout coinbase flag: {point}"))?;
            let script = prev
                .script_pubkey
                .ok_or_else(|| format!("Missing prevout locking script: {point}"))?;
            let script_pubkey = ScriptBuf::from_hex(&script.hex)
                .map_err(|e| format!("Invalid prevout script: outpoint={point}, error={e}"))?;
            input_total = input_total
                .checked_add(prev.value.to_sat())
                .ok_or("Input sum overflow")?;
            let txout = TxOut {
                value: prev.value,
                script_pubkey,
            };
            if created > height {
                return Err(format!("Prevout created after spending block: {point}"));
            }
            if let Some(&origin_position) = positions.get(&point.txid) {
                let origin = &block.txdata[origin_position];
                if origin_position >= position
                    || created != height
                    || coinbase != origin.is_coinbase()
                    || origin.output.get(point.vout as usize) != Some(&txout)
                {
                    return Err(format!(
                        "Same-block prevout disagrees with creating transaction: {point}"
                    ));
                }
            } else if created == height {
                return Err(format!("Same-block prevout transaction absent: {point}"));
            }
            prevouts.insert(
                point,
                SpentPrevout {
                    txout,
                    height: created,
                    coinbase,
                },
            );
        }
        if output_total > input_total {
            return Err("Block transaction outputs exceed spent inputs".into());
        }
    }
    Ok(BlockPrevouts {
        height,
        block: block.clone(),
        prevouts,
    })
}
