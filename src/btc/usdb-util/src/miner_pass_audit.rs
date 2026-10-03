//! Rebuildable MinerPass operation audit data; not additional consensus state.

use crate::{BtcScriptHash, CommitSourceProof, SourceAuthorization};
use serde::{Deserialize, Serialize};

/// Public chain locations identifying the verified source coin; no witness/signature bytes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MintSourceAudit {
    /// Height and hash of the commit block used for this proof.
    pub commit_height: u32,
    /// Exact canonical commit block hash.
    pub commit_block_hash: String,
    /// Commit output spent by the reveal, in txid:vout form.
    pub commit_outpoint: String,
    /// Input supplying the inscription sat to that commit output.
    pub input_index: usize,
    /// Source UTXO spent by the commit.
    pub source_outpoint: String,
    /// Sat offset within the source UTXO.
    pub source_offset: u64,
    /// Source locking-script identity D.
    pub source_owner: BtcScriptHash,
    /// Supported signature form, or `unsupported` for a protocol rejection.
    pub authorization: String,
    /// Annex presence for Taproot key-path proofs, otherwise None.
    pub has_annex: Option<bool>,
}

impl From<&CommitSourceProof> for MintSourceAudit {
    fn from(proof: &CommitSourceProof) -> Self {
        let (authorization, has_annex) = match &proof.authorization {
            SourceAuthorization::P2pkhAll => ("p2pkh_all", None),
            SourceAuthorization::P2wpkhAll => ("p2wpkh_all", None),
            SourceAuthorization::TaprootKeyPath { has_annex } => {
                ("taproot_key_path", Some(*has_annex))
            }
            SourceAuthorization::Unsupported(_) => ("unsupported", None),
        };
        Self {
            commit_height: proof.block_height,
            commit_block_hash: proof.block_hash.to_string(),
            commit_outpoint: proof.commit_outpoint.to_string(),
            input_index: proof.input_index,
            source_outpoint: proof.source_outpoint.to_string(),
            source_offset: proof.source_offset,
            source_owner: proof.source_owner,
            authorization: authorization.into(),
            has_annex,
        }
    }
}

/// Auxiliary explanation of a completed v2 mint attempt, reconstructible from chain and pass history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MinerPassMintAudit {
    /// Fixed serialization contract, independent of economic query semantics.
    pub schema_version: String,
    /// Attempted inscription, including Invalid mints.
    pub inscription_id: String,
    /// Reveal block height.
    pub block_height: u32,
    /// Reveal block hash; distinguishes reorg branches at the same height.
    pub block_hash: String,
    /// Recipient E when the sat/envelope is supported; None for the Invalid-only sentinel.
    pub recipient: Option<BtcScriptHash>,
    /// Exact recipient balance before its reveal transaction, if eligibility was evaluated.
    pub balance_before_tx: Option<u64>,
    /// Historical occupancy immediately before this ordered mint event.
    pub ever_valid_owner: Option<bool>,
    /// Verified source evidence, absent for first opening or rejection before proof loading.
    pub source: Option<MintSourceAudit>,
    /// Successful operation: first_opening, same_owner, or cross_owner; None on Invalid.
    pub operation_path: Option<String>,
    /// Protocol rejection code; runtime evidence/IO failures never create audit records.
    pub error_code: Option<String>,
    /// Existing canonical protocol rejection reason, without local diagnostic context.
    pub error_reason: Option<String>,
}
