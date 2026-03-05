use super::super::content::USDBInscription;
use bitcoincore_rpc::bitcoin::Block;
use ord::InscriptionId;
use ordinals::SatPoint;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

pub type InscriptionSourceFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone)]
pub struct DiscoveredMint {
    pub inscription_id: InscriptionId,
    pub inscription_number: i32,
    pub block_height: u32,
    pub timestamp: u32,
    pub satpoint: Option<SatPoint>,
    pub content_string: String,
    pub content: USDBInscription,
}

pub trait InscriptionSource: Send + Sync {
    fn source_name(&self) -> &'static str;

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>>;
}
