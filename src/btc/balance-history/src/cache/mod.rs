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
    // 尽力缓存策略，用以块极度落后情况下，使用local loader快速加载
    // Best Effort strategy, used when the block is extremely behind, to use local loader for quick loading
    BestEffort,

    // 常规缓存策略，用以块同步完成后，使用较小缓存占用
    // Regular caching strategy, used after block synchronization is complete, using smaller cache occupancy
    Normal,
}