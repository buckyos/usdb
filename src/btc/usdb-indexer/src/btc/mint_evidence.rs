//! Read-only v2 evidence context. No pass state or activation selection is changed here.

use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use bitcoincore_rpc::bitcoin::{
    Block, OutPoint,
    secp256k1::{Secp256k1, XOnlyPublicKey},
    taproot::{ControlBlock, LeafVersion},
};
use lru::LruCache;
use ord::{InscriptionId, ParsedEnvelope};
use ordinals::SatPoint;
use usdb_util::{
    BTCRpcClientRef, BlockPrevouts, BtcScriptHash, CommitSourceProof, ToBtcScriptHash,
    is_core_unspendable, prove_commit_source,
};

use super::UTXOValueManager;

/// A supported inscription's actual input and output sat, tied to one reveal block.
#[derive(Debug, Clone)]
pub struct MintSatEvidence {
    inscription_id: InscriptionId,
    commit_outpoint: OutPoint,
    /// Offset of the supported inscription in its commit output (zero for the v2 subset).
    pub commit_offset: u64,
    /// Exact output and sat offset created by the reveal.
    pub satpoint: SatPoint,
    /// Locking-script identity of the reveal recipient.
    pub mint_owner: BtcScriptHash,
}

impl MintSatEvidence {
    /// Exact commit output whose script and sat location were checked against this reveal.
    pub fn commit_outpoint(&self) -> OutPoint {
        self.commit_outpoint
    }
}

/// Deterministic sat-form rejection is separate from retryable evidence loading errors.
#[derive(Debug, Clone)]
pub enum MintSatOutcome {
    /// The supported Ord subset determines a usable input and recipient sat.
    Located(MintSatEvidence),
    /// The chain data is available but this inscription form is outside the v2 subset.
    Unsupported(&'static str),
}

/// Source lookup outcome when the creating transaction has no spending input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintSourceOutcome {
    /// A located source input with its supported or unsupported signature classification.
    Proven(CommitSourceProof),
    /// A coinbase commit has no spending owner and cannot authorize inherited rights.
    CoinbaseCommit,
}

/// Lazy evidence for one reveal block, including commits before the index origin or snapshot base.
/// The context is transient; no failed or partially loaded evidence is published.
pub struct MintEvidenceContext {
    client: BTCRpcClientRef,
    inputs: UTXOValueManager,
    historical: Mutex<LruCache<u32, Arc<BlockPrevouts>>>,
}

impl MintEvidenceContext {
    /// Bind the context to the block currently being processed.
    pub fn new(client: BTCRpcClientRef, height: u32, block: Arc<Block>) -> Self {
        Self {
            inputs: UTXOValueManager::new(client.clone(), height, block),
            client,
            // A reveal block may reference many old blocks; do not retain them all in memory.
            historical: Mutex::new(LruCache::new(NonZeroUsize::new(4).unwrap())),
        }
    }

    /// Obtain complete input evidence for pre-transaction balances and sat location.
    pub fn block_prevouts(&self) -> Result<Arc<BlockPrevouts>, String> {
        self.inputs.get_prevouts()
    }

    /// Locate a single-envelope, no-pointer inscription using Ord 0.24.2 input ordering.
    /// Pointer, unbound, malformed/ambiguous envelopes and fee/unspendable destinations are
    /// deterministic unsupported forms; missing block input evidence remains retryable.
    pub fn locate_mint(&self, id: InscriptionId) -> Result<MintSatOutcome, String> {
        let evidence = self.block_prevouts()?;
        let tx = evidence
            .block()
            .txdata
            .iter()
            .find(|tx| tx.compute_txid() == id.txid)
            .ok_or_else(|| format!("Reveal absent from processing block: {id}"))?;
        let envelopes = ParsedEnvelope::from_transaction(tx);
        let envelope = envelopes
            .get(id.index as usize)
            .ok_or_else(|| format!("Reveal envelope absent: {id}"))?;
        if envelope.payload.pointer.is_some() {
            return Ok(MintSatOutcome::Unsupported(
                "Pointer inscriptions are not supported",
            ));
        }
        if envelope.offset != 0
            || envelopes
                .iter()
                .filter(|other| other.input == envelope.input)
                .count()
                != 1
        {
            return Ok(MintSatOutcome::Unsupported(
                "Multiple envelopes on one reveal input",
            ));
        }
        if envelope.pushnum
            || envelope.stutter
            || envelope.payload.duplicate_field
            || envelope.payload.incomplete_field
            || envelope.payload.unrecognized_even_field
        {
            return Ok(MintSatOutcome::Unsupported("Malformed or unbound envelope"));
        }
        let index = envelope.input as usize;
        let input = tx
            .input
            .get(index)
            .ok_or("Reveal envelope input out of range")?;
        let previous = &evidence.get(&input.previous_output)?.txout;
        if !previous.script_pubkey.is_p2tr() {
            return Ok(MintSatOutcome::Unsupported(
                "Inscription input is not native Taproot",
            ));
        }
        let leaf = input
            .witness
            .taproot_leaf_script()
            .ok_or("Reveal Taproot leaf absent")?;
        if leaf.version != LeafVersion::TapScript {
            return Ok(MintSatOutcome::Unsupported("Unknown Taproot leaf version"));
        }
        let control = ControlBlock::decode(
            input
                .witness
                .taproot_control_block()
                .ok_or("Reveal control block absent")?,
        )
        .map_err(|err| format!("Invalid reveal control block: {err}"))?;
        let output_key = XOnlyPublicKey::from_slice(&previous.script_pubkey.as_bytes()[2..])
            .map_err(|err| format!("Invalid reveal output key: {err}"))?;
        if !control.verify_taproot_commitment(
            &Secp256k1::verification_only(),
            output_key,
            leaf.script,
        ) {
            return Err("Reveal script disagrees with commit output key".into());
        }
        if previous.value.to_sat() == 0 {
            return Ok(MintSatOutcome::Unsupported(
                "Zero-valued inscription input is unbound",
            ));
        }
        let mut absolute = 0u64;
        for earlier in &tx.input[..index] {
            absolute = absolute
                .checked_add(evidence.get(&earlier.previous_output)?.txout.value.to_sat())
                .ok_or("Reveal input sum overflow")?;
        }
        for (vout, output) in tx.output.iter().enumerate() {
            if absolute < output.value.to_sat() {
                if is_core_unspendable(&output.script_pubkey) {
                    return Ok(MintSatOutcome::Unsupported(
                        "Inscription recipient is unspendable",
                    ));
                }
                return Ok(MintSatOutcome::Located(MintSatEvidence {
                    inscription_id: id,
                    commit_outpoint: input.previous_output,
                    commit_offset: 0,
                    satpoint: SatPoint {
                        outpoint: OutPoint::new(id.txid, vout as u32),
                        offset: absolute,
                    },
                    mint_owner: output.script_pubkey.to_btc_script_hash(),
                }));
            }
            absolute -= output.value.to_sat();
        }
        Ok(MintSatOutcome::Unsupported(
            "Inscription lost to reveal fees",
        ))
    }

    /// Trace the located sat into its creating commit block and verify that exact source input.
    /// No getrawtransaction, gettxout, index-origin floor or browser-derived source is used.
    pub fn prove_source(&self, sat: &MintSatEvidence) -> Result<MintSourceOutcome, String> {
        let result = (|| {
            let current = self.block_prevouts()?;
            // Do not accept receipts from another context or a replaced branch.
            let MintSatOutcome::Located(checked) = self.locate_mint(sat.inscription_id)? else {
                return Err("Source receipt no longer has a supported reveal".to_string());
            };
            if checked.commit_outpoint != sat.commit_outpoint
                || checked.commit_offset != sat.commit_offset
                || checked.satpoint != sat.satpoint
                || checked.mint_owner != sat.mint_owner
            {
                return Err("Source receipt disagrees with reveal evidence".into());
            }
            let coin = current.get(&sat.commit_outpoint)?;
            let commit = if coin.height == current.height() {
                current.clone()
            } else {
                let mut history = self.historical.lock().unwrap();
                let hash = self.client.get_block_hash(coin.height)?;
                if history
                    .get(&coin.height)
                    .is_some_and(|entry| entry.block().block_hash() != hash)
                {
                    history.pop(&coin.height);
                }
                if let Some(entry) = history.get(&coin.height) {
                    entry.clone()
                } else {
                    let block = self.client.get_block_by_hash(&hash)?;
                    let entry = Arc::new(self.client.get_block_prevouts(coin.height, &block)?);
                    history.put(coin.height, entry.clone());
                    entry
                }
            };
            let origin = commit
                .block()
                .txdata
                .iter()
                .find(|tx| tx.compute_txid() == sat.commit_outpoint.txid)
                .ok_or("Creating commit absent at prevout creation height")?;
            if origin.output.get(sat.commit_outpoint.vout as usize) != Some(&coin.txout)
                || origin.is_coinbase() != coin.coinbase
            {
                return Err("Reveal prevout disagrees with creating commit".into());
            }
            let proof = if origin.is_coinbase() {
                MintSourceOutcome::CoinbaseCommit
            } else {
                MintSourceOutcome::Proven(prove_commit_source(
                    &commit,
                    sat.commit_outpoint,
                    sat.commit_offset,
                )?)
            };
            // Check both anchors after all reads, including cache hits.
            if self.client.get_block_hash(commit.height())? != commit.block().block_hash()
                || self.client.get_block_hash(current.height())? != current.block().block_hash()
            {
                return Err("Canonical chain changed during commit source retrieval".into());
            }
            Ok(proof)
        })();
        result.map_err(|err| {
            let msg = format!(
                "Mint source unavailable: inscription={}, error={err}",
                sat.inscription_id
            );
            error!("{msg}");
            msg
        })
    }
}
