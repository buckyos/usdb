use super::content::USDBInscription;
use bitcoincore_rpc::bitcoin::{
    Amount, Txid,
};
use ord::InscriptionId;
use ordinals::SatPoint;
use std::str::FromStr;
use usdb_util::USDBScriptHash;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InscriptionOperation {
    Inscribe,
    Transfer,
}

impl InscriptionOperation {
    pub fn as_str(&self) -> &str {
        match self {
            InscriptionOperation::Inscribe => "inscribe",
            InscriptionOperation::Transfer => "transfer",
        }
    }

    pub fn need_track_transfer(&self) -> bool {
        match self {
            InscriptionOperation::Inscribe => true,
            InscriptionOperation::Transfer => true,
        }
    }
}

impl FromStr for InscriptionOperation {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "inscribe" => Ok(InscriptionOperation::Inscribe),
            "transfer" => Ok(InscriptionOperation::Transfer),
            _ => Err(format!("Invalid inscription operation: {}", s)),
        }
    }
}

#[derive(Clone)]
pub struct InscriptionNewItem {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub block_height: u32,
    pub timestamp: u32,
    pub address: USDBScriptHash, // The creator address
    pub satpoint: SatPoint,
    pub value: Amount,

    pub content_string: String,
    pub content: USDBInscription,
    pub op: InscriptionOperation,

    pub commit_txid: Txid,
}

impl InscriptionNewItem {
    pub fn txid(&self) -> &Txid {
        &self.satpoint.outpoint.txid
    }
}

#[derive(Clone)]
pub struct InscriptionTransferItem {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub block_height: u32,
    pub timestamp: u32,
    pub satpoint: SatPoint,
    pub prev_satpoint: Option<SatPoint>,

    // When transfer, to_address is None means burn as fee
    pub from_address: USDBScriptHash,
    pub to_address: Option<USDBScriptHash>,
    pub value: Amount,

    pub content: String,
    pub op: InscriptionOperation,
    pub index: u64, // Index indicates the number of transfers
}

impl InscriptionTransferItem {
    pub fn set_prev_satpoint(&mut self, prev_satpoint: SatPoint) {
        assert!(self.prev_satpoint.is_none(), "prev_satpoint is already set");
        self.prev_satpoint = Some(prev_satpoint);
    }

    pub fn txid(&self) -> &Txid {
        &self.satpoint.outpoint.txid
    }
}

pub struct BlockInscriptionsCollector {
    block_height: u32,
    new_inscriptions: Vec<InscriptionNewItem>,
    transfer_inscriptions: Vec<InscriptionTransferItem>,
}

impl BlockInscriptionsCollector {
    pub fn new(block_height: u32) -> Self {
        Self {
            block_height,
            new_inscriptions: Vec::new(),
            transfer_inscriptions: Vec::new(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.new_inscriptions.is_empty() && self.transfer_inscriptions.is_empty()
    }

    pub fn add_new_inscription(&mut self, item: InscriptionNewItem) {
        self.new_inscriptions.push(item);
    }

    pub fn add_new_inscriptions(&mut self, items: Vec<InscriptionNewItem>) {
        self.new_inscriptions.extend(items);
    }

    pub fn add_transfer_inscription(&mut self, item: InscriptionTransferItem) {
        self.transfer_inscriptions.push(item);
    }

    pub fn add_transfer_inscriptions(&mut self, items: Vec<InscriptionTransferItem>) {
        self.transfer_inscriptions.extend(items);
    }

    pub fn block_height(&self) -> u32 {
        self.block_height
    }

    pub fn new_inscriptions(&self) -> &Vec<InscriptionNewItem> {
        &self.new_inscriptions
    }

    pub fn transfer_inscriptions(&self) -> &Vec<InscriptionTransferItem> {
        &self.transfer_inscriptions
    }
}
