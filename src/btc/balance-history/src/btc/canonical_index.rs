//! Incremental physical block locations. Entries are candidates, never canonical chain authority.

use std::fs::{self, File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use bitcoincore_rpc::bitcoin::{Block, BlockHash, block::Header, consensus, hashes::Hash};
use rust_rocksdb::{DB, Options, WriteBatch};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_BLOCK_BYTES: u32 = 4_000_000;
const SCAN_RECORD_BUDGET: usize = 4096;
const TAIL_RECHECK_SECONDS: u64 = 30;

#[derive(Clone, Debug, Serialize, Deserialize)]
struct Location {
    file: u32,
    offset: u64,
    size: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    len: u64,
    modified_ns: u64,
    device: u64,
    inode: u64,
    pending: bool,
    #[serde(default)]
    checked_at_seconds: u64,
}

fn file_identity(meta: &Metadata) -> (u64, u64) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        (meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        (0, 0)
    }
}

fn modified_ns(meta: &Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|t| t.as_nanos().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn read_xor_key(blocks: &Path) -> Result<[u8; 8], String> {
    match fs::read(blocks.join("xor.dat")) {
        Ok(bytes) => bytes
            .try_into()
            .map_err(|_| "Local blocks xor.dat must contain exactly 8 bytes".to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok([0; 8]),
        Err(error) => Err(format!("Read local block XOR key: {error}")),
    }
}

fn read_at(file: &mut File, offset: u64, bytes: &mut [u8], key: &[u8; 8]) -> Result<(), String> {
    file.seek(SeekFrom::Start(offset))
        .and_then(|_| file.read_exact(bytes))
        .map_err(|e| e.to_string())?;
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte ^= key[((offset % 8) as usize + i) % 8];
    }
    Ok(())
}

fn block_key(hash: &BlockHash) -> Vec<u8> {
    let mut key = vec![b'b'];
    key.extend_from_slice(hash.as_byte_array());
    key
}

/// Disposable persistent locations, isolated from authoritative balance-history state.
pub(super) struct CanonicalBlockIndex {
    blocks: PathBuf,
    magic: u32,
    xor: [u8; 8],
    db: DB,
}

impl CanonicalBlockIndex {
    pub(super) fn open(data_dir: &Path, cache_dir: &Path, magic: u32) -> Result<Self, String> {
        let blocks = fs::canonicalize(data_dir.join("blocks"))
            .map_err(|e| format!("Open local blocks directory: {e}"))?;
        let xor = read_xor_key(&blocks)?;
        // Separate source directories, networks and XOR keys without deleting an older cache.
        let mut identity = Sha256::new();
        identity.update(b"balance-history:canonical-block-index:v1");
        identity.update(blocks.as_os_str().as_encoded_bytes());
        identity.update(magic.to_be_bytes());
        identity.update(xor);
        let path = cache_dir.join(crate::assumeutxo::format::hex(&identity.finalize()));
        fs::create_dir_all(&path).map_err(|e| e.to_string())?;
        let mut options = Options::default();
        options.create_if_missing(true);
        options.set_max_open_files(64);
        options.set_write_buffer_size(8 * 1024 * 1024);
        options.set_max_write_buffer_number(2);
        let db = DB::open(&options, &path)
            .map_err(|e| format!("Open derived local block index: {e}"))?;
        Ok(Self {
            blocks,
            magic,
            xor,
            db,
        })
    }

    fn path(&self, index: u32) -> PathBuf {
        self.blocks.join(format!("blk{index:05}.dat"))
    }

    /// Reconsider every file independently, including the newest and noncontiguous file numbers.
    /// A bounded header scan avoids reading all historical block bodies before serving a batch.
    pub(super) fn refresh(&self, cancelled: &dyn Fn() -> bool) -> Result<usize, String> {
        if read_xor_key(&self.blocks)? != self.xor {
            return Err("Local block XOR identity changed; reopen the derived index".to_string());
        }
        let mut files = Vec::new();
        for entry in fs::read_dir(&self.blocks).map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if let Some(number) = name
                .strip_prefix("blk")
                .and_then(|s| s.strip_suffix(".dat"))
                && number.len() >= 5
                && number.bytes().all(|b| b.is_ascii_digit())
                && let Ok(index) = number.parse::<u32>()
            {
                files.push(index);
            }
        }
        // Recent files are more likely to contain post-snapshot blocks. No file is marked sealed.
        files.sort_unstable_by(|a, b| b.cmp(a));
        let mut count = 0;
        for index in files {
            if cancelled() {
                return Err("Local block indexing cancelled".to_string());
            }
            if count >= SCAN_RECORD_BUDGET {
                break;
            }
            match self.scan_file(index, SCAN_RECORD_BUDGET - count, cancelled) {
                Ok(scanned) => count += scanned,
                Err(error) => log::warn!(
                    "Local block file scan deferred: file={}, error={error}",
                    self.path(index).display()
                ),
            }
        }
        Ok(count)
    }

    fn scan_file(
        &self,
        index: u32,
        budget: usize,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<usize, String> {
        let mut file = File::open(self.path(index)).map_err(|e| e.to_string())?;
        let meta = file.metadata().map_err(|e| e.to_string())?;
        let (device, inode) = file_identity(&meta);
        let modified_ns = modified_ns(&meta);
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        let key = [b'f']
            .into_iter()
            .chain(index.to_be_bytes())
            .collect::<Vec<_>>();
        let saved: Option<Cursor> = self
            .db
            .get(&key)
            .map_err(|e| e.to_string())?
            .map(|bytes| serde_json::from_slice(&bytes))
            .transpose()
            .map_err(|e| e.to_string())?;
        let same_file = saved.as_ref().is_some_and(|c| {
            c.device == device && c.inode == inode && c.len <= meta.len() && c.offset <= meta.len()
        });
        if same_file
            && saved.as_ref().is_some_and(|c| {
                !c.pending
                    && c.len == meta.len()
                    && c.modified_ns == modified_ns
                    && now >= c.checked_at_seconds
                    && now - c.checked_at_seconds < TAIL_RECHECK_SECONDS
            })
        {
            return Ok(0);
        }
        let mut offset = if same_file {
            saved.as_ref().unwrap().offset
        } else {
            0
        };
        let mut retry_offset = offset;
        let mut count = 0;
        let mut batch = WriteBatch::default();
        while offset.saturating_add(88) <= meta.len() && count < budget {
            if cancelled() {
                return Err("Local block indexing cancelled".to_string());
            }
            let mut header = [0u8; 88];
            read_at(&mut file, offset, &mut header, &self.xor)?;
            let magic = u32::from_le_bytes(header[..4].try_into().unwrap());
            if magic != self.magic {
                // Core may preallocate raw zero bytes or write encrypted zero padding.
                let raw_zero = header
                    .iter()
                    .enumerate()
                    .all(|(i, b)| *b == self.xor[((offset % 8) as usize + i) % 8]);
                if header.iter().any(|b| *b != 0) && !raw_zero {
                    log::warn!(
                        "Local block tail is not a complete frame: file={index}, offset={offset}"
                    );
                }
                break;
            }
            let size = u32::from_le_bytes(header[4..8].try_into().unwrap());
            if !(81..=MAX_BLOCK_BYTES).contains(&size) || offset + 8 + u64::from(size) > meta.len()
            {
                break;
            }
            let block_header: Header =
                consensus::deserialize(&header[8..]).map_err(|e| e.to_string())?;
            let location = Location {
                file: index,
                offset,
                size,
            };
            batch.put(
                block_key(&block_header.block_hash()),
                serde_json::to_vec(&location).map_err(|e| e.to_string())?,
            );
            // Revisit the last candidate record after any subsequent write, even if size is unchanged.
            // Preallocation can make an unfinished payload look physically complete. Full transaction
            // and witness commitments are checked when a candidate is actually read for consumption.
            retry_offset = offset;
            offset += 8 + u64::from(size);
            count += 1;
        }
        let cursor = Cursor {
            offset: retry_offset,
            len: meta.len(),
            modified_ns,
            device,
            inode,
            pending: count == budget,
            checked_at_seconds: now,
        };
        batch.put(key, serde_json::to_vec(&cursor).map_err(|e| e.to_string())?);
        // Locations and their resume cursor advance together; losing a cache batch only causes rescan.
        self.db.write(&batch).map_err(|e| e.to_string())?;
        Ok(count)
    }

    /// Revalidate the entire payload on every read; a physical location is not a trust decision.
    pub(super) fn block(&self, hash: &BlockHash) -> Result<Option<Block>, String> {
        let Some(bytes) = self.db.get(block_key(hash)).map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        let location: Location = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        if !(81..=MAX_BLOCK_BYTES).contains(&location.size) {
            return Err("Invalid cached local block length".to_string());
        }
        let mut file = File::open(self.path(location.file)).map_err(|e| e.to_string())?;
        let mut bytes = vec![0; location.size as usize + 8];
        read_at(&mut file, location.offset, &mut bytes, &self.xor)?;
        if u32::from_le_bytes(bytes[..4].try_into().unwrap()) != self.magic
            || u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != location.size
        {
            return Err("Cached local block frame changed".to_string());
        }
        let block: Block = consensus::deserialize(&bytes[8..])
            .map_err(|e| format!("Incomplete or invalid local block: {e}"))?;
        validate_payload(&block, hash)?;
        Ok(Some(block))
    }
}

pub(super) fn validate_payload(block: &Block, hash: &BlockHash) -> Result<(), String> {
    // A Merkle tree with an odd leaf count has the same root after duplicating its last leaf.
    // Reject repeated txids so that a mutated local payload cannot exploit that ambiguity.
    let mut txids = std::collections::HashSet::new();
    if block.block_hash() != *hash
        || !block.check_merkle_root()
        || !block.check_witness_commitment()
        || block
            .txdata
            .iter()
            .any(|tx| !txids.insert(tx.compute_txid()))
    {
        return Err(
            "Local/RPC block does not match its requested hash or transaction commitments"
                .to_string(),
        );
    }
    Ok(())
}
