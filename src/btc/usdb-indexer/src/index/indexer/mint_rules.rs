//! Current MinerPass execution and chain-bound discovery metadata.

use std::collections::HashSet;
use std::sync::Arc;

use bitcoincore_rpc::bitcoin::{Block, OutPoint, hashes::Hash};
use ord::InscriptionId;
use ordinals::SatPoint;
use usdb_util::{BtcRuleContext, BtcScriptHash};

use super::{CollectedMintItems, InscriptionIndexer, InvalidPassMintInscriptionInfo};
use crate::btc::mint_evidence::{MintEvidenceContext, MintSatOutcome};
use crate::btc::transaction_balance::TransactionBalanceContext;
use crate::index::MintValidationErrorCode;
use crate::inscription::{
    BitcoindInscriptionSource, DiscoveredMintBatch, InscriptionNewItem, InscriptionSource,
};

/// Shared chain and balance evidence for all ordered mint events in a block.
pub(super) struct BlockMintContext {
    pub(super) rules: BtcRuleContext,
    pub(super) evidence: MintEvidenceContext,
    // Blocks with no schema/sat-valid candidates need no balance RPC.
    pub(super) balances: Option<TransactionBalanceContext>,
}

impl BlockMintContext {
    // Ordered events must never be interpreted with another block's supported rules.
    pub(super) fn require_height(&self, height: u32) -> Result<(), String> {
        if self.rules.btc_height() != height {
            let msg = format!(
                "Mint rule context height mismatch: event_height={height}, context_height={}, registry_id={}, network_id={}, rules_scope={}",
                self.rules.btc_height(),
                self.rules.activation_registry_id(),
                self.rules.scope().network_id,
                self.rules.scope().rules_scope()
            );
            error!("{msg}");
            return Err(msg);
        }
        Ok(())
    }
}

impl InscriptionIndexer {
    pub(super) async fn collect_block_inscription_mints(
        &self,
        rules: &BtcRuleContext,
        block: Arc<Block>,
        evidence: &MintEvidenceContext,
    ) -> Result<CollectedMintItems, String> {
        let height = rules.btc_height();
        let discovered = self
            .inscription_source
            .load_block_mint_batch(
                rules,
                Some(block.clone()),
                self.config.config().bitcoin.network(),
            )
            .await?;
        // Reparse locally even with an external source: omissions, duplicates, schema outcomes
        // and source-local numbering must not change the v2 consensus event set.
        let batch = BitcoindInscriptionSource::new(self.btc_client.clone())
            .load_block_mint_batch(
                rules,
                Some(block.clone()),
                self.config.config().bitcoin.network(),
            )
            .await?;
        Self::check_v2_discovery(&block, height, &discovered, &batch)?;
        let mut valid_items = Vec::new();
        let mut invalid_items = Vec::new();
        for mint in batch.valid_mints {
            match evidence.locate_mint(mint.inscription_id)? {
                MintSatOutcome::Located(sat) => {
                    let output = block.txdata.iter().find(|tx| tx.compute_txid() == sat.satpoint.outpoint.txid)
                        .and_then(|tx| tx.output.get(sat.satpoint.outpoint.vout as usize))
                        .ok_or_else(|| {
                            let msg = format!("Located mint output missing from reveal block: inscription_id={}, block_height={height}, block_hash={}, satpoint={}", mint.inscription_id, block.block_hash(), sat.satpoint);
                            error!("{msg}"); msg
                        })?;
                    valid_items.push(InscriptionNewItem {
                        inscription_id: mint.inscription_id,
                        inscription_number: mint.inscription_number,
                        block_height: height,
                        timestamp: block.header.time,
                        address: sat.mint_owner,
                        satpoint: sat.satpoint,
                        value: output.value,
                        op: mint.content.op(),
                        content: mint.content,
                        content_string: mint.content_string,
                        commit_txid: sat.commit_outpoint().txid,
                    });
                }
                MintSatOutcome::Unsupported(reason) => invalid_items.push(Self::v2_invalid_mint(
                    mint.inscription_id,
                    mint.inscription_number,
                    height,
                    None,
                    MintValidationErrorCode::UnsupportedInscription,
                    reason.into(),
                )),
            }
        }
        for mint in batch.invalid_mints {
            let location = match evidence.locate_mint(mint.inscription_id)? {
                MintSatOutcome::Located(sat) => Some((sat.mint_owner, sat.satpoint)),
                MintSatOutcome::Unsupported(_) => None,
            };
            invalid_items.push(Self::v2_invalid_mint(
                mint.inscription_id,
                mint.inscription_number,
                height,
                location,
                mint.error_code,
                mint.error_reason,
            ));
        }
        Ok(CollectedMintItems {
            valid_items,
            invalid_items,
        })
    }

    // A deterministic sentinel describes unsupported/unbound v2 envelopes without inventing a
    // spendable recipient. Invalid passes are never used as an acquisition or tracker seed.
    fn v2_invalid_mint(
        id: InscriptionId,
        number: i32,
        height: u32,
        location: Option<(BtcScriptHash, SatPoint)>,
        code: MintValidationErrorCode,
        reason: String,
    ) -> InvalidPassMintInscriptionInfo {
        let (owner, satpoint) = location.unwrap_or((
            BtcScriptHash::all_zeros(),
            SatPoint {
                outpoint: OutPoint::new(id.txid, u32::MAX),
                offset: 0,
            },
        ));
        InvalidPassMintInscriptionInfo {
            inscription_id: id,
            inscription_number: number,
            mint_txid: id.txid,
            mint_block_height: height,
            mint_owner: owner,
            satpoint,
            error_code: code.as_str().into(),
            error_reason: reason,
        }
    }

    fn check_v2_discovery(
        block: &Block,
        height: u32,
        discovered: &DiscoveredMintBatch,
        canonical: &DiscoveredMintBatch,
    ) -> Result<(), String> {
        fn entries(batch: &DiscoveredMintBatch) -> Vec<(InscriptionId, i32, u32, &str, bool)> {
            let mut items: Vec<_> = batch
                .valid_mints
                .iter()
                .map(|m| {
                    (
                        m.inscription_id,
                        m.inscription_number,
                        m.block_height,
                        m.content_string.as_str(),
                        true,
                    )
                })
                .chain(batch.invalid_mints.iter().map(|m| {
                    (
                        m.inscription_id,
                        m.inscription_number,
                        m.block_height,
                        m.content_string.as_str(),
                        false,
                    )
                }))
                .collect();
            items.sort_unstable();
            items
        }
        let source = entries(discovered);
        let actual = entries(canonical);
        if source != actual {
            let mismatch = source
                .iter()
                .zip(&actual)
                .find(|(left, right)| left != right)
                .map(|(left, right)| (left.0, right.0));
            let msg = format!(
                "Mint discovery disagrees with canonical reveal set: block_height={height}, block_hash={}, source_count={}, canonical_count={}, first_mismatched_ids={mismatch:?}",
                block.block_hash(),
                source.len(),
                actual.len()
            );
            error!("{msg}");
            return Err(msg);
        }
        Ok(())
    }

    pub(super) async fn load_mint_balances(
        &self,
        evidence: &MintEvidenceContext,
        mints: &[InscriptionNewItem],
    ) -> Result<Option<TransactionBalanceContext>, String> {
        if mints.is_empty() {
            return Ok(None);
        }
        let mut seen = HashSet::new();
        let owners = mints
            .iter()
            .map(|mint| mint.address)
            .filter(|owner| seen.insert(*owner))
            .collect();
        let inputs = evidence.block_prevouts()?;
        TransactionBalanceContext::load_at_block_end(
            &self.btc_client,
            &self.balance_evidence_client,
            &inputs,
            owners,
        )
        .await
        .map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inscription::DiscoveredInvalidMint;

    #[test]
    fn discovery_must_match_complete_canonical_candidates() {
        let block = crate::index::test_miner_evidence::block(vec![]);
        let mint = DiscoveredInvalidMint {
            inscription_id: InscriptionId {
                txid: block.txdata[0].compute_txid(),
                index: 0,
            },
            inscription_number: 0,
            block_height: 10,
            timestamp: 0,
            satpoint: None,
            content_string: "fixture".into(),
            error_code: MintValidationErrorCode::InvalidSchema,
            error_reason: "fixture rejection".into(),
        };
        let expected = DiscoveredMintBatch {
            valid_mints: vec![],
            invalid_mints: vec![mint],
        };
        assert!(InscriptionIndexer::check_v2_discovery(&block, 10, &expected, &expected).is_ok());
        for variant in 0..4 {
            let mut source = expected.clone();
            match variant {
                0 => source.invalid_mints.clear(),
                1 => source.invalid_mints.push(source.invalid_mints[0].clone()),
                2 => source.invalid_mints[0].inscription_number = 1,
                _ => source.invalid_mints[0].content_string = "different content".into(),
            }
            assert!(
                InscriptionIndexer::check_v2_discovery(&block, 10, &source, &expected).is_err()
            );
        }
    }
}
