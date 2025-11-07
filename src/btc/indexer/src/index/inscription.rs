use bitcoincore_rpc::bitcoin::{
    Amount, Txid,
    address::{Address, NetworkUnchecked},
};
use ord::InscriptionId;
use ordinals::SatPoint;

pub struct InscriptionNewItem {
    pub inscription_id: InscriptionId,
    pub inscription_number: u64,
    pub block_height: u64,
    pub timestamp: u32,
    pub address: Address<NetworkUnchecked>, // The creator address
    pub satpoint: SatPoint,
    pub value: Amount,
    pub content: String,
    pub commit_txid: Txid,
}

impl InscriptionNewItem {
    pub fn txid(&self) -> &Txid {
        &self.satpoint.outpoint.txid
    }
}

pub struct InscriptionTransferItem {
    pub inscription_id: InscriptionId,
    pub inscription_number: u64,
    pub block_height: u64,
    pub timestamp: u32,
    pub satpoint: SatPoint,
    pub prev_satpoint: Option<SatPoint>,

    // When transfer, to_address is None means burn as fee
    pub from_address: Address<NetworkUnchecked>,
    pub to_address: Option<Address<NetworkUnchecked>>,
    pub value: Amount,

    pub content: String,
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
    block_height: u64,
    new_inscriptions: Vec<InscriptionNewItem>,
    transfer_inscriptions: Vec<InscriptionTransferItem>,
}

impl BlockInscriptionsCollector {
    pub fn new(block_height: u64) -> Self {
        Self {
            block_height,
            new_inscriptions: Vec::new(),
            transfer_inscriptions: Vec::new(),
        }
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

    pub fn block_height(&self) -> u64 {
        self.block_height
    }

    pub fn new_inscriptions(&self) -> &Vec<InscriptionNewItem> {
        &self.new_inscriptions
    }

    pub fn transfer_inscriptions(&self) -> &Vec<InscriptionTransferItem> {
        &self.transfer_inscriptions
    }
}
