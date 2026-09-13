use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use bitcoincore_rpc::bitcoin::{Amount, Block, OutPoint};
use usdb_util::BTCRpcClientRef;

/// A bounded, lazy input-value context for one exact block, shared by mint and transfer processing.
/// Core undo retains historical spent values independently of balance-history's own undo window.
pub struct UTXOValueManager {
    btc_client: BTCRpcClientRef,
    height: u32,
    block: Arc<Block>,
    values: Mutex<Option<HashMap<OutPoint, Amount>>>,
}

impl UTXOValueManager {
    /// Bind an empty context to the block whose transactions will consume these inputs.
    pub fn new(btc_client: BTCRpcClientRef, height: u32, block: Arc<Block>) -> Self {
        Self {
            btc_client,
            height,
            block,
            values: Mutex::new(None),
        }
    }

    /// Match both height and full block bytes before reusing a previous context.
    pub fn matches(&self, height: u32, block: &Block) -> bool {
        self.height == height && *self.block == *block
    }

    /// Return a spent input value, loading one complete, checked verbosity-3 response on first use.
    /// Failed loads are not cached; retries can succeed after Core makes block undo available.
    pub async fn get_utxo(&self, outpoint: &OutPoint) -> Result<Amount, String> {
        let mut values = self.values.lock().unwrap();
        if values.is_none() {
            *values = Some(
                self.btc_client
                    .get_block_input_values(self.height, &self.block)?,
            );
        }
        values.as_ref().unwrap().get(outpoint).copied().ok_or_else(|| {
            let msg = format!("Outpoint is not an input of the processing block: height={}, outpoint={outpoint}", self.height);
            error!("{msg}");
            msg
        })
    }
}

pub type UTXOValueManagerRef = Arc<UTXOValueManager>;
