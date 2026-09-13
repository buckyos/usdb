//! Block-scoped historical input values supplied by Core's block undo data, without txindex.

use std::collections::{HashMap, HashSet};

use bitcoincore_rpc::bitcoin::{Amount, Block, BlockHash, OutPoint, Transaction, consensus};
use serde::Deserialize;

#[derive(Deserialize)]
struct BlockResponse {
    hash: BlockHash,
    height: u32,
    confirmations: i64,
    tx: Vec<TransactionResponse>,
}

#[derive(Deserialize)]
struct TransactionResponse {
    hex: String,
    vin: Vec<InputResponse>,
}

#[derive(Deserialize)]
struct InputResponse {
    txid: Option<bitcoincore_rpc::bitcoin::Txid>,
    vout: Option<u32>,
    prevout: Option<PreviousOutput>,
}

#[derive(Deserialize)]
struct PreviousOutput {
    #[serde(with = "bitcoincore_rpc::bitcoin::amount::serde::as_btc")]
    value: Amount,
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
