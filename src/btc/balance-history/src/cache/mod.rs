mod balance;
mod block_file;
mod monitor;
mod utxo;

pub use balance::*;
pub use block_file::*;
pub use monitor::*;
pub use utxo::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStrategy {
    // Best Effort strategy, used when the block is extremely behind, to use local loader for quick loading
    BestEffort,

    // Regular caching strategy, used after block synchronization is complete, using smaller cache occupancy
    Normal,
}