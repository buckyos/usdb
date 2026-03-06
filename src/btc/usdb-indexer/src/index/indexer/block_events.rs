use super::InscriptionIndexer;
use crate::inscription::{InscriptionNewItem, InscriptionTransferItem};
use bitcoincore_rpc::bitcoin::{Block, OutPoint, Txid};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Clone)]
pub(super) enum BlockProcessEvent {
    Mint(InscriptionNewItem),
    Transfer(InscriptionTransferItem),
}

struct OrderedBlockProcessEvent {
    sort_key: EventSortKey,
    inscription_key: String,
    event: BlockProcessEvent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct EventSortKey {
    tx_position: usize,
    priority: u8,
    input_index: usize,
    inscription_index: u32,
}

pub(super) struct BlockEventPlanner {
    block_height: u32,
    block: Arc<Block>,
    mint_items: Vec<InscriptionNewItem>,
    transfer_items: Vec<InscriptionTransferItem>,
}

impl BlockEventPlanner {
    pub(super) fn new(
        block_height: u32,
        block: Arc<Block>,
        mint_items: Vec<InscriptionNewItem>,
        transfer_items: Vec<InscriptionTransferItem>,
    ) -> Self {
        Self {
            block_height,
            block,
            mint_items,
            transfer_items,
        }
    }

    pub(super) fn plan(self) -> Result<Vec<BlockProcessEvent>, String> {
        if self.mint_items.is_empty() && self.transfer_items.is_empty() {
            return Ok(Vec::new());
        }

        let tx_positions = InscriptionIndexer::build_block_tx_position_map(&self.block);
        let transfer_input_indices = Self::build_transfer_input_index_map(&self.block);
        let mut ordered_events = Vec::<OrderedBlockProcessEvent>::with_capacity(
            self.mint_items.len() + self.transfer_items.len(),
        );

        for item in self.mint_items {
            let tx_position = tx_positions
                .get(&item.inscription_id.txid)
                .copied()
                .ok_or_else(|| {
                    let msg = format!(
                        "Mint transaction {} not found in block {} when ordering events for inscription {}",
                        item.inscription_id.txid, self.block_height, item.inscription_id
                    );
                    error!("{}", msg);
                    msg
                })?;

            ordered_events.push(OrderedBlockProcessEvent {
                sort_key: EventSortKey {
                    tx_position,
                    priority: 1,
                    input_index: usize::MAX,
                    inscription_index: item.inscription_id.index,
                },
                inscription_key: item.inscription_id.to_string(),
                event: BlockProcessEvent::Mint(item),
            });
        }

        for item in self.transfer_items {
            let transfer_txid = *item.txid();
            let tx_position = tx_positions.get(&transfer_txid).copied().ok_or_else(|| {
                let msg = format!(
                    "Transfer transaction {} not found in block {} when ordering events for inscription {}",
                    transfer_txid, self.block_height, item.inscription_id
                );
                error!("{}", msg);
                msg
            })?;
            let input_index = transfer_input_indices
                .get(&transfer_txid)
                .and_then(|m| m.get(&item.prev_satpoint.outpoint))
                .copied()
                .ok_or_else(|| {
                    let msg = format!(
                        "Transfer input not found in transaction {} at block {} for inscription {}, prev_outpoint={}",
                        transfer_txid,
                        self.block_height,
                        item.inscription_id,
                        item.prev_satpoint.outpoint
                    );
                    error!("{}", msg);
                    msg
                })?;

            ordered_events.push(OrderedBlockProcessEvent {
                sort_key: EventSortKey {
                    tx_position,
                    priority: 0,
                    input_index,
                    inscription_index: item.inscription_id.index,
                },
                inscription_key: item.inscription_id.to_string(),
                event: BlockProcessEvent::Transfer(item),
            });
        }

        ordered_events.sort_by(|a, b| {
            a.sort_key
                .cmp(&b.sort_key)
                .then(a.inscription_key.cmp(&b.inscription_key))
        });

        Ok(ordered_events.into_iter().map(|item| item.event).collect())
    }

    fn build_transfer_input_index_map(block: &Block) -> HashMap<Txid, HashMap<OutPoint, usize>> {
        let mut ret = HashMap::<Txid, HashMap<OutPoint, usize>>::with_capacity(block.txdata.len());
        for tx in &block.txdata {
            let txid = tx.compute_txid();
            let mut inputs = HashMap::<OutPoint, usize>::with_capacity(tx.input.len());
            for (input_index, vin) in tx.input.iter().enumerate() {
                inputs.entry(vin.previous_output).or_insert(input_index);
            }
            ret.insert(txid, inputs);
        }
        ret
    }
}

pub(super) struct BlockEventExecutor<'a> {
    indexer: &'a InscriptionIndexer,
}

impl<'a> BlockEventExecutor<'a> {
    pub(super) fn new(indexer: &'a InscriptionIndexer) -> Self {
        Self { indexer }
    }

    pub(super) async fn execute(
        &self,
        ordered_events: Vec<BlockProcessEvent>,
    ) -> Result<(usize, usize), String> {
        let mut new_inscriptions_count = 0usize;
        let mut transfer_count = 0usize;

        for event in ordered_events {
            match event {
                BlockProcessEvent::Mint(item) => {
                    self.indexer.on_new_inscription(&item).await?;
                    new_inscriptions_count += 1;
                }
                BlockProcessEvent::Transfer(item) => {
                    match item.to_address {
                        Some(addr) => {
                            info!(
                                "Inscription {} transferred from {} to {} at block {}",
                                item.inscription_id, item.from_address, addr, item.block_height
                            );

                            self.indexer
                                .miner_pass_manager
                                .on_pass_transfer(
                                    &item.inscription_id,
                                    &addr,
                                    &item.satpoint,
                                    item.block_height,
                                )
                                .await?;
                        }
                        None => {
                            info!(
                                "Inscription {} burned from {} at block {}",
                                item.inscription_id, item.from_address, item.block_height
                            );

                            self.indexer
                                .miner_pass_manager
                                .on_pass_burned(&item.inscription_id, item.block_height)
                                .await?;
                        }
                    }
                    transfer_count += 1;
                }
            }
        }

        Ok((new_inscriptions_count, transfer_count))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::content::{USDBInscription, USDBMint};
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{
        Amount, Block, Network, ScriptBuf, Sequence, Transaction, TxIn, TxOut, Witness, absolute,
        constants, transaction,
    };
    use ord::InscriptionId;
    use ordinals::SatPoint;
    use usdb_util::{ToUSDBScriptHash, USDBScriptHash};

    fn test_script_hash(tag: u8) -> USDBScriptHash {
        ScriptBuf::from(vec![tag; 32]).to_usdb_script_hash()
    }

    fn test_transaction(input_outpoints: Vec<OutPoint>) -> Transaction {
        Transaction {
            version: transaction::Version::TWO,
            lock_time: absolute::LockTime::ZERO,
            input: input_outpoints
                .into_iter()
                .map(|prev| TxIn {
                    previous_output: prev,
                    script_sig: ScriptBuf::new(),
                    sequence: Sequence::MAX,
                    witness: Witness::default(),
                })
                .collect(),
            output: vec![TxOut {
                value: Amount::from_sat(1_000),
                script_pubkey: ScriptBuf::new(),
            }],
        }
    }

    fn test_mint_item(txid: Txid, index: u32, owner_tag: u8) -> InscriptionNewItem {
        InscriptionNewItem {
            inscription_id: InscriptionId { txid, index },
            inscription_number: index as i32,
            block_height: 1,
            timestamp: 0,
            address: test_script_hash(owner_tag),
            satpoint: SatPoint {
                outpoint: OutPoint { txid, vout: 0 },
                offset: 0,
            },
            value: Amount::from_sat(10_000),
            content_string: "{\"p\":\"usdb\",\"op\":\"mint\"}".to_string(),
            content: USDBInscription::Mint(USDBMint {
                eth_main: "0x1111111111111111111111111111111111111111".to_string(),
                eth_collab: None,
                prev: Vec::new(),
            }),
            op: crate::inscription::InscriptionOperation::Inscribe,
            commit_txid: Txid::from_slice(&[owner_tag; 32]).unwrap(),
        }
    }

    fn test_transfer_item(
        inscription_tag: u8,
        txid: Txid,
        prev_outpoint: OutPoint,
        from_tag: u8,
        to_tag: u8,
    ) -> InscriptionTransferItem {
        InscriptionTransferItem {
            inscription_id: InscriptionId {
                txid: Txid::from_slice(&[inscription_tag; 32]).unwrap(),
                index: 0,
            },
            block_height: 1,
            prev_satpoint: SatPoint {
                outpoint: prev_outpoint,
                offset: 0,
            },
            satpoint: SatPoint {
                outpoint: OutPoint { txid, vout: 0 },
                offset: 0,
            },
            from_address: test_script_hash(from_tag),
            to_address: Some(test_script_hash(to_tag)),
        }
    }

    #[test]
    fn test_block_event_planner_orders_by_tx_priority_input_and_inscription() {
        let input0 = OutPoint {
            txid: Txid::from_slice(&[1u8; 32]).unwrap(),
            vout: 0,
        };
        let input1 = OutPoint {
            txid: Txid::from_slice(&[2u8; 32]).unwrap(),
            vout: 1,
        };
        let tx = test_transaction(vec![input0, input1]);
        let txid = tx.compute_txid();
        let block = Arc::new(Block {
            header: constants::genesis_block(Network::Bitcoin).header,
            txdata: vec![tx],
        });

        let mint_high = test_mint_item(txid, 2, 10);
        let mint_low = test_mint_item(txid, 0, 11);
        let transfer_input1 = test_transfer_item(21, txid, input1, 3, 4);
        let transfer_input0 = test_transfer_item(20, txid, input0, 5, 6);

        let ordered = BlockEventPlanner::new(
            900_000,
            block,
            vec![mint_high, mint_low],
            vec![transfer_input1, transfer_input0],
        )
        .plan()
        .unwrap();

        assert_eq!(ordered.len(), 4);
        match &ordered[0] {
            BlockProcessEvent::Transfer(item) => {
                assert_eq!(item.prev_satpoint.outpoint, input0);
            }
            _ => panic!("expected first event to be transfer(input0)"),
        }
        match &ordered[1] {
            BlockProcessEvent::Transfer(item) => {
                assert_eq!(item.prev_satpoint.outpoint, input1);
            }
            _ => panic!("expected second event to be transfer(input1)"),
        }
        match &ordered[2] {
            BlockProcessEvent::Mint(item) => {
                assert_eq!(item.inscription_id.index, 0);
            }
            _ => panic!("expected third event to be mint(index=0)"),
        }
        match &ordered[3] {
            BlockProcessEvent::Mint(item) => {
                assert_eq!(item.inscription_id.index, 2);
            }
            _ => panic!("expected fourth event to be mint(index=2)"),
        }
    }

    #[test]
    fn test_block_event_planner_fails_when_transfer_input_missing() {
        let input0 = OutPoint {
            txid: Txid::from_slice(&[1u8; 32]).unwrap(),
            vout: 0,
        };
        let tx = test_transaction(vec![input0]);
        let txid = tx.compute_txid();
        let block = Arc::new(Block {
            header: constants::genesis_block(Network::Bitcoin).header,
            txdata: vec![tx],
        });
        let missing_input = OutPoint {
            txid: Txid::from_slice(&[8u8; 32]).unwrap(),
            vout: 0,
        };
        let transfer = test_transfer_item(31, txid, missing_input, 7, 8);

        let err = match BlockEventPlanner::new(900_001, block, Vec::new(), vec![transfer]).plan() {
            Ok(_) => panic!("expected planner to fail for missing transfer input"),
            Err(err) => err,
        };
        assert!(err.contains("Transfer input not found"));
    }
}
