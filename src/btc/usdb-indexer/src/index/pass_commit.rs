use balance_history::BlockCommitInfo as BalanceHistoryBlockCommitInfo;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PASS_COMMIT_PROTOCOL_VERSION: &str = "1.0.0";
pub const PASS_COMMIT_HASH_ALGO: &str = "sha256";

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PassBlockMutation {
    Mint {
        inscription_id: String,
        inscription_number: i32,
        mint_owner: String,
        satpoint: String,
        eth_main: String,
        eth_collab: Option<String>,
        prev: Vec<String>,
    },
    InvalidMint {
        inscription_id: String,
        inscription_number: i32,
        mint_owner: String,
        satpoint: String,
        error_code: String,
        error_reason: String,
    },
    StateTransition {
        inscription_id: String,
        from_state: String,
        to_state: String,
        owner: String,
        satpoint: String,
    },
    OwnerTransfer {
        inscription_id: String,
        state: String,
        from_owner: String,
        to_owner: String,
        from_satpoint: String,
        to_satpoint: String,
    },
    SatpointUpdate {
        inscription_id: String,
        state: String,
        owner: String,
        from_satpoint: String,
        to_satpoint: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PassBlockCommitEntry {
    // Local usdb-indexer block height whose pass state transitions are being committed.
    pub block_height: u32,
    // Upstream balance-history block height used as the anchor for this local commit.
    // In pass commit v1 this is required to equal block_height; it is stored separately so the
    // external anchor source remains explicit in the persisted/RPC schema.
    pub balance_history_block_height: u32,
    // Upstream balance-history logical block commit at the same anchor height.
    // Because balance-history block_commit already commits to its btc_block_hash, pass commit v1
    // does not need to duplicate the upstream block hash here while the two heights stay equal.
    pub balance_history_block_commit: String,
    // Hash of the normalized pass mutation stream collected for this block only.
    pub mutation_root: String,
    // Rolling local pass block commit derived from previous local commit + upstream anchor + mutation_root.
    pub block_commit: String,
    // Version of the local pass commit protocol used to hash this entry.
    pub commit_protocol_version: String,
    // Hash algorithm name used by mutation_root and block_commit.
    pub commit_hash_algo: String,
}

#[derive(Debug, Clone, Default)]
pub struct PassBlockMutationCollector {
    // Block height for which this collector is recording pass mutations.
    block_height: u32,
    // Ordered logical mutation stream emitted by pass-state transitions in this block.
    mutations: Vec<PassBlockMutation>,
}

impl PassBlockMutationCollector {
    pub fn new(block_height: u32) -> Self {
        Self {
            block_height,
            mutations: Vec::new(),
        }
    }

    pub fn push(&mut self, mutation: PassBlockMutation) {
        self.mutations.push(mutation);
    }

    pub fn block_height(&self) -> u32 {
        self.block_height
    }

    pub fn mutations(&self) -> &[PassBlockMutation] {
        &self.mutations
    }

    pub fn mutation_root(&self) -> Result<String, String> {
        let mut hasher = Sha256::new();
        hasher.update(PASS_COMMIT_PROTOCOL_VERSION.as_bytes());
        hasher.update(b"|");
        hasher.update(self.block_height.to_be_bytes());
        hasher.update(b"|");
        hasher.update((self.mutations.len() as u32).to_be_bytes());

        for mutation in &self.mutations {
            let encoded = serde_json::to_vec(mutation).map_err(|e| {
                let msg = format!("Failed to serialize pass block mutation: {}", e);
                error!("{}", msg);
                msg
            })?;
            hasher.update(b"|");
            hasher.update((encoded.len() as u32).to_be_bytes());
            hasher.update(encoded);
        }

        Ok(encode_hex(&hasher.finalize()))
    }

    pub fn build_commit_entry(
        &self,
        upstream_commit: &BalanceHistoryBlockCommitInfo,
        prev_local_commit: Option<&PassBlockCommitEntry>,
    ) -> Result<PassBlockCommitEntry, String> {
        // Pass commit v1 intentionally anchors block N only to balance-history block commit N.
        // If future protocol revisions allow cross-height anchoring, this invariant and schema
        // will need to change because upstream block_commit would no longer imply local N's hash.
        if upstream_commit.block_height != self.block_height {
            let msg = format!(
                "Balance-history block commit height mismatch: local_block_height={}, upstream_block_height={}",
                self.block_height, upstream_commit.block_height
            );
            error!("{}", msg);
            return Err(msg);
        }

        let mutation_root = self.mutation_root()?;
        let previous_commit = prev_local_commit
            .map(|entry| entry.block_commit.as_str())
            .unwrap_or("genesis");

        let mut hasher = Sha256::new();
        hasher.update(PASS_COMMIT_PROTOCOL_VERSION.as_bytes());
        hasher.update(b"|");
        hasher.update(self.block_height.to_be_bytes());
        hasher.update(b"|");
        hasher.update(previous_commit.as_bytes());
        hasher.update(b"|");
        hasher.update(upstream_commit.block_height.to_be_bytes());
        hasher.update(b"|");
        hasher.update(upstream_commit.block_commit.as_bytes());
        hasher.update(b"|");
        hasher.update(mutation_root.as_bytes());

        Ok(PassBlockCommitEntry {
            block_height: self.block_height,
            balance_history_block_height: upstream_commit.block_height,
            balance_history_block_commit: upstream_commit.block_commit.clone(),
            mutation_root,
            block_commit: encode_hex(&hasher.finalize()),
            commit_protocol_version: PASS_COMMIT_PROTOCOL_VERSION.to_string(),
            commit_hash_algo: PASS_COMMIT_HASH_ALGO.to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pass_block_commit_collector_empty_mutation_root_is_stable() {
        let collector = PassBlockMutationCollector::new(100);
        let root = collector.mutation_root().unwrap();
        assert_eq!(root.len(), 64);
    }

    #[test]
    fn test_pass_block_commit_collector_builds_commit_entry() {
        let mut collector = PassBlockMutationCollector::new(120);
        collector.push(PassBlockMutation::Mint {
            inscription_id: "a".repeat(64) + "i0",
            inscription_number: 1,
            mint_owner: "owner".to_string(),
            satpoint: "satpoint".to_string(),
            eth_main: "0x1".to_string(),
            eth_collab: None,
            prev: Vec::new(),
        });

        let upstream = BalanceHistoryBlockCommitInfo {
            block_height: 120,
            btc_block_hash: "11".repeat(32),
            balance_delta_root: "22".repeat(32),
            block_commit: "33".repeat(32),
            commit_protocol_version: "1.0.0".to_string(),
            commit_hash_algo: "sha256".to_string(),
        };
        let entry = collector.build_commit_entry(&upstream, None).unwrap();
        assert_eq!(entry.block_height, 120);
        assert_eq!(entry.balance_history_block_height, 120);
        assert_eq!(entry.balance_history_block_commit, "33".repeat(32));
        assert_eq!(entry.commit_protocol_version, PASS_COMMIT_PROTOCOL_VERSION);
        assert_eq!(entry.commit_hash_algo, PASS_COMMIT_HASH_ALGO);
        assert_eq!(entry.mutation_root.len(), 64);
        assert_eq!(entry.block_commit.len(), 64);
    }
}
