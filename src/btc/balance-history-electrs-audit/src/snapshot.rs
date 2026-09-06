use crate::model::{SampleKind, SnapshotSummary};
use balance_history::snapshot_audit::{
    SplitSnapshotAudit, immutable_audit_uri, open_immutable_audit_db,
};
use balance_history::snapshot_contract::CORE_SNAPSHOT_MANIFEST_VERSION;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{Network, Script, ScriptBuf};
use rusqlite::{Connection, OptionalExtension};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs::File;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use usdb_util::{BtcScriptHash, ToBtcScriptHash, parse_script_hash_any};

#[derive(Clone, Debug)]
pub struct AuditSample {
    pub sample_id: String,
    pub kind: SampleKind,
    pub script_hash: BtcScriptHash,
    pub script_pubkey: ScriptBuf,
    pub script_type: String,
    pub expected_balance: u64,
    pub last_change_height: Option<u32>,
}

#[derive(Clone, Debug, Default)]
pub struct SamplePlanStats {
    pub blacklisted_candidates_replaced: usize,
    pub duplicate_candidates_replaced: usize,
}

#[derive(Clone, Debug)]
pub struct SamplePlan {
    pub samples: Vec<AuditSample>,
    pub stats: SamplePlanStats,
}

#[derive(Clone, Debug, Default)]
pub struct Blacklist {
    script_hashes: HashSet<BtcScriptHash>,
    pub id: String,
}

impl Blacklist {
    pub fn load(path: Option<&Path>, network: Network) -> Result<Self, String> {
        let mut script_hashes = HashSet::new();
        if let Some(path) = path {
            let file = File::open(path)
                .map_err(|error| format!("Failed to open blacklist {}: {error}", path.display()))?;
            for (index, line) in BufReader::new(file).lines().enumerate() {
                let line = line.map_err(|error| {
                    format!(
                        "Failed to read blacklist {} line {}: {error}",
                        path.display(),
                        index + 1
                    )
                })?;
                let value = line.split('#').next().unwrap_or_default().trim();
                if value.is_empty() {
                    continue;
                }
                let script_hash = parse_script_hash_any(value, &network).map_err(|error| {
                    format!(
                        "Invalid blacklist entry at {}:{}: {error}",
                        path.display(),
                        index + 1
                    )
                })?;
                script_hashes.insert(script_hash);
            }
        }

        let mut canonical = script_hashes
            .iter()
            .map(|script_hash| format!("{script_hash:x}"))
            .collect::<Vec<_>>();
        canonical.sort_unstable();
        let id = hex::encode(Sha256::digest(canonical.join("\n").as_bytes()));
        Ok(Self { script_hashes, id })
    }

    fn contains(&self, script_hash: &BtcScriptHash) -> bool {
        self.script_hashes.contains(script_hash)
    }
}

pub struct SnapshotStore {
    conn: Connection,
    pub summary: SnapshotSummary,
    network: Network,
    registry_table: &'static str,
}

#[derive(Debug, Deserialize)]
struct AuditManifest {
    manifest_version: String,
    file_name: String,
    file_sha256: String,
    state_ref: AuditStateRef,
}

#[derive(Debug, Deserialize)]
struct AuditStateRef {
    block_height: u32,
    stable_block_hash: String,
    #[serde(default)]
    snapshot_id: Option<String>,
    consensus_identity: AuditConsensusIdentity,
}

#[derive(Debug, Deserialize)]
struct AuditConsensusIdentity {
    network: String,
}

#[derive(Debug)]
struct RawCandidate {
    script_hash: Vec<u8>,
    height: Option<i64>,
    balance: i64,
    script_pubkey: Option<Vec<u8>>,
}

impl SnapshotStore {
    #[cfg(test)]
    pub fn open(
        snapshot_file: &Path,
        manifest_file: Option<&Path>,
        verify_file_hash: bool,
    ) -> Result<Self, String> {
        Self::open_with_registry(snapshot_file, manifest_file, None, None, verify_file_hash)
    }

    /// Opens a legacy snapshot or an identity-checked core/registry pair for bounded sampling.
    pub fn open_with_registry(
        snapshot_file: &Path,
        manifest_file: Option<&Path>,
        registry_file: Option<&Path>,
        registry_manifest: Option<&Path>,
        verify_file_hash: bool,
    ) -> Result<Self, String> {
        let snapshot_file = snapshot_file.canonicalize().map_err(|error| {
            format!(
                "Failed to canonicalize snapshot {}: {error}",
                snapshot_file.display()
            )
        })?;
        let manifest_file = manifest_file
            .map(PathBuf::from)
            .unwrap_or_else(|| snapshot_file.with_extension("manifest.json"));
        let manifest_data = std::fs::read_to_string(&manifest_file).map_err(|error| {
            format!(
                "Failed to read snapshot manifest {}: {error}",
                manifest_file.display()
            )
        })?;
        let format: serde_json::Value = serde_json::from_str(&manifest_data)
            .map_err(|error| format!("Invalid audit manifest: {error}"))?;
        if format["manifest_version"] == CORE_SNAPSHOT_MANIFEST_VERSION {
            let registry =
                registry_file.ok_or("Split snapshot sampling requires --script-registry-db")?;
            let input = SplitSnapshotAudit::open(
                &snapshot_file,
                Some(&manifest_file),
                Some(registry),
                registry_manifest,
                verify_file_hash,
            )?;
            return Self::open_split(input);
        }
        if registry_file.is_some() || registry_manifest.is_some() {
            return Err("Registry sidecar options require a split core snapshot".to_string());
        }
        if format["manifest_version"] != "balance-history-snapshot-manifest:v1" {
            return Err("Unsupported audit snapshot manifest version".to_string());
        }
        let manifest: AuditManifest = serde_json::from_str(&manifest_data).map_err(|error| {
            format!(
                "Failed to parse snapshot manifest {}: {error}",
                manifest_file.display()
            )
        })?;
        let actual_file_name = snapshot_file
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                format!(
                    "Snapshot path has no UTF-8 file name: {}",
                    snapshot_file.display()
                )
            })?;
        if manifest.file_name != actual_file_name {
            return Err(format!(
                "Snapshot manifest file_name mismatch: expected {}, got {}",
                actual_file_name, manifest.file_name
            ));
        }

        if verify_file_hash {
            let actual_hash = sha256_file(&snapshot_file)?;
            if actual_hash != manifest.file_sha256 {
                return Err(format!(
                    "Snapshot SHA256 mismatch: manifest={}, actual={}",
                    manifest.file_sha256, actual_hash
                ));
            }
        }

        let conn = open_immutable_audit_db(&snapshot_file)?;

        let meta = conn
            .query_row(
                "SELECT block_height, balance_history_count, utxo_count, block_commit_count, script_registry_count, version FROM meta ORDER BY generated_at DESC LIMIT 1",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, i64>(4)?,
                        row.get::<_, i64>(5)?,
                    ))
                },
            )
            .map_err(|error| format!("Failed to load snapshot meta: {error}"))?;
        if meta.5 != 2 {
            return Err(format!(
                "Unsupported legacy audit snapshot schema: {}",
                meta.5
            ));
        }
        let height = nonnegative_u32(meta.0, "meta.block_height")?;
        if height != manifest.state_ref.block_height {
            return Err(format!(
                "Snapshot height mismatch: meta={}, manifest={}",
                height, manifest.state_ref.block_height
            ));
        }
        let network = manifest
            .state_ref
            .consensus_identity
            .network
            .parse::<Network>()
            .map_err(|error| {
                format!(
                    "Unsupported manifest BTC network {}: {error}",
                    manifest.state_ref.consensus_identity.network
                )
            })?;
        let summary = SnapshotSummary {
            file: snapshot_file.display().to_string(),
            manifest_file: manifest_file.display().to_string(),
            manifest_version: manifest.manifest_version,
            declared_file_sha256: manifest.file_sha256,
            file_sha256_verified: verify_file_hash,
            snapshot_id: manifest.state_ref.snapshot_id,
            height,
            block_hash: manifest.state_ref.stable_block_hash,
            db_schema_version: nonnegative_u32(meta.5, "meta.version")?.to_string(),
            core_artifact_id: None,
            script_registry: None,
            balance_history_count: nonnegative_u64(meta.1, "meta.balance_history_count")?,
            utxo_count: nonnegative_u64(meta.2, "meta.utxo_count")?,
            block_commit_count: nonnegative_u64(meta.3, "meta.block_commit_count")?,
            script_registry_count: nonnegative_u64(meta.4, "meta.script_registry_count")?,
            btc_network: network.to_string(),
        };

        Ok(Self {
            conn,
            summary,
            network,
            registry_table: "main.script_registry",
        })
    }

    fn open_split(input: SplitSnapshotAudit) -> Result<Self, String> {
        let registry = input
            .script_registry
            .as_ref()
            .ok_or("Split audit registry missing")?;
        let conn = open_immutable_audit_db(&input.core.file)?;
        conn.execute(
            "ATTACH DATABASE ?1 AS registry",
            [immutable_audit_uri(&registry.file)?],
        )
        .map_err(|error| format!("Failed to attach immutable audit registry: {error}"))?;
        conn.execute_batch("PRAGMA registry.cache_size=-8192;")
            .map_err(|error| format!("Failed to bound audit registry cache: {error}"))?;
        let manifest = &input.core.manifest;
        let network = manifest
            .db_identity
            .btc_network
            .parse::<Network>()
            .map_err(|error| format!("Invalid audit Bitcoin network: {error}"))?;
        let summary = SnapshotSummary {
            file: input.core.file.display().to_string(),
            manifest_file: input.core.manifest_file.display().to_string(),
            manifest_version: manifest.manifest_version.clone(),
            declared_file_sha256: manifest.file_sha256.clone(),
            file_sha256_verified: input.file_hashes_verified,
            snapshot_id: Some(manifest.core_snapshot_id.clone()),
            height: input.core_meta.block_height,
            block_hash: manifest.state_ref.stable_block_hash.clone(),
            db_schema_version: manifest.snapshot_schema_version.clone(),
            core_artifact_id: Some(manifest.core_artifact_id.clone()),
            script_registry: input.script_registry.clone(),
            balance_history_count: input.core_meta.balance_history_count,
            utxo_count: input.core_meta.utxo_count,
            block_commit_count: input.core_meta.block_commit_count,
            script_registry_count: registry.manifest.entry_count,
            btc_network: network.to_string(),
        };
        Ok(Self {
            conn,
            summary,
            network,
            registry_table: "registry.script_registry",
        })
    }

    pub fn network(&self) -> Network {
        self.network
    }

    pub fn sample(
        &self,
        seed: &str,
        sample_count: usize,
        zero_sample_percent: u8,
        blacklist: &Blacklist,
    ) -> Result<SamplePlan, String> {
        if sample_count == 0 {
            return Err("sample_count must be greater than zero".to_string());
        }
        if zero_sample_percent > 100 {
            return Err("zero_sample_percent must be at most 100".to_string());
        }
        let zero_count = sample_count * usize::from(zero_sample_percent) / 100;
        let positive_count = sample_count - zero_count;
        let mut samples = Vec::with_capacity(sample_count);
        let mut selected = HashSet::with_capacity(sample_count);
        let mut stats = SamplePlanStats::default();

        self.sample_stratum(
            seed,
            SampleKind::PositiveBalance,
            positive_count,
            blacklist,
            &mut selected,
            &mut samples,
            &mut stats,
        )?;
        self.sample_stratum(
            seed,
            SampleKind::ZeroBalance,
            zero_count,
            blacklist,
            &mut selected,
            &mut samples,
            &mut stats,
        )?;

        Ok(SamplePlan { samples, stats })
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_stratum(
        &self,
        seed: &str,
        kind: SampleKind,
        count: usize,
        blacklist: &Blacklist,
        selected: &mut HashSet<BtcScriptHash>,
        samples: &mut Vec<AuditSample>,
        stats: &mut SamplePlanStats,
    ) -> Result<(), String> {
        let label = match kind {
            SampleKind::PositiveBalance => "positive",
            SampleKind::ZeroBalance => "zero",
        };
        for ordinal in 0..count {
            let mut accepted = None;
            for attempt in 0..10_000u32 {
                let probe = deterministic_probe(seed, label, ordinal, attempt);
                let candidate = self.probe_candidate(kind, &probe)?;
                let script_hash = BtcScriptHash::from_slice(&candidate.script_hash)
                    .map_err(|error| format!("Invalid snapshot script_hash bytes: {error}"))?;
                if blacklist.contains(&script_hash) {
                    stats.blacklisted_candidates_replaced += 1;
                    continue;
                }
                if !selected.insert(script_hash) {
                    stats.duplicate_candidates_replaced += 1;
                    continue;
                }
                let script_pubkey =
                    ScriptBuf::from_bytes(candidate.script_pubkey.ok_or_else(|| {
                        format!(
                            "Audit registry mapping missing for core balance script {script_hash:x}"
                        )
                    })?);
                let actual_hash = script_pubkey.to_btc_script_hash();
                if actual_hash != script_hash {
                    return Err(format!(
                        "Snapshot script registry mismatch: key={script_hash:x}, script_hash={actual_hash:x}"
                    ));
                }
                accepted = Some(AuditSample {
                    sample_id: format!("{label}:{ordinal:06}"),
                    kind,
                    script_hash,
                    script_type: classify_script(script_pubkey.as_script()).to_string(),
                    script_pubkey,
                    expected_balance: nonnegative_u64(candidate.balance, "balance")?,
                    last_change_height: candidate
                        .height
                        .map(|height| nonnegative_u32(height, "height"))
                        .transpose()?,
                });
                break;
            }
            samples.push(accepted.ok_or_else(|| {
                format!("Failed to select unique non-blacklisted {label} sample {ordinal}")
            })?);
        }
        Ok(())
    }

    fn probe_candidate(&self, kind: SampleKind, probe: &[u8; 32]) -> Result<RawCandidate, String> {
        let (range_sql, wrap_sql) = match kind {
            SampleKind::PositiveBalance => (
                "SELECT b.script_hash, b.height, b.balance, s.script_pubkey FROM balance_history b LEFT JOIN {registry} s ON s.script_hash=b.script_hash WHERE b.script_hash>=?1 AND b.balance>0 ORDER BY b.script_hash ASC LIMIT 1",
                "SELECT b.script_hash, b.height, b.balance, s.script_pubkey FROM balance_history b LEFT JOIN {registry} s ON s.script_hash=b.script_hash WHERE b.balance>0 ORDER BY b.script_hash ASC LIMIT 1",
            ),
            SampleKind::ZeroBalance => (
                "SELECT s.script_hash, NULL, 0, s.script_pubkey FROM {registry} s LEFT JOIN balance_history b ON b.script_hash=s.script_hash WHERE s.script_hash>=?1 AND b.script_hash IS NULL ORDER BY s.script_hash ASC LIMIT 1",
                "SELECT s.script_hash, NULL, 0, s.script_pubkey FROM {registry} s LEFT JOIN balance_history b ON b.script_hash=s.script_hash WHERE b.script_hash IS NULL ORDER BY s.script_hash ASC LIMIT 1",
            ),
        };
        let range_sql = range_sql.replace("{registry}", self.registry_table);
        let wrap_sql = wrap_sql.replace("{registry}", self.registry_table);
        if let Some(candidate) =
            self.query_candidate(&range_sql, rusqlite::params![probe.as_slice()])?
        {
            return Ok(candidate);
        }
        self.query_candidate(&wrap_sql, [])?
            .ok_or_else(|| format!("Snapshot has no {:?} sampling candidates", kind))
    }

    fn query_candidate<P: rusqlite::Params>(
        &self,
        sql: &str,
        params: P,
    ) -> Result<Option<RawCandidate>, String> {
        self.conn
            .query_row(sql, params, |row| {
                Ok(RawCandidate {
                    script_hash: row.get(0)?,
                    height: row.get(1)?,
                    balance: row.get(2)?,
                    script_pubkey: row.get(3)?,
                })
            })
            .optional()
            .map_err(|error| format!("Failed to probe snapshot candidate: {error}"))
    }
}

fn deterministic_probe(seed: &str, label: &str, ordinal: usize, attempt: u32) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(b"balance-history-electrs-audit-sample:v1\0");
    hasher.update(seed.as_bytes());
    hasher.update([0]);
    hasher.update(label.as_bytes());
    hasher.update([0]);
    hasher.update((ordinal as u64).to_be_bytes());
    hasher.update(attempt.to_be_bytes());
    hasher.finalize().into()
}

fn classify_script(script: &Script) -> &'static str {
    if script.is_p2pkh() {
        "p2pkh"
    } else if script.is_p2sh() {
        "p2sh"
    } else if script.is_p2wpkh() {
        "p2wpkh"
    } else if script.is_p2wsh() {
        "p2wsh"
    } else if script.is_p2tr() {
        "p2tr"
    } else if script.is_op_return() {
        "op_return"
    } else {
        "nonstandard"
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let file = File::open(path)
        .map_err(|error| format!("Failed to open {} for hashing: {error}", path.display()))?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = reader
            .read(&mut buffer)
            .map_err(|error| format!("Failed to hash {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

fn nonnegative_u64(value: i64, field: &str) -> Result<u64, String> {
    u64::try_from(value).map_err(|_| format!("{field} must be non-negative, got {value}"))
}

fn nonnegative_u32(value: i64, field: &str) -> Result<u32, String> {
    u32::try_from(value).map_err(|_| format!("{field} is outside u32 range: {value}"))
}

#[cfg(test)]
#[path = "../../../../tests/common/split_snapshot_audit.rs"]
pub(crate) mod split_fixture;

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::Amount;
    use rusqlite::params;
    use std::str::FromStr;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf, PathBuf) {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("snapshot_10.db");
        let manifest_path = temp.path().join("snapshot_10.manifest.json");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE meta (block_height INTEGER, balance_history_count INTEGER, utxo_count INTEGER, block_commit_count INTEGER, script_registry_count INTEGER, generated_at INTEGER, version INTEGER);
             CREATE TABLE balance_history (script_hash BLOB PRIMARY KEY, height INTEGER, balance INTEGER, delta INTEGER);
             CREATE TABLE script_registry (script_hash BLOB PRIMARY KEY, script_pubkey BLOB);",
        )
        .unwrap();

        let mut registry_count = 0;
        for index in 1..=12u8 {
            let script = ScriptBuf::from(vec![0x51, index]);
            let script_hash = script.to_btc_script_hash();
            conn.execute(
                "INSERT INTO script_registry VALUES (?1, ?2)",
                params![script_hash.as_ref() as &[u8], script.as_bytes()],
            )
            .unwrap();
            if index <= 8 {
                conn.execute(
                    "INSERT INTO balance_history VALUES (?1, 9, ?2, ?2)",
                    params![
                        script_hash.as_ref() as &[u8],
                        Amount::from_sat(u64::from(index)).to_sat() as i64
                    ],
                )
                .unwrap();
            }
            registry_count += 1;
        }
        conn.execute(
            "INSERT INTO meta VALUES (10, 8, 0, 10, ?1, 1, 2)",
            [registry_count],
        )
        .unwrap();
        std::fs::write(
            &manifest_path,
            serde_json::json!({
                "manifest_version": "balance-history-snapshot-manifest:v1",
                "file_name": "snapshot_10.db",
                "file_sha256": "declared",
                "state_ref": {
                    "block_height": 10,
                    "stable_block_hash": "11".repeat(32),
                    "snapshot_id": "22".repeat(32),
                    "consensus_identity": { "network": "bitcoin" }
                }
            })
            .to_string(),
        )
        .unwrap();
        (temp, db_path, manifest_path)
    }

    #[test]
    fn sampling_is_deterministic_and_stratified() {
        let (_temp, db_path, manifest_path) = fixture();
        let store = SnapshotStore::open(&db_path, Some(&manifest_path), false).unwrap();
        let blacklist = Blacklist::default();
        let first = store.sample("seed", 8, 25, &blacklist).unwrap();
        let second = store.sample("seed", 8, 25, &blacklist).unwrap();

        let first_ids = first
            .samples
            .iter()
            .map(|sample| (sample.sample_id.clone(), sample.script_hash))
            .collect::<Vec<_>>();
        let second_ids = second
            .samples
            .iter()
            .map(|sample| (sample.sample_id.clone(), sample.script_hash))
            .collect::<Vec<_>>();
        assert_eq!(first_ids, second_ids);
        assert_eq!(
            first
                .samples
                .iter()
                .filter(|sample| sample.kind == SampleKind::PositiveBalance)
                .count(),
            6
        );
        assert_eq!(
            first
                .samples
                .iter()
                .filter(|sample| sample.kind == SampleKind::ZeroBalance)
                .count(),
            2
        );
    }

    #[test]
    fn blacklist_accepts_script_hash_and_address() {
        let temp = TempDir::new().unwrap();
        let script = ScriptBuf::from(vec![0x51]);
        let address =
            bitcoincore_rpc::bitcoin::Address::from_str("1K6KoYC69NnafWJ7YgtrpwJxBLiijWqwa6")
                .unwrap()
                .require_network(Network::Bitcoin)
                .unwrap();
        let file = temp.path().join("blacklist.txt");
        std::fs::write(
            &file,
            format!(
                "{:x}\n{}\n# comment\n",
                script.to_btc_script_hash(),
                address
            ),
        )
        .unwrap();
        let blacklist = Blacklist::load(Some(&file), Network::Bitcoin).unwrap();
        assert!(blacklist.contains(&script.to_btc_script_hash()));
        assert!(blacklist.contains(&address.script_pubkey().to_btc_script_hash()));
    }
    #[test]
    fn split_sampling_is_stratified_and_missing_mapping_fails() {
        let temp = TempDir::new().unwrap();
        let fixture = super::split_fixture::fixture(temp.path());
        assert!(fixture.legacy.is_file());
        let open = || {
            SnapshotStore::open_with_registry(
                &fixture.core,
                None,
                Some(&fixture.registry),
                None,
                false,
            )
        };
        let store = open().unwrap();
        let plan = store
            .sample("split-seed", 8, 25, &Blacklist::default())
            .unwrap();
        assert_eq!(
            plan.samples
                .iter()
                .filter(|sample| sample.kind == SampleKind::PositiveBalance)
                .count(),
            6
        );
        assert_eq!(
            plan.samples
                .iter()
                .filter(|sample| sample.kind == SampleKind::ZeroBalance)
                .count(),
            2
        );
        assert_eq!(store.summary.script_registry_count, 12);
        assert!(store.summary.script_registry.is_some());
        let repeated = store
            .sample("split-seed", 8, 25, &Blacklist::default())
            .unwrap();
        assert_eq!(
            plan.samples
                .iter()
                .map(|sample| sample.script_hash)
                .collect::<Vec<_>>(),
            repeated
                .samples
                .iter()
                .map(|sample| sample.script_hash)
                .collect::<Vec<_>>()
        );
        let missing = plan.samples[0].script_hash;
        drop(store);
        Connection::open(&fixture.registry)
            .unwrap()
            .execute(
                "DELETE FROM script_registry WHERE script_hash=?1",
                [missing.as_ref() as &[u8]],
            )
            .unwrap();
        let error = open()
            .unwrap()
            .sample("split-seed", 8, 25, &Blacklist::default())
            .unwrap_err();
        assert!(error.contains("registry mapping missing"), "{error}");
    }

    #[test]
    fn split_sampling_requires_paired_registry_and_rejects_corrupt_hash() {
        let temp = TempDir::new().unwrap();
        let fixture = super::split_fixture::fixture(temp.path());
        let missing = SnapshotStore::open_with_registry(&fixture.core, None, None, None, false)
            .err()
            .unwrap();
        assert!(missing.contains("requires --script-registry-db"));
        let verified = SnapshotStore::open_with_registry(
            &fixture.core,
            None,
            Some(&fixture.registry),
            None,
            true,
        )
        .unwrap();
        assert!(verified.summary.file_sha256_verified);
        drop(verified);
        Connection::open(&fixture.registry)
            .unwrap()
            .execute_batch("UPDATE script_registry SET script_pubkey=x'51'")
            .unwrap();
        let error = SnapshotStore::open_with_registry(
            &fixture.core,
            None,
            Some(&fixture.registry),
            None,
            true,
        )
        .err()
        .unwrap();
        assert!(error.contains("SHA256 mismatch"));
    }
}
