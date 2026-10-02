//! Sat-specific commit source evidence. This proves a Bitcoin spend, not USDB content consent.

use bitcoincore_rpc::bitcoin::{
    BlockHash, OutPoint, PublicKey, ScriptBuf, Transaction, TxOut, ecdsa,
    hashes::Hash,
    script::Instruction,
    secp256k1::{Message, Secp256k1, XOnlyPublicKey},
    sighash::{Annex, EcdsaSighashType, Prevouts, SighashCache, TapSighashType},
    taproot,
};

use super::BlockPrevouts;
use crate::{BtcScriptHash, ToBtcScriptHash};

/// Supported output-covering signature form, or a deterministic unsupported Bitcoin spend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceAuthorization {
    /// Standard P2PKH with a verified ECDSA SIGHASH_ALL signature.
    P2pkhAll,
    /// Native P2WPKH with a verified ECDSA SIGHASH_ALL signature.
    P2wpkhAll,
    /// Verified Taproot key-path DEFAULT/ALL, including the annex when present.
    TaprootKeyPath {
        /// Whether a BIP-341 annex was included in the signature hash.
        has_annex: bool,
    },
    /// A valid Bitcoin spend outside the supported USDB source-proof subset.
    Unsupported(String),
}

/// Reverse mapping of one actual commit output sat to its spending input and owner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSourceProof {
    /// Canonical commit block height.
    pub block_height: u32,
    /// Canonical commit block hash.
    pub block_hash: BlockHash,
    /// Commit output consumed by the reveal.
    pub commit_outpoint: OutPoint,
    /// Sat offset within the commit output.
    pub commit_offset: u64,
    /// Exact source input, which need not be the first input.
    pub input_index: usize,
    /// Spent source coin.
    pub source_outpoint: OutPoint,
    /// Sat offset in the source coin.
    pub source_offset: u64,
    /// Locking-script identity of the source coin.
    pub source_owner: BtcScriptHash,
    /// Output-covering signature evidence; unsupported is distinct from unavailable data.
    pub authorization: SourceAuthorization,
}

/// Trace a commit output sat using complete canonical undo evidence, then verify its source spend.
/// Errors mean missing or inconsistent evidence and must not become a permanent Invalid mint.
/// Unsupported source signatures are returned as data so first-opening exemption remains possible.
pub fn prove_commit_source(
    evidence: &BlockPrevouts,
    point: OutPoint,
    offset: u64,
) -> Result<CommitSourceProof, String> {
    let result = (|| {
        let tx = evidence
            .block()
            .txdata
            .iter()
            .find(|tx| tx.compute_txid() == point.txid)
            .ok_or_else(|| format!("Commit absent from evidence block: {point}"))?;
        if tx.is_coinbase() {
            return Err("Commit cannot be coinbase".to_string());
        }
        let output = tx
            .output
            .get(point.vout as usize)
            .ok_or("Commit vout out of range")?;
        if offset >= output.value.to_sat() {
            return Err("Commit sat offset out of range".into());
        }
        let mut absolute = offset;
        for output in &tx.output[..point.vout as usize] {
            absolute = absolute
                .checked_add(output.value.to_sat())
                .ok_or("Commit output sum overflow")?;
        }
        let prevouts = tx
            .input
            .iter()
            .map(|input| {
                evidence
                    .get(&input.previous_output)
                    .map(|prev| prev.txout.clone())
            })
            .collect::<Result<Vec<_>, _>>()?;
        let input_total = prevouts
            .iter()
            .try_fold(0u64, |sum, prev| sum.checked_add(prev.value.to_sat()))
            .ok_or("Commit input sum overflow")?;
        let output_total = tx
            .output
            .iter()
            .try_fold(0u64, |sum, out| sum.checked_add(out.value.to_sat()))
            .ok_or("Commit output sum overflow")?;
        if output_total > input_total {
            return Err("Commit outputs exceed inputs".into());
        }
        let mut remaining = absolute;
        for (index, prev) in prevouts.iter().enumerate() {
            if remaining < prev.value.to_sat() {
                return Ok(CommitSourceProof {
                    block_height: evidence.height(),
                    block_hash: evidence.block().block_hash(),
                    commit_outpoint: point,
                    commit_offset: offset,
                    input_index: index,
                    source_outpoint: tx.input[index].previous_output,
                    source_offset: remaining,
                    source_owner: prev.script_pubkey.to_btc_script_hash(),
                    authorization: verify_source_spend(tx, index, &prevouts)?,
                });
            }
            remaining -= prev.value.to_sat();
        }
        Err("Commit sat has no source input".to_string())
    })();
    result.map_err(|err| {
        let msg = format!(
            "Commit source evidence unavailable: outpoint={point}, offset={offset}, error={err}"
        );
        error!("{msg}");
        msg
    })
}

fn unsupported(reason: &str) -> Result<SourceAuthorization, String> {
    Ok(SourceAuthorization::Unsupported(reason.to_string()))
}

// Core already validated the block; cryptographic verification binds our decoded witness and
// prevout to the exact transaction again. A bad verification is an evidence inconsistency.
fn verify_source_spend(
    tx: &Transaction,
    index: usize,
    prevouts: &[TxOut],
) -> Result<SourceAuthorization, String> {
    let input = &tx.input[index];
    let prev = &prevouts[index];
    let script = &prev.script_pubkey;
    let secp = Secp256k1::verification_only();
    let mut cache = SighashCache::new(tx);
    if script.is_p2tr() {
        if !input.script_sig.is_empty() {
            return unsupported("Taproot scriptSig is not empty");
        }
        let mut stack: Vec<_> = input.witness.iter().collect();
        let annex = if stack.len() >= 2 && stack.last().unwrap().first() == Some(&0x50) {
            Some(Annex::new(stack.pop().unwrap()).map_err(|e| format!("Invalid annex: {e}"))?)
        } else {
            None
        };
        if stack.len() != 1 {
            return unsupported("Taproot script-path or missing key-path signature");
        }
        let signature = match taproot::Signature::from_slice(stack[0]) {
            Ok(signature) => signature,
            Err(_) => return unsupported("Unsupported Taproot signature encoding"),
        };
        if !matches!(
            signature.sighash_type,
            TapSighashType::Default | TapSighashType::All
        ) || (stack[0].len() == 65 && stack[0][64] != 1)
        {
            return unsupported("Taproot source requires DEFAULT or ALL without ANYONECANPAY");
        }
        let has_annex = annex.is_some();
        let digest = cache
            .taproot_signature_hash(
                index,
                &Prevouts::All(prevouts),
                annex,
                None,
                signature.sighash_type,
            )
            .map_err(|e| format!("Taproot sighash failed: {e}"))?;
        let key = XOnlyPublicKey::from_slice(&script.as_bytes()[2..])
            .map_err(|e| format!("Invalid Taproot output key: {e}"))?;
        secp.verify_schnorr(
            &signature.signature,
            &Message::from_digest(digest.to_byte_array()),
            &key,
        )
        .map_err(|e| format!("Taproot source signature disagrees with evidence: {e}"))?;
        return Ok(SourceAuthorization::TaprootKeyPath { has_annex });
    }
    let (sig_bytes, pub_bytes, form) = if script.is_p2pkh() {
        if !input.witness.is_empty() {
            return unsupported("P2PKH witness is not empty");
        }
        let pushes = input
            .script_sig
            .instructions_minimal()
            .map(|instruction| match instruction {
                Ok(Instruction::PushBytes(bytes)) => Some(bytes.as_bytes()),
                _ => None,
            })
            .collect::<Option<Vec<_>>>();
        let Some(pushes) = pushes else {
            return unsupported("Nonstandard P2PKH scriptSig");
        };
        if pushes.len() != 2 {
            return unsupported("P2PKH requires signature and public key");
        }
        (pushes[0], pushes[1], SourceAuthorization::P2pkhAll)
    } else if script.is_p2wpkh() {
        let stack: Vec<_> = input.witness.iter().collect();
        if !input.script_sig.is_empty() || stack.len() != 2 {
            return unsupported("Native P2WPKH requires empty scriptSig and two witness elements");
        }
        (stack[0], stack[1], SourceAuthorization::P2wpkhAll)
    } else {
        return unsupported("Unsupported source locking script");
    };
    if sig_bytes.last() != Some(&1) {
        return unsupported("ECDSA source requires ALL without ANYONECANPAY");
    }
    let signature = match ecdsa::Signature::from_slice(sig_bytes) {
        Ok(signature) => signature,
        Err(_) => return unsupported("Unsupported ECDSA signature encoding"),
    };
    let key = match PublicKey::from_slice(pub_bytes) {
        Ok(key) if pub_bytes.len() == 33 || pub_bytes.len() == 65 => key,
        _ => return unsupported("Unsupported ECDSA public key encoding"),
    };
    let digest = if script.is_p2pkh() {
        if ScriptBuf::new_p2pkh(&key.pubkey_hash()) != *script {
            return Err("P2PKH source public key disagrees with prevout".into());
        }
        cache
            .legacy_signature_hash(index, script, EcdsaSighashType::All.to_u32())
            .map_err(|e| format!("Legacy sighash failed: {e}"))?
            .to_byte_array()
    } else {
        let Ok(hash) = key.wpubkey_hash() else {
            return unsupported("P2WPKH requires a compressed public key");
        };
        if ScriptBuf::new_p2wpkh(&hash) != *script {
            return Err("P2WPKH source public key disagrees with prevout".into());
        }
        cache
            .p2wpkh_signature_hash(index, script, prev.value, EcdsaSighashType::All)
            .map_err(|e| format!("Witness sighash failed: {e}"))?
            .to_byte_array()
    };
    let mut normalized = signature.signature;
    normalized.normalize_s();
    secp.verify_ecdsa(&Message::from_digest(digest), &normalized, &key.inner)
        .map_err(|e| format!("ECDSA source signature disagrees with evidence: {e}"))?;
    Ok(form)
}
