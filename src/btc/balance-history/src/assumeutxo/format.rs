//! Streaming reader for Bitcoin Core snapshot format v2 and hash_serialized_3.
// Encoding references: Bitcoin Core v31.1 coins.h, compressor.cpp and kernel/coinstats.cpp
// (Bitcoin Core developers, MIT license). The commitment hashes decoded Coin contents.

use std::fs::File;
use std::io::{BufReader, Read};
use std::path::Path;
use std::time::Instant;

use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{BlockHash, Network, OutPoint, ScriptBuf, Txid};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAX_MONEY: u64 = 21_000_000 * 100_000_000;
const MAX_GROUP_COINS: u64 = 1_000_000;

/// Trusted input identity; mainnet callers additionally pin the Core 935000 constants.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotIdentity {
    /// Bitcoin network represented by the snapshot.
    pub network: Network,
    /// Height of the already-applied baseline block.
    pub base_height: u32,
    /// Canonical displayed Bitcoin block hash of the baseline.
    pub base_hash: String,
    /// SHA-256 of the exact downloaded file.
    pub file_sha256: String,
    /// Core's double-SHA-256 commitment to the ordered decoded UTXO records.
    pub hash_serialized_3: String,
}

/// A decoded Coin, including fields that are not stored in balance-history's UTXO projection.
#[derive(Clone, Debug)]
pub struct SnapshotCoin {
    /// Transaction output key.
    pub outpoint: OutPoint,
    /// Creation height of this output.
    pub height: u32,
    /// Whether the creating transaction was coinbase.
    pub coinbase: bool,
    /// Output amount in satoshis; zero-valued outputs are retained.
    pub value: u64,
    /// Original locking script, without address or wallet filtering.
    pub script: ScriptBuf,
}

/// Results produced only after strict EOF, file hash and logical commitment validation.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotScan {
    /// Number of decoded live outputs.
    pub coins: u64,
    /// Number of distinct transaction groups.
    pub transactions: u64,
    /// Sum of all output values in satoshis.
    pub total_satoshis: u64,
    /// Exact input byte count consumed by the reader.
    pub bytes: u64,
    /// Measured wall-clock duration of this scan, including callback work.
    pub elapsed_seconds: f64,
    /// Verified input identity.
    pub identity: SnapshotIdentity,
}

struct HashedReader<R> {
    inner: R,
    hash: Sha256,
    bytes: u64,
}

impl<R: Read> Read for HashedReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(buf)?;
        self.hash.update(&buf[..count]);
        self.bytes += count as u64;
        Ok(count)
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(result, "{byte:02x}").expect("String formatting cannot fail");
    }
    result
}

pub(crate) fn hash_bytes(value: &str) -> Result<[u8; 32], String> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("Expected a 64-character hexadecimal hash".to_string());
    }
    let mut bytes = [0; 32];
    for (i, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[2 * i..2 * i + 2], 16).map_err(|e| e.to_string())?;
    }
    Ok(bytes)
}

fn read_array<const N: usize>(reader: &mut impl Read) -> Result<[u8; N], String> {
    let mut buf = [0; N];
    reader
        .read_exact(&mut buf)
        .map_err(|e| format!("Truncated snapshot: {e}"))?;
    Ok(buf)
}

fn compact_size(reader: &mut impl Read) -> Result<u64, String> {
    let first = read_array::<1>(reader)?[0];
    let (value, minimum) = match first {
        253 => (u64::from(u16::from_le_bytes(read_array(reader)?)), 253),
        254 => (u64::from(u32::from_le_bytes(read_array(reader)?)), 0x10000),
        255 => (u64::from_le_bytes(read_array(reader)?), 0x1_0000_0000),
        n => (u64::from(n), 0),
    };
    if value < minimum {
        return Err("Noncanonical CompactSize in snapshot".to_string());
    }
    Ok(value)
}

fn varint(reader: &mut impl Read) -> Result<u64, String> {
    let mut n = 0u64;
    loop {
        let byte = read_array::<1>(reader)?[0];
        n = n
            .checked_mul(128)
            .and_then(|n| n.checked_add(u64::from(byte & 127)))
            .ok_or("Snapshot VARINT overflow")?;
        if byte & 128 == 0 {
            return Ok(n);
        }
        n = n.checked_add(1).ok_or("Snapshot VARINT overflow")?;
    }
}

fn amount(mut n: u64) -> Result<u64, String> {
    if n == 0 {
        return Ok(0);
    }
    n -= 1;
    let exponent = (n % 10) as u32;
    n /= 10;
    let value = if exponent < 9 {
        (n / 9)
            .checked_mul(10)
            .and_then(|v| v.checked_add(n % 9 + 1))
    } else {
        n.checked_add(1)
    }
    .and_then(|v| v.checked_mul(10u64.pow(exponent)))
    .ok_or("Compressed amount overflow")?;
    if value > MAX_MONEY {
        return Err("Snapshot amount exceeds MAX_MONEY".to_string());
    }
    Ok(value)
}

fn script(reader: &mut impl Read) -> Result<ScriptBuf, String> {
    let code = varint(reader)?;
    let bytes = match code {
        0 | 1 => {
            let hash = read_array::<20>(reader)?;
            if code == 0 {
                [vec![0x76, 0xa9, 20], hash.to_vec(), vec![0x88, 0xac]].concat()
            } else {
                [vec![0xa9, 20], hash.to_vec(), vec![0x87]].concat()
            }
        }
        2..=5 => {
            let mut key = [0u8; 33];
            key[0] = if code >= 4 {
                code as u8 - 2
            } else {
                code as u8
            };
            key[1..].copy_from_slice(&read_array::<32>(reader)?);
            if code < 4 {
                // Core preserves compressed public keys verbatim, including invalid curve points.
                [vec![33], key.to_vec(), vec![0xac]].concat()
            } else {
                let key = bitcoincore_rpc::bitcoin::secp256k1::PublicKey::from_slice(&key)
                    .map_err(|e| format!("Invalid compressed uncompressed-key script: {e}"))?;
                [vec![65], key.serialize_uncompressed().to_vec(), vec![0xac]].concat()
            }
        }
        _ => {
            let length = code - 6;
            if length > 10_000 {
                return Err("Snapshot script exceeds 10000 bytes".to_string());
            }
            let mut bytes = vec![0; length as usize];
            reader
                .read_exact(&mut bytes)
                .map_err(|e| format!("Truncated snapshot script: {e}"))?;
            bytes
        }
    };
    if bytes.first() == Some(&0x6a) {
        return Err("Unspendable OP_RETURN output in live UTXO snapshot".to_string());
    }
    Ok(ScriptBuf::from_bytes(bytes))
}

fn hash_coin(hash: &mut Sha256, coin: &SnapshotCoin) {
    hash.update(coin.outpoint.txid.as_byte_array());
    hash.update(coin.outpoint.vout.to_le_bytes());
    hash.update(((coin.height << 1) | u32::from(coin.coinbase)).to_le_bytes());
    hash.update(coin.value.to_le_bytes());
    // CTxOut uses CompactSize for its uncompressed script length.
    let n = coin.script.len() as u64;
    if n < 253 {
        hash.update([n as u8]);
    } else if n <= 65535 {
        hash.update([253]);
        hash.update((n as u16).to_le_bytes());
    } else {
        hash.update([254]);
        hash.update((n as u32).to_le_bytes());
    }
    hash.update(coin.script.as_bytes());
}

/// Decode a snapshot with bounded transaction groups and batches, invoking the callback in key order.
/// Callback writes must remain unpublished until this function successfully verifies both hashes.
/// Resuming callers can rescan the prefix without applying previously committed batches again.
pub fn scan_snapshot(
    path: &Path,
    expected: &SnapshotIdentity,
    batch_size: usize,
    mut on_batch: impl FnMut(&[SnapshotCoin], u64) -> Result<(), String>,
) -> Result<SnapshotScan, String> {
    if !(1..=1_000_000).contains(&batch_size) {
        return Err("Invalid snapshot batch size".to_string());
    }
    hash_bytes(&expected.file_sha256)?;
    hash_bytes(&expected.hash_serialized_3)?;
    let expected_hash: BlockHash = expected
        .base_hash
        .parse()
        .map_err(|e| format!("Invalid base hash: {e}"))?;
    if expected.base_height >= 1 << 31 {
        return Err("Snapshot height exceeds Coin encoding".to_string());
    }
    let start = Instant::now();
    let file =
        File::open(path).map_err(|e| format!("Cannot open snapshot {}: {e}", path.display()))?;
    let mut reader = BufReader::with_capacity(
        4 * 1024 * 1024,
        HashedReader {
            inner: file,
            hash: Sha256::new(),
            bytes: 0,
        },
    );
    if read_array::<5>(&mut reader)? != *b"utxo\xff"
        || u16::from_le_bytes(read_array(&mut reader)?) != 2
    {
        return Err("Expected Bitcoin Core snapshot format v2".to_string());
    }
    let network_magic = match expected.network {
        Network::Bitcoin => [0xf9, 0xbe, 0xb4, 0xd9],
        Network::Regtest => [0xfa, 0xbf, 0xb5, 0xda],
        Network::Testnet => [0x0b, 0x11, 0x09, 0x07],
        Network::Signet => [0x0a, 0x03, 0xcf, 0x40],
        Network::Testnet4 => [0x1c, 0x16, 0x3f, 0x28],
    };
    if read_array::<4>(&mut reader)? != network_magic {
        return Err("Snapshot network mismatch".to_string());
    }
    if read_array::<32>(&mut reader)? != *expected_hash.as_byte_array() {
        return Err("Snapshot base hash mismatch".to_string());
    }
    let total = u64::from_le_bytes(read_array(&mut reader)?);
    let mut hash = Sha256::new();
    let mut coins = 0u64;
    let mut transactions = 0u64;
    let mut total_satoshis = 0u64;
    let mut previous = None;
    let mut batch = Vec::with_capacity(batch_size);
    let mut progress = Instant::now();
    eprintln!(
        "AssumeUTXO scan started: path={}, coins={total}, base_height={}",
        path.display(),
        expected.base_height
    );
    while coins < total {
        let txid = read_array::<32>(&mut reader)?;
        if previous.is_some_and(|prev| prev >= txid) {
            return Err("Snapshot transaction groups are duplicated or out of order".to_string());
        }
        previous = Some(txid);
        let count = compact_size(&mut reader)?;
        if count == 0 || count > MAX_GROUP_COINS || count > total - coins {
            return Err("Invalid snapshot transaction group size".to_string());
        }
        let mut group = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let vout =
                u32::try_from(compact_size(&mut reader)?).map_err(|_| "Snapshot vout overflow")?;
            if vout == u32::MAX {
                return Err("Null output index in snapshot".to_string());
            }
            let code = varint(&mut reader)?;
            if code >> 1 > u64::from(expected.base_height) {
                return Err("Snapshot Coin height exceeds baseline".to_string());
            }
            let value = amount(varint(&mut reader)?)?;
            total_satoshis = total_satoshis
                .checked_add(value)
                .filter(|v| *v <= MAX_MONEY)
                .ok_or("Snapshot total exceeds MAX_MONEY")?;
            group.push(SnapshotCoin {
                outpoint: OutPoint {
                    txid: Txid::from_byte_array(txid),
                    vout,
                },
                height: (code >> 1) as u32,
                coinbase: code & 1 == 1,
                value,
                script: script(&mut reader)?,
            });
        }
        // The DB cursor's VARINT order is not guaranteed to be numeric vout order.
        group.sort_unstable_by_key(|coin| coin.outpoint.vout);
        if group
            .windows(2)
            .any(|v| v[0].outpoint.vout == v[1].outpoint.vout)
        {
            return Err("Duplicate output in snapshot group".to_string());
        }
        for coin in group {
            hash_coin(&mut hash, &coin);
            batch.push(coin);
            coins += 1;
            if batch.len() == batch_size {
                on_batch(&batch, coins)?;
                batch.clear();
            }
        }
        transactions += 1;
        if progress.elapsed().as_secs() >= 10 {
            eprintln!(
                "AssumeUTXO scan progress: coins={coins}, total={total}, elapsed_seconds={:.1}",
                start.elapsed().as_secs_f64()
            );
            progress = Instant::now();
        }
    }
    if !batch.is_empty() {
        on_batch(&batch, coins)?;
    }
    if reader.read(&mut [0u8; 1]).map_err(|e| e.to_string())? != 0 {
        return Err("Trailing bytes after snapshot coins".to_string());
    }
    let source = reader.into_inner();
    let file_hash = hex(&source.hash.finalize());
    let mut commitment = Sha256::digest(hash.finalize()).to_vec();
    commitment.reverse();
    let commitment = hex(&commitment);
    if file_hash != expected.file_sha256.to_ascii_lowercase() {
        return Err(format!("Snapshot file SHA256 mismatch: actual={file_hash}"));
    }
    if commitment != expected.hash_serialized_3.to_ascii_lowercase() {
        return Err(format!(
            "Snapshot UTXO commitment mismatch: actual={commitment}"
        ));
    }
    let result = SnapshotScan {
        coins,
        transactions,
        total_satoshis,
        bytes: source.bytes,
        elapsed_seconds: start.elapsed().as_secs_f64(),
        identity: expected.clone(),
    };
    eprintln!(
        "AssumeUTXO scan finished: coins={coins}, elapsed_seconds={:.1}, hash_serialized_3={commitment}",
        result.elapsed_seconds
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn amount_vectors_and_integer_bounds() {
        assert_eq!(amount(0).unwrap(), 0);
        assert_eq!(amount(1).unwrap(), 1);
        assert_eq!(amount(9).unwrap(), 100_000_000);
        assert_eq!(amount(50).unwrap(), 5_000_000_000);
        assert!(amount(u64::MAX).is_err());
        assert!(varint(&mut &[0xff; 12][..]).is_err());
        assert!(compact_size(&mut &[253, 1, 0][..]).is_err());
    }

    #[test]
    fn compressed_script_forms_and_zero_length() {
        assert_eq!(script(&mut &[6][..]).unwrap().len(), 0);
        assert!(script(&mut &[7, 0x6a][..]).is_err());
        for code in 0..=1 {
            let mut bytes = vec![code];
            bytes.extend([0x12; 20]);
            assert_eq!(
                script(&mut bytes.as_slice()).unwrap().len(),
                if code == 0 { 25 } else { 23 }
            );
        }
        let x =
            hash_bytes("79be667ef9dcbbac55a06295ce870b07029bfcdb2dce28d959f2815b16f81798").unwrap();
        for code in 2..=5 {
            let mut bytes = vec![code];
            bytes.extend(x);
            let decoded = script(&mut bytes.as_slice()).unwrap();
            assert_eq!(decoded.len(), if code < 4 { 35 } else { 67 });
            assert_eq!(decoded.as_bytes().last(), Some(&0xac));
        }
    }
}
