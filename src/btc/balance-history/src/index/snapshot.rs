use crate::config::{BalanceHistoryConfigRef, SnapshotTrustMode};
use crate::db::{
    BalanceHistoryDB, BalanceHistoryDBIdentity, BalanceHistoryDBMode, BalanceHistoryDBRef,
    BalanceHistoryEntry, BlockCommitEntry, CoreSnapshotDb, CoreSnapshotMeta, ScriptRegistryEntry,
    ScriptRegistrySnapshotDb, ScriptRegistrySnapshotMeta, SnapshotCallback, SnapshotHash,
};
use crate::output::IndexOutputRef;
use crate::service::{HistoricalSnapshotStateRef, build_historical_state_ref_at_height};
use crate::snapshot_contract::{
    CoreSnapshotManifest, ScriptRegistryBaseIdentity, ScriptRegistryManifest,
};
use crate::snapshot_provenance::{
    SnapshotInstallOrigin, SnapshotInstallProvenance, SnapshotVerificationState,
};
use base64::Engine as _;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use usdb_util::{BtcScriptHash, UTXOEntry, parse_json_strict};

/// Detached signature scheme name used by signed snapshot manifests.
pub const SNAPSHOT_SIGNATURE_SCHEME_ED25519: &str = "ed25519";
const SNAPSHOT_INSTALL_PROGRESS_SCHEMA_VERSION: &str =
    "balance-history-core-snapshot-install-progress:v1";
const SNAPSHOT_INSTALL_STAGE_COUNT: u8 = 7;
const SNAPSHOT_INSTALL_PROGRESS_WRITE_INTERVAL: Duration = Duration::from_secs(1);
const SNAPSHOT_INSTALL_VERIFICATION_HEARTBEAT_INTERVAL: Duration = Duration::from_secs(10);
const SNAPSHOT_INSTALL_VERIFICATION_CACHE_SIZE_KIB: u32 = 512 * 1024;

pub struct SnapshotIndexer {
    config: BalanceHistoryConfigRef,
    db: BalanceHistoryDBRef,
    output: IndexOutputRef,
}

/// Files and verified metadata produced for one registry-free core artifact.
#[derive(Clone, Debug)]
pub struct CoreSnapshotCreationResult {
    /// Finalized core SQLite file.
    pub db_path: PathBuf,
    /// Core artifact manifest.
    pub manifest_path: PathBuf,
    /// Optional detached manifest signature.
    pub signature_path: Option<PathBuf>,
    /// Counts and checkpoint identity stored inside the core database.
    pub meta: CoreSnapshotMeta,
    /// Manifest data written beside the core database.
    pub manifest: CoreSnapshotManifest,
}

/// Files and verified metadata produced for one standalone registry artifact.
#[derive(Clone, Debug)]
pub struct ScriptRegistryCreationResult {
    /// Finalized registry SQLite file.
    pub db_path: PathBuf,
    /// Registry artifact manifest.
    pub manifest_path: PathBuf,
    /// Optional detached manifest signature.
    pub signature_path: Option<PathBuf>,
    /// Count and paired core identity stored inside the registry database.
    pub meta: ScriptRegistrySnapshotMeta,
    /// Manifest data written beside the registry database.
    pub manifest: ScriptRegistryManifest,
}

impl SnapshotIndexer {
    pub fn new(
        config: BalanceHistoryConfigRef,
        db: BalanceHistoryDBRef,
        output: IndexOutputRef,
    ) -> Self {
        Self { config, db, output }
    }

    pub fn run(&self, target_block_height: u32) -> Result<(), String> {
        let snapshot_dir = self.config.snapshot_dir();
        let core_path =
            snapshot_dir.join(format!("balance_history_core_{}.db", target_block_height));
        let registry_path =
            snapshot_dir.join(format!("script_registry_{}.db", target_block_height));
        let core = self.run_core_to_path(target_block_height, &core_path)?;
        self.run_registry_to_path(&core.manifest, &registry_path)?;
        Ok(())
    }

    /// Exports and signs a registry-free core artifact for one exact BTC checkpoint.
    pub fn run_core_to_path(
        &self,
        target_block_height: u32,
        db_path: &Path,
    ) -> Result<CoreSnapshotCreationResult, String> {
        let started = Instant::now();
        self.validate_export_target(target_block_height)?;
        let db_identity = self.source_db_identity()?;
        let state_ref = self.historical_state_ref(target_block_height)?;
        let generated_at = unix_timestamp();

        self.output.println(&format!(
            "Creating core snapshot database at {} for height {}",
            db_path.display(),
            target_block_height
        ));
        let snapshot_db = CoreSnapshotDb::create(db_path).map_err(|error| {
            let message = format!("Failed to create core snapshot database: {error}");
            self.output.eprintln(&message);
            message
        })?;
        let snapshot_db = Arc::new(Mutex::new(snapshot_db));

        let balance_history_count = {
            let stage_started = Instant::now();
            let estimated = self.db.get_history_balance_count()?;
            self.output.println(&format!(
                "Will generate core balance snapshot with approximately {} source entries at block height {}",
                estimated, target_block_height
            ));
            self.output
                .start_estimated_load_stage("Core snapshot balance history", estimated);
            let generator = CoreSnapshotGenerator::new(snapshot_db.clone(), self.output.clone());
            self.db.generate_balance_history_snapshot_parallel(
                target_block_height,
                Arc::new(Box::new(generator.clone()) as Box<dyn SnapshotCallback>),
            )?;
            let count = generator.balance_history_count.load(Ordering::SeqCst);
            self.output.finish_load_stage(&format!(
                "Core balance snapshot complete: {} rows written",
                count
            ));
            info!(
                "Split snapshot creation stage completed: component=core, stage=balance_history, block_height={}, row_count={}, elapsed_ms={}",
                target_block_height,
                count,
                stage_started.elapsed().as_millis()
            );
            count
        };

        let utxo_count = {
            let stage_started = Instant::now();
            let estimated = self.db.get_utxo_count()?;
            self.output.println(&format!(
                "Will generate core UTXO snapshot with approximately {} entries",
                estimated
            ));
            self.output
                .start_estimated_load_stage("Core snapshot UTXOs", estimated);
            let generator = CoreSnapshotGenerator::new(snapshot_db.clone(), self.output.clone());
            self.db.generate_utxo_snapshot_parallel(Arc::new(
                Box::new(generator.clone()) as Box<dyn SnapshotCallback>
            ))?;
            let count = generator.utxo_count.load(Ordering::SeqCst);
            self.output.finish_load_stage(&format!(
                "Core UTXO snapshot complete: {} rows written",
                count
            ));
            info!(
                "Split snapshot creation stage completed: component=core, stage=utxo, block_height={}, row_count={}, elapsed_ms={}",
                target_block_height,
                count,
                stage_started.elapsed().as_millis()
            );
            count
        };

        let block_commit_count = {
            let stage_started = Instant::now();
            let estimated = self
                .db
                .get_block_commit_count()?
                .min(u64::from(target_block_height) + 1);
            self.output.println(&format!(
                "Will generate core block commitments up to height {} with approximately {} entries",
                target_block_height, estimated
            ));
            self.output
                .start_estimated_load_stage("Core snapshot block commits", estimated);
            let generator = CoreSnapshotGenerator::new(snapshot_db.clone(), self.output.clone());
            self.db.generate_block_commit_snapshot(
                target_block_height,
                Arc::new(Box::new(generator.clone()) as Box<dyn SnapshotCallback>),
            )?;
            let count = generator.block_commit_count.load(Ordering::SeqCst);
            self.output.finish_load_stage(&format!(
                "Core block-commit snapshot complete: {} rows written",
                count
            ));
            info!(
                "Split snapshot creation stage completed: component=core, stage=block_commit, block_height={}, row_count={}, elapsed_ms={}",
                target_block_height,
                count,
                stage_started.elapsed().as_millis()
            );
            count
        };

        let meta = CoreSnapshotMeta {
            block_height: target_block_height,
            balance_history_count,
            utxo_count,
            block_commit_count,
            generated_at,
            db_identity: db_identity.clone(),
            core_snapshot_id: state_ref.snapshot_id.clone(),
        };
        snapshot_db.lock().unwrap().write_meta(&meta)?;
        let snapshot_db = Arc::try_unwrap(snapshot_db)
            .map_err(|_| {
                "Core snapshot DB still has active writers before finalization".to_string()
            })?
            .into_inner()
            .map_err(|_| "Core snapshot DB mutex was poisoned".to_string())?;
        let db_path = snapshot_db.finalize_for_distribution()?;
        let file_sha256 = SnapshotHash::calc_hash(&db_path)?;
        let signing_key = self.load_signing_key()?;
        let manifest = CoreSnapshotManifest::build(
            file_basename(&db_path)?,
            file_sha256,
            state_ref,
            db_identity,
            signing_key.as_ref().map(|key| key.key_id.clone()),
            generated_at,
        )?;
        let manifest_path = manifest_path_for_snapshot_file(&db_path);
        manifest.save(&manifest_path)?;
        let signature_path = signing_key
            .as_ref()
            .map(|key| {
                sign_snapshot_artifact_manifest(key, &manifest_path, &manifest.signature_payload()?)
            })
            .transpose()?;
        info!(
            "Split snapshot artifact completed: component=core, block_height={}, artifact_id={}, file_sha256={}, balance_history_count={}, utxo_count={}, block_commit_count={}, elapsed_ms={}",
            target_block_height,
            manifest.core_artifact_id,
            manifest.file_sha256,
            balance_history_count,
            utxo_count,
            block_commit_count,
            started.elapsed().as_millis()
        );
        Ok(CoreSnapshotCreationResult {
            db_path,
            manifest_path,
            signature_path,
            meta,
            manifest,
        })
    }

    /// Exports and signs a standalone registry paired to an already-built core artifact.
    pub fn run_registry_to_path(
        &self,
        core_manifest: &CoreSnapshotManifest,
        db_path: &Path,
    ) -> Result<ScriptRegistryCreationResult, String> {
        let started = Instant::now();
        core_manifest.validate()?;
        self.validate_export_target(core_manifest.state_ref.block_height)?;
        let current_identity = self.source_db_identity()?;
        if current_identity != core_manifest.db_identity {
            return Err("Source DB identity changed after core snapshot generation".to_string());
        }
        let current_state_ref = self.historical_state_ref(core_manifest.state_ref.block_height)?;
        if current_state_ref != core_manifest.state_ref {
            return Err("Source state-ref changed after core snapshot generation".to_string());
        }
        let generated_at = unix_timestamp();
        self.output.println(&format!(
            "Creating standalone script registry at {} for core snapshot {}",
            db_path.display(),
            core_manifest.core_snapshot_id
        ));
        let registry_db = Arc::new(Mutex::new(ScriptRegistrySnapshotDb::create(db_path)?));
        let estimated = self.db.get_estimated_script_registry_count()?;
        self.output.println(&format!(
            "Will generate standalone script registry with approximately {} entries",
            estimated
        ));
        self.output
            .start_estimated_load_stage("Standalone script registry", estimated);
        let generator = ScriptRegistryGenerator::new(registry_db.clone(), self.output.clone());
        self.db
            .generate_script_registry_snapshot_parallel(Arc::new(
                Box::new(generator.clone()) as Box<dyn SnapshotCallback>
            ))?;
        let entry_count = generator.entry_count.load(Ordering::SeqCst);
        drop(generator);
        self.output.finish_load_stage(&format!(
            "Standalone script registry complete: {} rows written",
            entry_count
        ));

        let base = ScriptRegistryBaseIdentity {
            btc_network: core_manifest.db_identity.btc_network.clone(),
            btc_genesis_hash: core_manifest.db_identity.btc_genesis_hash.clone(),
            base_height: core_manifest.state_ref.block_height,
            base_block_hash: core_manifest.state_ref.stable_block_hash.clone(),
            core_snapshot_id: core_manifest.core_snapshot_id.clone(),
        };
        let meta = ScriptRegistrySnapshotMeta {
            base: base.clone(),
            entry_count,
            generated_at,
        };
        registry_db.lock().unwrap().write_meta(&meta)?;
        let registry_db = Arc::try_unwrap(registry_db)
            .map_err(|_| {
                "Script-registry DB still has active writers before finalization".to_string()
            })?
            .into_inner()
            .map_err(|_| "Script-registry DB mutex was poisoned".to_string())?;
        let db_path = registry_db.finalize_for_distribution()?;
        let file_sha256 = SnapshotHash::calc_hash(&db_path)?;
        let signing_key = self.load_signing_key()?;
        let manifest = ScriptRegistryManifest::build(
            file_basename(&db_path)?,
            file_sha256,
            base,
            entry_count,
            signing_key.as_ref().map(|key| key.key_id.clone()),
            generated_at,
        )?;
        manifest.validate_against_core(core_manifest)?;
        let manifest_path = manifest_path_for_snapshot_file(&db_path);
        manifest.save(&manifest_path)?;
        let signature_path = signing_key
            .as_ref()
            .map(|key| {
                sign_snapshot_artifact_manifest(key, &manifest_path, &manifest.signature_payload()?)
            })
            .transpose()?;
        info!(
            "Split snapshot artifact completed: component=script_registry, block_height={}, artifact_id={}, file_sha256={}, entry_count={}, elapsed_ms={}",
            core_manifest.state_ref.block_height,
            manifest.registry_artifact_id,
            manifest.file_sha256,
            entry_count,
            started.elapsed().as_millis()
        );
        Ok(ScriptRegistryCreationResult {
            db_path,
            manifest_path,
            signature_path,
            meta,
            manifest,
        })
    }

    fn validate_export_target(&self, target_block_height: u32) -> Result<(), String> {
        let last_synced_height = self.db.get_btc_block_height()?;
        if target_block_height != last_synced_height {
            return Err(format!(
                "Split snapshot export requires exact current state: target block height {} must equal last synced BTC block height {}",
                target_block_height, last_synced_height
            ));
        }
        Ok(())
    }

    fn source_db_identity(&self) -> Result<BalanceHistoryDBIdentity, String> {
        self.db
            .get_db_identity()?
            .ok_or_else(|| "Source balance-history DB has no identity".to_string())
    }

    fn historical_state_ref(
        &self,
        target_block_height: u32,
    ) -> Result<HistoricalSnapshotStateRef, String> {
        build_historical_state_ref_at_height(&self.config, self.db.as_ref(), target_block_height)?
            .ok_or_else(|| {
                format!(
                    "Failed to build historical state ref for snapshot height {}",
                    target_block_height
                )
            })
    }

    fn load_signing_key(&self) -> Result<Option<SnapshotSigningKeyFile>, String> {
        self.config
            .snapshot_signing_key_path()
            .map(|path| SnapshotSigningKeyFile::load(&path))
            .transpose()
    }
}

#[derive(Clone)]
struct CoreSnapshotGenerator {
    db: Arc<Mutex<CoreSnapshotDb>>,
    count: Arc<AtomicU64>,
    balance_history_count: Arc<AtomicU64>,
    utxo_count: Arc<AtomicU64>,
    block_commit_count: Arc<AtomicU64>,
    output: IndexOutputRef,
}

impl CoreSnapshotGenerator {
    fn new(db: Arc<Mutex<CoreSnapshotDb>>, output: IndexOutputRef) -> Self {
        Self {
            db,
            count: Arc::new(AtomicU64::new(0)),
            balance_history_count: Arc::new(AtomicU64::new(0)),
            utxo_count: Arc::new(AtomicU64::new(0)),
            block_commit_count: Arc::new(AtomicU64::new(0)),
            output,
        }
    }
}

impl SnapshotCallback for CoreSnapshotGenerator {
    fn on_balance_history_entries(
        &self,
        entries: &[BalanceHistoryEntry],
        entries_processed: u64,
    ) -> Result<(), String> {
        self.db
            .lock()
            .unwrap()
            .put_balance_history_entries(entries)?;
        self.balance_history_count
            .fetch_add(entries.len() as u64, Ordering::SeqCst);
        let count = self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
        self.output.update_load_current_count(count);
        if let Some(last_entry) = entries.last() {
            self.output.set_load_message(&format!(
                "{}: {} sat @ {}",
                last_entry.script_hash, last_entry.balance, last_entry.block_height
            ));
        }
        Ok(())
    }

    fn on_utxo_entries(&self, entries: &[UTXOEntry], entries_processed: u64) -> Result<(), String> {
        self.db.lock().unwrap().put_utxo_entries(entries)?;
        self.utxo_count
            .fetch_add(entries.len() as u64, Ordering::SeqCst);
        let count = self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
        self.output.update_load_current_count(count);
        if let Some(last_entry) = entries.last() {
            self.output
                .set_load_message(&last_entry.outpoint.to_string());
        }
        Ok(())
    }

    fn on_block_commit_entries(
        &self,
        entries: &[BlockCommitEntry],
        entries_processed: u64,
    ) -> Result<(), String> {
        self.db.lock().unwrap().put_block_commit_entries(entries)?;
        self.block_commit_count
            .fetch_add(entries.len() as u64, Ordering::SeqCst);
        let count = self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
        self.output.update_load_current_count(count);
        if let Some(last_entry) = entries.last() {
            self.output.set_load_message(&format!(
                "block_commit@{} {:x}",
                last_entry.block_height, last_entry.btc_block_hash
            ));
        }
        Ok(())
    }

    fn on_script_registry_entries(
        &self,
        _entries: &[ScriptRegistryEntry],
        _entries_processed: u64,
    ) -> Result<(), String> {
        Err("Core snapshot exporter cannot receive script-registry rows".to_string())
    }
}

#[derive(Clone)]
struct ScriptRegistryGenerator {
    db: Arc<Mutex<ScriptRegistrySnapshotDb>>,
    count: Arc<AtomicU64>,
    entry_count: Arc<AtomicU64>,
    output: IndexOutputRef,
}

impl ScriptRegistryGenerator {
    fn new(db: Arc<Mutex<ScriptRegistrySnapshotDb>>, output: IndexOutputRef) -> Self {
        Self {
            db,
            count: Arc::new(AtomicU64::new(0)),
            entry_count: Arc::new(AtomicU64::new(0)),
            output,
        }
    }

    fn unsupported(component: &str) -> Result<(), String> {
        Err(format!(
            "Script-registry exporter cannot receive {component} rows"
        ))
    }
}

impl SnapshotCallback for ScriptRegistryGenerator {
    fn on_balance_history_entries(
        &self,
        _entries: &[BalanceHistoryEntry],
        _entries_processed: u64,
    ) -> Result<(), String> {
        Self::unsupported("balance-history")
    }

    fn on_utxo_entries(
        &self,
        _entries: &[UTXOEntry],
        _entries_processed: u64,
    ) -> Result<(), String> {
        Self::unsupported("UTXO")
    }

    fn on_block_commit_entries(
        &self,
        _entries: &[BlockCommitEntry],
        _entries_processed: u64,
    ) -> Result<(), String> {
        Self::unsupported("block-commit")
    }

    fn on_script_registry_entries(
        &self,
        entries: &[ScriptRegistryEntry],
        entries_processed: u64,
    ) -> Result<(), String> {
        self.db.lock().unwrap().put_entries(entries)?;
        self.entry_count
            .fetch_add(entries.len() as u64, Ordering::SeqCst);
        let count = self.count.fetch_add(entries_processed, Ordering::SeqCst) + entries_processed;
        self.output.update_load_current_count(count);
        if let Some(last_entry) = entries.last() {
            self.output
                .set_load_message(&format!("script_registry: {}", last_entry.script_hash));
        }
        Ok(())
    }
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn file_basename(path: &Path) -> Result<String, String> {
    path.file_name()
        .and_then(|value| value.to_str())
        .map(str::to_string)
        .ok_or_else(|| {
            format!(
                "Failed to derive artifact file name from {}",
                path.display()
            )
        })
}

#[derive(Clone, Debug)]
pub struct CoreSnapshotData {
    /// Path to the registry-free core SQLite file to install.
    pub file: PathBuf,

    /// Required strict v1 core manifest describing the artifact and restored state.
    pub manifest_file: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct SnapshotInstallProgressRecord {
    schema_version: String,
    state: String,
    stage: String,
    stage_index: u8,
    stage_count: u8,
    current: u64,
    total: u64,
    unit: String,
    stage_current: u64,
    stage_total: u64,
    block_height: Option<u32>,
    snapshot_file: String,
    message: String,
    attempt_started_at_unix: u64,
    stage_started_at_unix: u64,
    updated_at_unix: u64,
}

#[derive(Clone)]
struct SnapshotInstallProgressReporter {
    path: PathBuf,
    snapshot_file: String,
    attempt_started_at_unix: u64,
    stage_started_at: Arc<Mutex<(String, u64)>>,
    last_write: Arc<Mutex<Option<Instant>>>,
    disabled: Arc<AtomicBool>,
}

impl SnapshotInstallProgressReporter {
    fn new(path: PathBuf, snapshot_file: &Path) -> Self {
        let attempt_started_at_unix = Self::unix_time_secs();
        Self {
            path,
            snapshot_file: snapshot_file.display().to_string(),
            attempt_started_at_unix,
            stage_started_at: Arc::new(Mutex::new((String::new(), attempt_started_at_unix))),
            last_write: Arc::new(Mutex::new(None)),
            disabled: Arc::new(AtomicBool::new(false)),
        }
    }

    fn unix_time_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
    }

    #[allow(clippy::too_many_arguments)]
    fn report(
        &self,
        state: &str,
        stage: &str,
        stage_index: u8,
        current: u64,
        total: u64,
        unit: &str,
        stage_current: u64,
        stage_total: u64,
        block_height: Option<u32>,
        message: &str,
        force: bool,
    ) {
        if self.disabled.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        let mut last_write = self.last_write.lock().unwrap();
        if !force
            && last_write.as_ref().is_some_and(|last| {
                now.duration_since(*last) < SNAPSHOT_INSTALL_PROGRESS_WRITE_INTERVAL
            })
        {
            return;
        }

        let observed_at_unix = Self::unix_time_secs();
        let (stage_started_at_unix, updated_at_unix) = {
            let mut stage_started_at = self.stage_started_at.lock().unwrap();
            if stage_started_at.0 != stage {
                *stage_started_at = (
                    stage.to_string(),
                    observed_at_unix.max(self.attempt_started_at_unix),
                );
            }
            (stage_started_at.1, observed_at_unix.max(stage_started_at.1))
        };
        let record = SnapshotInstallProgressRecord {
            schema_version: SNAPSHOT_INSTALL_PROGRESS_SCHEMA_VERSION.to_string(),
            state: state.to_string(),
            stage: stage.to_string(),
            stage_index,
            stage_count: SNAPSHOT_INSTALL_STAGE_COUNT,
            current,
            total,
            unit: unit.to_string(),
            stage_current,
            stage_total,
            block_height,
            snapshot_file: self.snapshot_file.clone(),
            message: message.to_string(),
            attempt_started_at_unix: self.attempt_started_at_unix,
            stage_started_at_unix,
            updated_at_unix,
        };
        *last_write = Some(now);
        if let Err(error) = self.write_record(&record) {
            self.disabled.store(true, Ordering::Relaxed);
            warn!(
                "Failed to write snapshot install progress to {}; disabling further progress writes: {}",
                self.path.display(),
                error
            );
        }
    }

    fn write_record(&self, record: &SnapshotInstallProgressRecord) -> Result<(), String> {
        let parent = self.path.parent().ok_or_else(|| {
            format!(
                "Snapshot install progress path has no parent: {}",
                self.path.display()
            )
        })?;
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create snapshot install progress directory {}: {}",
                parent.display(),
                error
            )
        })?;
        let file_name = self
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| {
                format!(
                    "Snapshot install progress path has no file name: {}",
                    self.path.display()
                )
            })?;
        let temporary_path = self.path.with_file_name(format!(".{file_name}.tmp"));
        let contents = serde_json::to_vec_pretty(record)
            .map_err(|error| format!("Failed to encode snapshot install progress: {error}"))?;
        std::fs::write(&temporary_path, contents).map_err(|error| {
            format!(
                "Failed to write temporary snapshot install progress {}: {}",
                temporary_path.display(),
                error
            )
        })?;
        std::fs::rename(&temporary_path, &self.path).map_err(|error| {
            format!(
                "Failed to publish snapshot install progress {}: {}",
                self.path.display(),
                error
            )
        })
    }
}

struct SnapshotInstallProgressHeartbeat {
    stop: mpsc::Sender<()>,
    worker: Option<JoinHandle<()>>,
}

impl SnapshotInstallProgressHeartbeat {
    fn start(
        progress: Option<&SnapshotInstallProgressReporter>,
        source_size: u64,
        message: &str,
    ) -> Option<Self> {
        let progress = progress?.clone();
        let message = message.to_string();
        let (stop, receiver) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("core-snapshot-install-heartbeat".to_string())
            .spawn(move || {
                while receiver
                    .recv_timeout(SNAPSHOT_INSTALL_VERIFICATION_HEARTBEAT_INTERVAL)
                    .is_err_and(|error| error == mpsc::RecvTimeoutError::Timeout)
                {
                    SnapshotInstaller::report_verification_progress(
                        Some(&progress),
                        source_size,
                        &message,
                    );
                }
            });
        match worker {
            Ok(worker) => Some(Self {
                stop,
                worker: Some(worker),
            }),
            Err(error) => {
                warn!(
                    "Failed to start core snapshot verification heartbeat: {}",
                    error
                );
                None
            }
        }
    }
}

impl Drop for SnapshotInstallProgressHeartbeat {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(worker) = self.worker.take()
            && worker.join().is_err()
        {
            warn!("Core snapshot verification heartbeat thread panicked");
        }
    }
}

struct SnapshotInstallStagingGuard {
    root: PathBuf,
}

impl SnapshotInstallStagingGuard {
    fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Drop for SnapshotInstallStagingGuard {
    fn drop(&mut self) {
        if self.root.exists()
            && let Err(error) = std::fs::remove_dir_all(&self.root)
        {
            warn!(
                "Failed to remove snapshot install staging root {}: {}",
                self.root.display(),
                error
            );
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotSigningKeyFile {
    /// Logical signer identifier stored in manifests and matched during install.
    pub key_id: String,
    /// Ed25519 secret key encoded as base64(raw 32-byte seed).
    pub secret_key_base64: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotTrustedKeySet {
    /// Trusted public keys accepted for signed snapshot manifests.
    pub keys: Vec<SnapshotTrustedPublicKey>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SnapshotTrustedPublicKey {
    /// Logical signer identifier referenced by manifests.
    pub key_id: String,
    /// Ed25519 public key encoded as base64(raw 32-byte bytes).
    pub public_key_base64: String,
}

impl SnapshotSigningKeyFile {
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = std::fs::read_to_string(path).map_err(|e| {
            let msg = format!(
                "Failed to read snapshot signing key {}: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;
        parse_json_strict(&data).map_err(|e| {
            let msg = format!(
                "Failed to parse snapshot signing key {} as JSON: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })
    }

    pub fn to_signing_key(&self) -> Result<SigningKey, String> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(self.secret_key_base64.as_bytes())
            .map_err(|e| {
                let msg = format!(
                    "Failed to decode base64 Ed25519 secret key for signer {}: {}",
                    self.key_id, e
                );
                error!("{}", msg);
                msg
            })?;
        let secret_key: [u8; 32] = raw.as_slice().try_into().map_err(|_| {
            let msg = format!(
                "Invalid Ed25519 secret key length for signer {}: expected 32 bytes, got {}",
                self.key_id,
                raw.len()
            );
            error!("{}", msg);
            msg
        })?;
        Ok(SigningKey::from_bytes(&secret_key))
    }
}

impl SnapshotTrustedKeySet {
    pub fn load(path: &Path) -> Result<Self, String> {
        let data = std::fs::read_to_string(path).map_err(|e| {
            let msg = format!(
                "Failed to read trusted snapshot keys {}: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;
        parse_json_strict(&data).map_err(|e| {
            let msg = format!(
                "Failed to parse trusted snapshot keys {} as JSON: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })
    }

    pub fn find_verifying_key(&self, key_id: &str) -> Result<Option<VerifyingKey>, String> {
        let Some(entry) = self.keys.iter().find(|entry| entry.key_id == key_id) else {
            return Ok(None);
        };
        entry.to_verifying_key().map(Some)
    }
}

impl SnapshotTrustedPublicKey {
    pub fn to_verifying_key(&self) -> Result<VerifyingKey, String> {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(self.public_key_base64.as_bytes())
            .map_err(|e| {
                let msg = format!(
                    "Failed to decode base64 Ed25519 public key for trusted signer {}: {}",
                    self.key_id, e
                );
                error!("{}", msg);
                msg
            })?;
        let public_key: [u8; 32] = raw.as_slice().try_into().map_err(|_| {
            let msg = format!(
                "Invalid Ed25519 public key length for trusted signer {}: expected 32 bytes, got {}",
                self.key_id,
                raw.len()
            );
            error!("{}", msg);
            msg
        })?;
        VerifyingKey::from_bytes(&public_key).map_err(|e| {
            let msg = format!(
                "Failed to construct Ed25519 verifying key for trusted signer {}: {}",
                self.key_id, e
            );
            error!("{}", msg);
            msg
        })
    }
}

/// Returns the default sidecar manifest path for one snapshot DB file.
pub fn manifest_path_for_snapshot_file(file: &Path) -> PathBuf {
    let mut path = file.to_path_buf();
    path.set_extension("manifest.json");
    path
}

/// Returns the detached signature sidecar path for one manifest file.
pub fn signature_path_for_manifest_file(file: &Path) -> PathBuf {
    let mut path = file.to_path_buf();
    path.set_extension("sig");
    path
}

fn save_signature_file(path: &Path, signature: &Signature) -> Result<(), String> {
    let encoded = base64::engine::general_purpose::STANDARD.encode(signature.to_bytes());
    std::fs::write(path, encoded).map_err(|e| {
        let msg = format!(
            "Failed to write snapshot signature sidecar {}: {}",
            path.display(),
            e
        );
        error!("{}", msg);
        msg
    })
}

fn load_signature_file(path: &Path) -> Result<Signature, String> {
    let data = std::fs::read_to_string(path).map_err(|e| {
        let msg = format!(
            "Failed to read snapshot signature sidecar {}: {}",
            path.display(),
            e
        );
        error!("{}", msg);
        msg
    })?;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(data.trim().as_bytes())
        .map_err(|e| {
            let msg = format!(
                "Failed to decode base64 snapshot signature {}: {}",
                path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;
    let signature_bytes: [u8; 64] = raw.as_slice().try_into().map_err(|_| {
        let msg = format!(
            "Invalid Ed25519 signature length in {}: expected 64 bytes, got {}",
            path.display(),
            raw.len()
        );
        error!("{}", msg);
        msg
    })?;
    Ok(Signature::from_bytes(&signature_bytes))
}

fn sign_snapshot_artifact_manifest(
    signing_key: &SnapshotSigningKeyFile,
    manifest_path: &Path,
    payload: &[u8],
) -> Result<PathBuf, String> {
    let signature_path = signature_path_for_manifest_file(manifest_path);
    let signature = signing_key.to_signing_key()?.sign(payload);
    save_signature_file(&signature_path, &signature)?;
    Ok(signature_path)
}

/// Verifies one domain-separated split-artifact manifest payload against a trusted-key catalog.
pub fn verify_snapshot_artifact_manifest_signature(
    signature_scheme: Option<&str>,
    signing_key_id: Option<&str>,
    manifest_path: &Path,
    payload: &[u8],
    trusted_keys_path: &Path,
) -> Result<String, String> {
    let signature_scheme = signature_scheme.ok_or_else(|| {
        format!(
            "Signed snapshot manifest {} is missing signature_scheme",
            manifest_path.display()
        )
    })?;
    if signature_scheme != SNAPSHOT_SIGNATURE_SCHEME_ED25519 {
        return Err(format!(
            "Unsupported snapshot signature scheme {} for {} (expected {})",
            signature_scheme,
            manifest_path.display(),
            SNAPSHOT_SIGNATURE_SCHEME_ED25519
        ));
    }
    let signing_key_id = signing_key_id.ok_or_else(|| {
        format!(
            "Signed snapshot manifest {} is missing signing_key_id",
            manifest_path.display()
        )
    })?;
    let signature_path = signature_path_for_manifest_file(manifest_path);
    if !signature_path.is_file() {
        return Err(format!(
            "Signed snapshot manifest requires signature sidecar {}, but it does not exist",
            signature_path.display()
        ));
    }
    let trusted_keys = SnapshotTrustedKeySet::load(trusted_keys_path)?;
    let verifying_key = trusted_keys
        .find_verifying_key(signing_key_id)?
        .ok_or_else(|| {
            format!(
                "Snapshot signer {} is not trusted by {}",
                signing_key_id,
                trusted_keys_path.display()
            )
        })?;
    let signature = load_signature_file(&signature_path)?;
    verifying_key.verify(payload, &signature).map_err(|error| {
        format!(
            "Snapshot signature verification failed for manifest {} signed by {}: {}",
            manifest_path.display(),
            signing_key_id,
            error
        )
    })?;
    Ok(signing_key_id.to_string())
}

pub struct SnapshotInstaller {
    config: BalanceHistoryConfigRef,
    db: BalanceHistoryDBRef,
    output: IndexOutputRef,
    progress_file: Option<PathBuf>,
}

impl SnapshotInstaller {
    /// Creates a snapshot installer without an external progress observer.
    pub fn new(
        config: BalanceHistoryConfigRef,
        db: BalanceHistoryDBRef,
        output: IndexOutputRef,
    ) -> Self {
        Self {
            config,
            db,
            output,
            progress_file: None,
        }
    }

    /// Enables best-effort atomic progress records for operational observers.
    ///
    /// Progress records never participate in snapshot validation or installation decisions.
    /// A write failure is logged and installation continues with its existing safety checks.
    pub fn with_progress_file(mut self, progress_file: PathBuf) -> Self {
        self.progress_file = Some(progress_file);
        self
    }

    pub fn install(self, data: CoreSnapshotData) -> Result<(), String> {
        let install_begin = Instant::now();
        info!("Starting core snapshot installation from {:?}", data);
        let progress = self
            .progress_file
            .as_ref()
            .map(|path| SnapshotInstallProgressReporter::new(path.clone(), &data.file));
        let source_size = data
            .file
            .metadata()
            .map(|metadata| metadata.len())
            .unwrap_or(0);
        if let Some(progress) = progress.as_ref() {
            progress.report(
                "running",
                "verify_source",
                1,
                0,
                source_size,
                "bytes",
                0,
                source_size,
                None,
                "verifying core snapshot source",
                true,
            );
        }

        self.output.start_load(0);
        let trust_mode = self.config.snapshot.trust_mode.clone();
        let verification_begin = Instant::now();
        let manifest = CoreSnapshotManifest::load(&data.manifest_file).map_err(|error| {
            let message = format!(
                "Failed to load required core snapshot manifest {}: {error}",
                data.manifest_file.display()
            );
            error!("{}", message);
            message
        })?;
        let signature_verified = if matches!(trust_mode, SnapshotTrustMode::Signed) {
            let trusted_keys_path = self.config.snapshot_trusted_keys_path().ok_or_else(|| {
                let msg = format!(
                    "Signed core snapshot install requires snapshot.trusted_keys_file in config for {}",
                    data.file.display()
                );
                error!("{}", msg);
                msg
            })?;
            verify_snapshot_artifact_manifest_signature(
                manifest.signature_scheme.as_deref(),
                manifest.signing_key_id.as_deref(),
                &data.manifest_file,
                &manifest.signature_payload()?,
                &trusted_keys_path,
            )?;
            true
        } else {
            false
        };

        if !data.file.is_file() {
            let msg = format!("Core snapshot file {:?} does not exist", data.file);
            error!("{}", msg);
            return Err(msg);
        }

        let file_name = file_basename(&data.file)?;
        if manifest.file_name != file_name {
            let msg = format!(
                "Core snapshot manifest file_name mismatch: manifest expects {}, actual file is {}",
                manifest.file_name, file_name
            );
            error!("{}", msg);
            return Err(msg);
        }

        self.output.println("Verifying core snapshot file hash...");
        let hash_begin = Instant::now();
        let file_hash = SnapshotHash::calc_hash_with_progress(&data.file, |processed, total| {
            if let Some(progress) = progress.as_ref() {
                progress.report(
                    "running",
                    "verify_source",
                    1,
                    processed,
                    total,
                    "bytes",
                    processed,
                    total,
                    None,
                    "verifying core snapshot file hash",
                    processed >= total,
                );
            }
        })?;
        if file_hash != manifest.file_sha256 {
            let msg = format!(
                "Core snapshot file hash mismatch: expected {}, got {}",
                manifest.file_sha256, file_hash
            );
            error!("{}", msg);
            return Err(msg);
        }
        info!(
            "Core snapshot installation stage completed: stage=file_hash, path={}, elapsed_ms={}",
            data.file.display(),
            hash_begin.elapsed().as_millis()
        );

        let snapshot_db = CoreSnapshotDb::open_for_verification(
            &data.file,
            SNAPSHOT_INSTALL_VERIFICATION_CACHE_SIZE_KIB,
        )
        .map_err(|e| {
            let msg = format!("Failed to open core snapshot database: {}", e);
            error!("{}", msg);
            msg
        })?;
        Self::report_verification_progress(
            progress.as_ref(),
            source_size,
            "checking core snapshot SQLite integrity",
        );
        let integrity_heartbeat = SnapshotInstallProgressHeartbeat::start(
            progress.as_ref(),
            source_size,
            "checking core snapshot SQLite integrity",
        );
        let integrity_begin = Instant::now();
        snapshot_db.verify_integrity().map_err(|error| {
            let msg = format!("Core snapshot SQLite integrity check failed: {error}");
            error!("{}", msg);
            msg
        })?;
        drop(integrity_heartbeat);
        info!(
            "Core snapshot installation verification completed: check=sqlite_integrity, elapsed_ms={}",
            integrity_begin.elapsed().as_millis()
        );
        Self::report_verification_progress(
            progress.as_ref(),
            source_size,
            "checking core snapshot schema and metadata",
        );
        snapshot_db.verify_schema().map_err(|error| {
            let msg = format!("Core snapshot schema verification failed: {error}");
            error!("{}", msg);
            msg
        })?;
        let meta = snapshot_db.read_meta().map_err(|e| {
            let msg = format!("Failed to read core snapshot metadata: {}", e);
            error!("{}", msg);
            msg
        })?;

        let expected_identity = BalanceHistoryDBIdentity::for_network(self.config.btc.network());
        if meta.db_identity != expected_identity {
            let msg = format!(
                "Core snapshot DB identity mismatch: expected {:?}, found {:?}",
                expected_identity, meta.db_identity
            );
            error!("{}", msg);
            return Err(msg);
        }
        if manifest.db_identity != meta.db_identity {
            let msg = format!(
                "Core snapshot manifest DB identity mismatch: manifest {:?}, snapshot {:?}",
                manifest.db_identity, meta.db_identity
            );
            error!("{}", msg);
            return Err(msg);
        }
        if manifest.state_ref.block_height != meta.block_height
            || manifest.core_snapshot_id != meta.core_snapshot_id
            || manifest.generated_at != meta.generated_at
        {
            let msg = format!(
                "Core snapshot metadata identity mismatch: manifest height={} core_snapshot_id={} generated_at={}, snapshot height={} core_snapshot_id={} generated_at={}",
                manifest.state_ref.block_height,
                manifest.core_snapshot_id,
                manifest.generated_at,
                meta.block_height,
                meta.core_snapshot_id,
                meta.generated_at
            );
            error!("{}", msg);
            return Err(msg);
        }
        let expected_consensus_identity = crate::service::build_consensus_snapshot_identity(
            &self.config,
            meta.block_height,
            &manifest.state_ref.stable_block_hash,
        )?;
        if manifest.state_ref.consensus_identity != expected_consensus_identity {
            let msg = format!(
                "Core snapshot consensus identity does not match local configuration: manifest {:?}, expected {:?}",
                manifest.state_ref.consensus_identity, expected_consensus_identity
            );
            error!("{}", msg);
            return Err(msg);
        }

        Self::report_verification_progress(
            progress.as_ref(),
            source_size,
            "counting core snapshot balance rows",
        );
        let count_begin = Instant::now();
        let balance_count_heartbeat = SnapshotInstallProgressHeartbeat::start(
            progress.as_ref(),
            source_size,
            "counting core snapshot balance rows",
        );
        let balance_history_count = snapshot_db.balance_history_count()?;
        drop(balance_count_heartbeat);
        Self::report_verification_progress(
            progress.as_ref(),
            source_size,
            "counting core snapshot UTXO rows",
        );
        let utxo_count_heartbeat = SnapshotInstallProgressHeartbeat::start(
            progress.as_ref(),
            source_size,
            "counting core snapshot UTXO rows",
        );
        let utxo_count = snapshot_db.utxo_count()?;
        drop(utxo_count_heartbeat);
        Self::report_verification_progress(
            progress.as_ref(),
            source_size,
            "counting core snapshot block commitments",
        );
        let block_commit_count_heartbeat = SnapshotInstallProgressHeartbeat::start(
            progress.as_ref(),
            source_size,
            "counting core snapshot block commitments",
        );
        let block_commit_count = snapshot_db.block_commit_count()?;
        drop(block_commit_count_heartbeat);
        let actual_counts = (balance_history_count, utxo_count, block_commit_count);
        let metadata_counts = (
            meta.balance_history_count,
            meta.utxo_count,
            meta.block_commit_count,
        );
        if actual_counts != metadata_counts {
            let msg = format!(
                "Core snapshot metadata count mismatch: metadata={metadata_counts:?}, actual={actual_counts:?}"
            );
            error!("{}", msg);
            return Err(msg);
        }
        info!(
            "Core snapshot installation verification completed: check=table_counts, elapsed_ms={}",
            count_begin.elapsed().as_millis()
        );
        Self::report_verification_progress(
            progress.as_ref(),
            source_size,
            "checking latest core block commitment",
        );
        let latest_commit = snapshot_db.latest_block_commit()?.ok_or_else(|| {
            let msg = format!(
                "Core snapshot at height {} does not contain a block commit",
                meta.block_height
            );
            error!("{}", msg);
            msg
        })?;
        if latest_commit.block_height != meta.block_height
            || format!("{:x}", latest_commit.btc_block_hash) != manifest.state_ref.stable_block_hash
            || crate::service::encode_commit_hex(&latest_commit.block_commit)
                != manifest.state_ref.latest_block_commit
        {
            let msg = "Core snapshot latest block commitment does not match manifest".to_string();
            error!("{}", msg);
            return Err(msg);
        }

        info!("Core snapshot metadata: {:?}", meta);
        self.output.println(&format!(
            "Core snapshot at block height {} contains {} balance rows, {} UTXOs, and {} block commits",
            meta.block_height,
            meta.balance_history_count,
            meta.utxo_count,
            meta.block_commit_count
        ));
        info!(
            "Core snapshot installation stage completed: stage=verify_source, block_height={}, core_snapshot_id={}, core_artifact_id={}, trust_mode={:?}, signature_verified={}, elapsed_ms={}",
            meta.block_height,
            manifest.core_snapshot_id,
            manifest.core_artifact_id,
            trust_mode,
            signature_verified,
            verification_begin.elapsed().as_millis()
        );

        let import_total = meta
            .balance_history_count
            .saturating_add(meta.utxo_count)
            .saturating_add(meta.block_commit_count);
        if let Some(progress) = progress.as_ref() {
            progress.report(
                "running",
                "open_staging_db",
                2,
                0,
                import_total,
                "entries",
                0,
                1,
                Some(meta.block_height),
                "opening staging RocksDB",
                true,
            );
        }

        let staging_open_begin = Instant::now();
        let staging_root = self.prepare_staging_root()?;
        let _staging_guard = SnapshotInstallStagingGuard::new(staging_root.clone());
        let staging_config = self.make_staging_config(staging_root.clone());
        let staging_db = BalanceHistoryDB::open(staging_config, BalanceHistoryDBMode::BestEffort)
            .map_err(|e| {
            let msg = format!("Failed to initialize staging database: {}", e);
            self.output.println(&msg);
            msg
        })?;
        info!(
            "Core snapshot installation stage completed: stage=open_staging_db, block_height={}, elapsed_ms={}",
            meta.block_height,
            staging_open_begin.elapsed().as_millis()
        );

        // Install into staging DB first, then atomically switch the live DB directory.
        let balance_history_begin = Instant::now();
        self.install_balance_history_snapshot(
            &staging_db,
            &snapshot_db,
            &meta,
            progress.as_ref(),
            import_total,
        )?;
        info!(
            "Core snapshot installation stage completed: stage=balance_history, block_height={}, row_count={}, elapsed_ms={}",
            meta.block_height,
            meta.balance_history_count,
            balance_history_begin.elapsed().as_millis()
        );
        let utxo_begin = Instant::now();
        self.install_utxo_snapshot(
            &staging_db,
            &snapshot_db,
            &meta,
            progress.as_ref(),
            import_total,
        )?;
        info!(
            "Core snapshot installation stage completed: stage=utxo, block_height={}, row_count={}, elapsed_ms={}",
            meta.block_height,
            meta.utxo_count,
            utxo_begin.elapsed().as_millis()
        );
        let block_commit_begin = Instant::now();
        self.install_block_commit_snapshot(
            &staging_db,
            &snapshot_db,
            &meta,
            progress.as_ref(),
            import_total,
        )?;
        info!(
            "Core snapshot installation stage completed: stage=block_commit, block_height={}, row_count={}, elapsed_ms={}",
            meta.block_height,
            meta.block_commit_count,
            block_commit_begin.elapsed().as_millis()
        );
        let finalize_begin = Instant::now();
        if let Some(progress) = progress.as_ref() {
            progress.report(
                "running",
                "finalize",
                6,
                import_total,
                import_total,
                "entries",
                0,
                1,
                Some(meta.block_height),
                "validating and flushing staging RocksDB",
                true,
            );
        }
        staging_db
            .put_btc_block_height(meta.block_height)
            .map_err(|e| {
                let msg = format!("Failed to update BTC block height: {}", e);
                self.output.println(&msg);
                msg
            })?;
        staging_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush staging database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.validate_staged_manifest(&staging_db, &meta, &manifest)?;
        let balance_query_floor = manifest.balance_query_floor;
        let history_query_floor = manifest.history_query_floor;
        staging_db
            .put_query_retention_floors(balance_query_floor, history_query_floor)
            .map_err(|e| {
                let msg = format!(
                    "Failed to persist snapshot query retention floors at block height {}: {}",
                    meta.block_height, e
                );
                error!("{}", msg);
                msg
            })?;
        let provenance = SnapshotInstallProvenance {
            origin: SnapshotInstallOrigin::SnapshotInstall,
            trust_mode,
            verification_state: if signature_verified {
                SnapshotVerificationState::SignatureVerified
            } else {
                SnapshotVerificationState::ManifestVerified
            },
            signature_verified,
            artifact_type: manifest.artifact_type,
            manifest_version: manifest.manifest_version.clone(),
            snapshot_schema_version: manifest.snapshot_schema_version.clone(),
            signature_scheme: manifest.signature_scheme.clone(),
            signing_key_id: manifest.signing_key_id.clone(),
            snapshot_file_sha256: manifest.file_sha256.clone(),
            core_snapshot_id: manifest.core_snapshot_id.clone(),
            core_artifact_id: manifest.core_artifact_id.clone(),
            installed_block_height: meta.block_height,
            balance_query_floor,
            history_query_floor,
        };
        staging_db
            .put_snapshot_install_provenance(&provenance)
            .map_err(|e| {
                let msg = format!(
                    "Failed to persist snapshot install provenance at block height {}: {}",
                    meta.block_height, e
                );
                error!("{}", msg);
                msg
            })?;
        staging_db.flush_all().map_err(|e| {
            let msg = format!(
                "Failed to flush snapshot install provenance metadata at block height {}: {}",
                meta.block_height, e
            );
            error!("{}", msg);
            msg
        })?;
        let finalize_elapsed_ms = finalize_begin.elapsed().as_millis();

        let swap_begin = Instant::now();
        let output = self.output.clone();
        if let Some(progress) = progress.as_ref() {
            progress.report(
                "running",
                "swap",
                7,
                import_total,
                import_total,
                "entries",
                0,
                1,
                Some(meta.block_height),
                "promoting staging RocksDB to live DB",
                true,
            );
        }
        self.swap_staging_db_into_place(staging_db, staging_root)?;
        let swap_elapsed_ms = swap_begin.elapsed().as_millis();

        if let Some(progress) = progress.as_ref() {
            progress.report(
                "complete",
                "complete",
                SNAPSHOT_INSTALL_STAGE_COUNT,
                import_total,
                import_total,
                "entries",
                1,
                1,
                Some(meta.block_height),
                "core snapshot imported into live RocksDB",
                true,
            );
        }

        output.println(&format!(
            "Completed core snapshot installation up to block height {}",
            meta.block_height
        ));
        output.finish_load();
        info!(
            "Core snapshot installation completed: block_height={}, core_snapshot_id={}, core_artifact_id={}, finalize_metadata_elapsed_ms={}, swap_elapsed_ms={}, total_elapsed_ms={}",
            meta.block_height,
            manifest.core_snapshot_id,
            manifest.core_artifact_id,
            finalize_elapsed_ms,
            swap_elapsed_ms,
            install_begin.elapsed().as_millis()
        );

        Ok(())
    }

    fn report_verification_progress(
        progress: Option<&SnapshotInstallProgressReporter>,
        source_size: u64,
        message: &str,
    ) {
        if let Some(progress) = progress {
            progress.report(
                "running",
                "verify_source",
                1,
                source_size,
                source_size,
                "bytes",
                0,
                0,
                None,
                message,
                true,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn report_import_progress(
        progress: Option<&SnapshotInstallProgressReporter>,
        meta: &CoreSnapshotMeta,
        stage: &str,
        stage_index: u8,
        completed_before_stage: u64,
        stage_current: u64,
        stage_total: u64,
        import_total: u64,
        message: &str,
        force: bool,
    ) {
        if let Some(progress) = progress {
            progress.report(
                "running",
                stage,
                stage_index,
                completed_before_stage.saturating_add(stage_current),
                import_total,
                "entries",
                stage_current,
                stage_total,
                Some(meta.block_height),
                message,
                force,
            );
        }
    }

    fn validate_staged_manifest(
        &self,
        staging_db: &BalanceHistoryDB,
        meta: &CoreSnapshotMeta,
        manifest: &CoreSnapshotManifest,
    ) -> Result<(), String> {
        self.output.println(&format!(
            "Validating staged snapshot state against manifest for block height {}",
            meta.block_height
        ));

        let staged_identity = staging_db.get_db_identity()?.ok_or_else(|| {
            let msg = "Staged balance-history DB has no identity".to_string();
            error!("{}", msg);
            msg
        })?;
        if staged_identity != manifest.db_identity {
            let msg = format!(
                "Staged balance-history DB identity mismatch: expected {:?}, got {:?}",
                manifest.db_identity, staged_identity
            );
            error!("{}", msg);
            return Err(msg);
        }

        let actual_state_ref =
            build_historical_state_ref_at_height(&self.config, staging_db, meta.block_height)?
                .ok_or_else(|| {
                    let msg = format!(
                        "Failed to reconstruct staged historical state ref at block height {}",
                        meta.block_height
                    );
                    error!("{}", msg);
                    msg
                })?;

        if actual_state_ref != manifest.state_ref {
            let msg = format!(
                "Installed staged snapshot state ref mismatch at height {}: expected {:?}, got {:?}",
                meta.block_height, manifest.state_ref, actual_state_ref
            );
            error!("{}", msg);
            return Err(msg);
        }

        self.output
            .println("Staged snapshot state matches manifest expectations");
        Ok(())
    }

    fn install_balance_history_snapshot(
        &self,
        target_db: &BalanceHistoryDB,
        snapshot_db: &CoreSnapshotDb,
        meta: &CoreSnapshotMeta,
        progress: Option<&SnapshotInstallProgressReporter>,
        import_total: u64,
    ) -> Result<(), String> {
        let total = meta.balance_history_count;
        Self::report_import_progress(
            progress,
            meta,
            "balance_history",
            3,
            0,
            0,
            total,
            import_total,
            "importing balance history entries",
            true,
        );
        if total == 0 {
            self.output
                .println("No balance history entries in snapshot, skipping installation");
            return Ok(());
        }

        self.output.update_load_total_count(total);
        self.output.println(&format!(
            "Installing balance history snapshot with {} entries up to block height {}",
            total, meta.block_height
        ));

        // Load balance by batch
        let page_size = 1024 * 256; // 256k entries per batch
        let mut last_script_hash: Option<BtcScriptHash> = None;
        let mut installed_total = 0u64;
        loop {
            let entries = snapshot_db
                .get_balance_history_entries(page_size, last_script_hash.as_ref())
                .map_err(|e| {
                    let msg = format!("Failed to read snapshot entries: {}", e);
                    self.output.println(&msg);
                    msg
                })?;

            target_db.put_address_history_async(&entries).map_err(|e| {
                let msg = format!("Failed to write snapshot entries to database: {}", e);
                self.output.println(&msg);
                msg
            })?;
            installed_total += entries.len() as u64;

            if let Some(last_entry) = entries.last() {
                last_script_hash = Some(last_entry.script_hash);
            }

            self.output.update_load_current_count(installed_total);
            Self::report_import_progress(
                progress,
                meta,
                "balance_history",
                3,
                0,
                installed_total,
                total,
                import_total,
                "importing balance history entries",
                installed_total >= total,
            );

            if entries.len() < page_size as usize {
                break;
            }
        }

        if installed_total != total {
            return Err(format!(
                "Installed balance rows {} do not match core metadata count {}",
                installed_total, total
            ));
        }

        target_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output
            .println("Balance history snapshot installation completed");

        Ok(())
    }

    fn install_utxo_snapshot(
        &self,
        target_db: &BalanceHistoryDB,
        snapshot_db: &CoreSnapshotDb,
        meta: &CoreSnapshotMeta,
        progress: Option<&SnapshotInstallProgressReporter>,
        import_total: u64,
    ) -> Result<(), String> {
        let total = meta.utxo_count;
        let completed_before_stage = meta.balance_history_count;
        Self::report_import_progress(
            progress,
            meta,
            "utxo",
            4,
            completed_before_stage,
            0,
            total,
            import_total,
            "importing UTXO entries",
            true,
        );
        if total == 0 {
            self.output
                .println("No UTXO entries in snapshot, skipping installation");
            return Ok(());
        }

        self.output.update_load_total_count(total);
        self.output.println(&format!(
            "Installing UTXO snapshot with {} entries up to block height {}",
            total, meta.block_height
        ));

        // Load UTXO by batch
        let page_size = 1024 * 256; // 256k entries per batch
        let mut last_outpoint = None;
        let mut installed_total = 0u64;
        loop {
            let utxos = snapshot_db
                .get_utxo_entries(page_size, last_outpoint.as_ref())
                .map_err(|e| {
                    let msg = format!("Failed to read snapshot UTXOs: {}", e);
                    self.output.println(&msg);
                    msg
                })?;

            target_db.put_utxos(&utxos).map_err(|e| {
                let msg = format!("Failed to write snapshot UTXOs to database: {}", e);
                self.output.println(&msg);
                msg
            })?;
            installed_total += utxos.len() as u64;

            if let Some(last_utxo) = utxos.last() {
                last_outpoint = Some(last_utxo.outpoint);
            }

            self.output.update_load_current_count(installed_total);
            Self::report_import_progress(
                progress,
                meta,
                "utxo",
                4,
                completed_before_stage,
                installed_total,
                total,
                import_total,
                "importing UTXO entries",
                installed_total >= total,
            );

            if utxos.len() < page_size as usize {
                break;
            }
        }

        if installed_total != total {
            return Err(format!(
                "Installed UTXOs {} do not match core metadata count {}",
                installed_total, total
            ));
        }

        target_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output.println("UTXO snapshot installation completed");

        Ok(())
    }

    fn install_block_commit_snapshot(
        &self,
        target_db: &BalanceHistoryDB,
        snapshot_db: &CoreSnapshotDb,
        meta: &CoreSnapshotMeta,
        progress: Option<&SnapshotInstallProgressReporter>,
        import_total: u64,
    ) -> Result<(), String> {
        let total = meta.block_commit_count;
        let completed_before_stage = meta.balance_history_count.saturating_add(meta.utxo_count);
        Self::report_import_progress(
            progress,
            meta,
            "block_commit",
            5,
            completed_before_stage,
            0,
            total,
            import_total,
            "importing block commit entries",
            true,
        );
        if total == 0 {
            self.output
                .println("No block commit entries in snapshot, skipping installation");
            return Ok(());
        }

        self.output.update_load_total_count(total);
        self.output.println(&format!(
            "Installing block commit snapshot with {} entries up to block height {}",
            total, meta.block_height
        ));

        let page_size = 1024 * 256;
        let mut last_block_height = None;
        let mut installed_total = 0u64;
        loop {
            let entries = snapshot_db
                .get_block_commit_entries(page_size, last_block_height)
                .map_err(|e| {
                    let msg = format!("Failed to read snapshot block commits: {}", e);
                    self.output.println(&msg);
                    msg
                })?;

            target_db.put_block_commits_async(&entries).map_err(|e| {
                let msg = format!("Failed to write snapshot block commits to database: {}", e);
                self.output.println(&msg);
                msg
            })?;
            installed_total += entries.len() as u64;

            if let Some(last_entry) = entries.last() {
                last_block_height = Some(last_entry.block_height);
            }

            self.output.update_load_current_count(installed_total);
            Self::report_import_progress(
                progress,
                meta,
                "block_commit",
                5,
                completed_before_stage,
                installed_total,
                total,
                import_total,
                "importing block commit entries",
                installed_total >= total,
            );

            if entries.len() < page_size as usize {
                break;
            }
        }

        if installed_total != total {
            return Err(format!(
                "Installed block commits {} do not match core metadata count {}",
                installed_total, total
            ));
        }

        target_db.flush_all().map_err(|e| {
            let msg = format!("Failed to flush database: {}", e);
            self.output.println(&msg);
            msg
        })?;

        self.output
            .println("Block commit snapshot installation completed");

        Ok(())
    }

    fn prepare_staging_root(&self) -> Result<PathBuf, String> {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let staging_root = self
            .config
            .root_dir
            .join(format!("snapshot_install_staging_{}", nanos));
        if staging_root.exists() {
            std::fs::remove_dir_all(&staging_root).map_err(|e| {
                let msg = format!(
                    "Failed to clear existing staging root {}: {}",
                    staging_root.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        std::fs::create_dir_all(&staging_root).map_err(|e| {
            let msg = format!(
                "Failed to create staging root {}: {}",
                staging_root.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        Ok(staging_root)
    }

    fn make_staging_config(&self, staging_root: PathBuf) -> BalanceHistoryConfigRef {
        let mut cfg = self.config.as_ref().clone();
        cfg.root_dir = staging_root;
        Arc::new(cfg)
    }

    fn swap_staging_db_into_place(
        self,
        staging_db: BalanceHistoryDB,
        staging_root: PathBuf,
    ) -> Result<(), String> {
        let live_db_dir = self.config.db_dir();
        let staged_db_dir = staging_root.join("db");
        let backup_db_dir = self.config.root_dir.join(format!(
            "db_backup_snapshot_install_{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));

        if !staged_db_dir.exists() {
            let msg = format!(
                "Staged DB directory does not exist: {}",
                staged_db_dir.display()
            );
            error!("{}", msg);
            return Err(msg);
        }

        staging_db.close();

        let live_db = Arc::try_unwrap(self.db).map_err(|_| {
            let msg =
                "Failed to acquire exclusive ownership of live DB before snapshot swap".to_string();
            error!("{}", msg);
            msg
        })?;
        live_db.close();

        if live_db_dir.exists() {
            std::fs::rename(&live_db_dir, &backup_db_dir).map_err(|e| {
                let msg = format!(
                    "Failed to move live DB directory {} to backup {}: {}",
                    live_db_dir.display(),
                    backup_db_dir.display(),
                    e
                );
                error!("{}", msg);
                msg
            })?;
        }

        if let Err(e) = std::fs::rename(&staged_db_dir, &live_db_dir) {
            if backup_db_dir.exists() {
                let _ = std::fs::rename(&backup_db_dir, &live_db_dir);
            }
            let msg = format!(
                "Failed to promote staged DB {} to live {}: {}",
                staged_db_dir.display(),
                live_db_dir.display(),
                e
            );
            error!("{}", msg);
            return Err(msg);
        }

        if backup_db_dir.exists() {
            info!(
                "Preserved previous live DB backup after snapshot install: {}",
                backup_db_dir.display()
            );
            self.output.println(&format!(
                "Previous live DB preserved at {}",
                backup_db_dir.display()
            ));
        }

        if staging_root.exists()
            && let Err(error) = std::fs::remove_dir_all(&staging_root)
        {
            warn!(
                "Core snapshot is live, but staging root cleanup failed for {}: {}",
                staging_root.display(),
                error
            );
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::BalanceHistoryConfig;
    use crate::db::{BalanceHistoryDBMode, BlockCommitEntry};
    use crate::output::IndexOutput;
    use crate::service::{
        BalanceHistoryRpc, BalanceHistoryRpcServer, COMMIT_HASH_ALGO, COMMIT_PROTOCOL_VERSION,
        build_consensus_snapshot_identity, encode_commit_hex,
    };
    use crate::status::SyncStatusManager;
    use bitcoincore_rpc::bitcoin::hashes::Hash;
    use bitcoincore_rpc::bitcoin::{BlockHash, OutPoint, ScriptBuf, Txid};
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::sync::watch;
    use usdb_util::{
        CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION, ToBtcScriptHash,
        build_consensus_snapshot_id,
    };

    fn temp_root(tag: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("balance_history_snapshot_{}_{}", tag, nanos));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn test_config_with_root(root_dir: &Path) -> BalanceHistoryConfig {
        BalanceHistoryConfig {
            root_dir: root_dir.to_path_buf(),
            ..BalanceHistoryConfig::default()
        }
    }

    struct TestCoreArtifact {
        db_path: PathBuf,
        manifest_path: PathBuf,
        manifest: CoreSnapshotManifest,
        script_hash: BtcScriptHash,
        outpoint: OutPoint,
        commit: BlockCommitEntry,
    }

    fn write_test_core_artifact(
        config: &BalanceHistoryConfig,
        root_dir: &Path,
        stable_lag_override: Option<u32>,
        data_model_override: Option<&str>,
        signing: Option<(&str, &SigningKey)>,
    ) -> TestCoreArtifact {
        let db_path = root_dir.join("balance_history_core_10.db");
        let script = ScriptBuf::from(vec![9u8; 32]);
        let script_hash = script.to_btc_script_hash();
        let outpoint = OutPoint {
            txid: Txid::from_slice(&[4u8; 32]).unwrap(),
            vout: 1,
        };
        let commit = BlockCommitEntry {
            block_height: 10,
            btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
            balance_delta_root: [11u8; 32],
            block_commit: [12u8; 32],
        };
        let stable_block_hash = format!("{:x}", commit.btc_block_hash);
        let mut consensus_identity =
            build_consensus_snapshot_identity(config, 10, &stable_block_hash).unwrap();
        if let Some(stable_lag) = stable_lag_override {
            consensus_identity.stable_lag = stable_lag;
        }
        let snapshot_id = build_consensus_snapshot_id(&consensus_identity);
        let state_ref = HistoricalSnapshotStateRef {
            block_height: 10,
            stable_block_hash,
            latest_block_commit: encode_commit_hex(&commit.block_commit),
            consensus_identity,
            snapshot_id: snapshot_id.clone(),
            snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
            snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
            commit_protocol_version: COMMIT_PROTOCOL_VERSION.to_string(),
            commit_hash_algo: COMMIT_HASH_ALGO.to_string(),
        };
        let mut db_identity = BalanceHistoryDBIdentity::for_network(config.btc.network());
        if let Some(data_model_version) = data_model_override {
            db_identity.data_model_version = data_model_version.to_string();
        }
        let generated_at = 1_725_000_000;
        let mut db = CoreSnapshotDb::create(&db_path).unwrap();
        db.put_balance_history_entries(&[BalanceHistoryEntry {
            script_hash,
            block_height: 10,
            delta: 75,
            balance: 75,
        }])
        .unwrap();
        db.put_utxo_entries(&[UTXOEntry {
            outpoint,
            script_hash,
            value: 75,
        }])
        .unwrap();
        db.put_block_commit_entries(std::slice::from_ref(&commit))
            .unwrap();
        db.write_meta(&CoreSnapshotMeta {
            block_height: 10,
            balance_history_count: 1,
            utxo_count: 1,
            block_commit_count: 1,
            generated_at,
            db_identity: db_identity.clone(),
            core_snapshot_id: snapshot_id,
        })
        .unwrap();
        db.finalize_for_distribution().unwrap();

        let signing_key_id = signing.map(|(key_id, _)| key_id.to_string());
        let manifest = CoreSnapshotManifest::build(
            file_basename(&db_path).unwrap(),
            SnapshotHash::calc_hash(&db_path).unwrap(),
            state_ref,
            db_identity,
            signing_key_id,
            generated_at,
        )
        .unwrap();
        let manifest_path = manifest_path_for_snapshot_file(&db_path);
        manifest.save(&manifest_path).unwrap();
        if let Some((_, signing_key)) = signing {
            let signature = signing_key.sign(&manifest.signature_payload().unwrap());
            save_signature_file(
                &signature_path_for_manifest_file(&manifest_path),
                &signature,
            )
            .unwrap();
        }

        TestCoreArtifact {
            db_path,
            manifest_path,
            manifest,
            script_hash,
            outpoint,
            commit,
        }
    }

    fn write_signing_material(
        root_dir: &Path,
        key_id: &str,
        seed_byte: u8,
    ) -> (PathBuf, SigningKey) {
        let signing_key = SigningKey::from_bytes(&[seed_byte; 32]);
        let trusted_keys_path = root_dir.join("trusted_snapshot_keys.json");
        let trusted_keys_file = SnapshotTrustedKeySet {
            keys: vec![SnapshotTrustedPublicKey {
                key_id: key_id.to_string(),
                public_key_base64: base64::engine::general_purpose::STANDARD
                    .encode(signing_key.verifying_key().to_bytes()),
            }],
        };
        std::fs::write(
            &trusted_keys_path,
            serde_json::to_vec_pretty(&trusted_keys_file).unwrap(),
        )
        .unwrap();
        (trusted_keys_path, signing_key)
    }

    fn open_test_live_db(config: &Arc<BalanceHistoryConfig>, height: u32) -> BalanceHistoryDBRef {
        let db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        db.put_btc_block_height(height).unwrap();
        Arc::new(db)
    }

    #[test]
    fn snapshot_install_progress_tracks_attempt_and_stage_start_times() {
        let root_dir = temp_root("install_progress_timing");
        let progress_path = root_dir.join("snapshot-loader.progress.json");
        let snapshot_path = root_dir.join("core.db");
        let reporter = SnapshotInstallProgressReporter::new(progress_path.clone(), &snapshot_path);

        reporter.report(
            "running",
            "verify_source",
            1,
            10,
            100,
            "bytes",
            10,
            100,
            None,
            "verifying core snapshot file hash",
            true,
        );
        let first: SnapshotInstallProgressRecord =
            serde_json::from_slice(&std::fs::read(&progress_path).unwrap()).unwrap();
        assert_eq!(
            first.schema_version,
            SNAPSHOT_INSTALL_PROGRESS_SCHEMA_VERSION
        );
        assert_eq!(first.stage_count, 7);
        assert!(first.stage_started_at_unix >= first.attempt_started_at_unix);

        *reporter.stage_started_at.lock().unwrap() = ("verify_source".to_string(), 1);
        reporter.report(
            "running",
            "open_staging_db",
            2,
            0,
            3,
            "entries",
            0,
            1,
            Some(10),
            "opening staging RocksDB",
            true,
        );
        let next_stage: SnapshotInstallProgressRecord =
            serde_json::from_slice(&std::fs::read(&progress_path).unwrap()).unwrap();
        assert_eq!(next_stage.stage, "open_staging_db");
        assert_eq!(next_stage.stage_count, 7);
        assert!(next_stage.stage_started_at_unix > 1);
        assert_eq!(
            next_stage.attempt_started_at_unix,
            first.attempt_started_at_unix
        );
        std::fs::remove_dir_all(root_dir).unwrap();
    }

    #[test]
    fn core_install_replaces_live_db_without_importing_registry() {
        let root_dir = temp_root("core_install_replace");
        let config = Arc::new(test_config_with_root(&root_dir));
        let old_script = ScriptBuf::from(vec![1u8; 32]);
        let old_script_hash = old_script.to_btc_script_hash();
        let old_outpoint = OutPoint {
            txid: Txid::from_slice(&[2u8; 32]).unwrap(),
            vout: 0,
        };
        let live_db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        live_db
            .put_address_history_async(&vec![BalanceHistoryEntry {
                script_hash: old_script_hash,
                block_height: 3,
                delta: 50,
                balance: 50,
            }])
            .unwrap();
        live_db
            .put_utxo(&old_outpoint, &old_script_hash, 50)
            .unwrap();
        live_db
            .put_script_registry_entries(&[ScriptRegistryEntry {
                script_hash: old_script_hash,
                script_pubkey: old_script,
            }])
            .unwrap();
        live_db.put_btc_block_height(3).unwrap();
        let live_db = Arc::new(live_db);
        let artifact = write_test_core_artifact(config.as_ref(), &root_dir, None, None, None);
        let progress_path = root_dir.join("bootstrap/core-loader.progress.json");
        let output = Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new())));
        SnapshotInstaller::new(config.clone(), live_db, output)
            .with_progress_file(progress_path.clone())
            .install(CoreSnapshotData {
                file: artifact.db_path,
                manifest_file: artifact.manifest_path,
            })
            .unwrap();

        let progress: SnapshotInstallProgressRecord =
            serde_json::from_slice(&std::fs::read(&progress_path).unwrap()).unwrap();
        assert_eq!(progress.state, "complete");
        assert_eq!(progress.stage, "complete");
        assert_eq!(progress.stage_count, 7);
        assert_eq!((progress.current, progress.total), (3, 3));

        let reopened_db =
            BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        assert_eq!(reopened_db.get_btc_block_height().unwrap(), 10);
        assert!(
            reopened_db
                .get_balance_delta_at_block_height(&old_script_hash, 3)
                .unwrap()
                .is_none()
        );
        assert!(reopened_db.get_utxo(&old_outpoint).unwrap().is_none());
        assert!(
            reopened_db
                .get_script_registry_entry(&old_script_hash)
                .unwrap()
                .is_none()
        );
        assert!(
            reopened_db
                .get_script_registry_entry(&artifact.script_hash)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            reopened_db
                .get_balance_delta_at_block_height(&artifact.script_hash, 10)
                .unwrap()
                .unwrap()
                .balance,
            75
        );
        assert_eq!(
            reopened_db
                .get_utxo(&artifact.outpoint)
                .unwrap()
                .unwrap()
                .value,
            75
        );
        assert_eq!(
            reopened_db.get_block_commit(10).unwrap(),
            Some(artifact.commit)
        );
        assert_eq!(reopened_db.get_query_retention_floors().unwrap(), (10, 11));

        let provenance = reopened_db
            .get_snapshot_install_provenance()
            .unwrap()
            .unwrap();
        assert_eq!(
            provenance.verification_state,
            SnapshotVerificationState::ManifestVerified
        );
        assert_eq!(provenance.artifact_type, artifact.manifest.artifact_type);
        assert_eq!(
            provenance.snapshot_schema_version,
            artifact.manifest.snapshot_schema_version
        );
        assert_eq!(
            provenance.core_snapshot_id,
            artifact.manifest.core_snapshot_id
        );
        assert_eq!(
            provenance.core_artifact_id,
            artifact.manifest.core_artifact_id
        );

        let status = Arc::new(SyncStatusManager::new());
        status.set_rpc_alive(true);
        status.update_phase(crate::status::SyncPhase::Indexing, None);
        let (shutdown_tx, _) = watch::channel(());
        let rpc_server = BalanceHistoryRpcServer::new(
            config,
            "127.0.0.1:0".parse().unwrap(),
            status,
            Arc::new(reopened_db),
            shutdown_tx,
        );
        let readiness = rpc_server.get_readiness().unwrap();
        assert!(readiness.query_ready);
        assert!(readiness.consensus_ready);
        assert!(
            !readiness
                .blockers
                .contains(&crate::ReadinessBlocker::SnapshotInstallUnverified)
        );

        assert!(
            !std::fs::read_dir(&root_dir)
                .unwrap()
                .filter_map(Result::ok)
                .any(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("snapshot_install_staging_"))
        );
        assert_eq!(
            std::fs::read_dir(&root_dir)
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("db_backup_snapshot_install_"))
                .count(),
            1
        );
    }

    #[test]
    fn core_install_rejects_local_consensus_identity_mismatch_before_swap() {
        let root_dir = temp_root("core_install_bad_stable_lag");
        let config = Arc::new(test_config_with_root(&root_dir));
        let live_db = open_test_live_db(&config, 3);
        let artifact = write_test_core_artifact(config.as_ref(), &root_dir, Some(5), None, None);
        let output = Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new())));
        let error = SnapshotInstaller::new(config.clone(), live_db, output)
            .install(CoreSnapshotData {
                file: artifact.db_path,
                manifest_file: artifact.manifest_path,
            })
            .unwrap_err();
        assert!(error.contains("consensus identity does not match local configuration"));

        let reopened_db = BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap();
        assert_eq!(reopened_db.get_btc_block_height().unwrap(), 3);
        assert!(
            reopened_db
                .get_snapshot_install_provenance()
                .unwrap()
                .is_none()
        );
        assert!(reopened_db.get_block_commit(10).unwrap().is_none());
    }

    #[test]
    fn core_install_rejects_db_identity_mismatch_before_swap() {
        let root_dir = temp_root("core_install_bad_db_identity");
        let config = Arc::new(test_config_with_root(&root_dir));
        let live_db = open_test_live_db(&config, 3);
        let artifact = write_test_core_artifact(
            config.as_ref(),
            &root_dir,
            None,
            Some("balance-history-data-model:tampered-v0"),
            None,
        );
        let output = Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new())));
        let error = SnapshotInstaller::new(config.clone(), live_db, output)
            .install(CoreSnapshotData {
                file: artifact.db_path,
                manifest_file: artifact.manifest_path,
            })
            .unwrap_err();
        assert!(error.contains("Core snapshot DB identity mismatch"));

        let reopened_db = BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap();
        assert_eq!(reopened_db.get_btc_block_height().unwrap(), 3);
        assert!(
            reopened_db
                .get_snapshot_install_provenance()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn core_install_accepts_trusted_signature_and_records_artifact_identity() {
        let root_dir = temp_root("core_install_signed");
        let mut config = test_config_with_root(&root_dir);
        config.snapshot.trust_mode = SnapshotTrustMode::Signed;
        let (trusted_keys_path, signing_key) =
            write_signing_material(&root_dir, "snapshot-signer-1", 42);
        config.snapshot.trusted_keys_file = Some(trusted_keys_path);
        let config = Arc::new(config);
        let live_db = open_test_live_db(&config, 3);
        let artifact = write_test_core_artifact(
            config.as_ref(),
            &root_dir,
            None,
            None,
            Some(("snapshot-signer-1", &signing_key)),
        );
        let output = Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new())));
        SnapshotInstaller::new(config.clone(), live_db, output)
            .install(CoreSnapshotData {
                file: artifact.db_path,
                manifest_file: artifact.manifest_path,
            })
            .unwrap();

        let reopened_db = BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap();
        let provenance = reopened_db
            .get_snapshot_install_provenance()
            .unwrap()
            .unwrap();
        assert_eq!(
            provenance.verification_state,
            SnapshotVerificationState::SignatureVerified
        );
        assert!(provenance.signature_verified);
        assert_eq!(
            provenance.signing_key_id.as_deref(),
            Some("snapshot-signer-1")
        );
        assert_eq!(
            provenance.snapshot_file_sha256,
            artifact.manifest.file_sha256
        );
        assert_eq!(
            provenance.core_artifact_id,
            artifact.manifest.core_artifact_id
        );
    }

    #[test]
    fn core_install_rejects_signed_artifact_without_signature_before_swap() {
        let root_dir = temp_root("core_install_missing_signature");
        let mut config = test_config_with_root(&root_dir);
        config.snapshot.trust_mode = SnapshotTrustMode::Signed;
        let (trusted_keys_path, signing_key) =
            write_signing_material(&root_dir, "snapshot-signer-1", 7);
        config.snapshot.trusted_keys_file = Some(trusted_keys_path);
        let config = Arc::new(config);
        let live_db = open_test_live_db(&config, 3);
        let artifact = write_test_core_artifact(
            config.as_ref(),
            &root_dir,
            None,
            None,
            Some(("snapshot-signer-1", &signing_key)),
        );
        std::fs::remove_file(signature_path_for_manifest_file(&artifact.manifest_path)).unwrap();
        let output = Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new())));
        let error = SnapshotInstaller::new(config.clone(), live_db, output)
            .install(CoreSnapshotData {
                file: artifact.db_path,
                manifest_file: artifact.manifest_path,
            })
            .unwrap_err();
        assert!(error.contains("requires signature sidecar"));

        let reopened_db = BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap();
        assert_eq!(reopened_db.get_btc_block_height().unwrap(), 3);
        assert!(
            reopened_db
                .get_snapshot_install_provenance()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn core_install_rejects_legacy_manifest_without_touching_live_db() {
        let root_dir = temp_root("core_install_legacy_manifest");
        let config = Arc::new(test_config_with_root(&root_dir));
        let live_db = open_test_live_db(&config, 3);
        let snapshot_path = root_dir.join("legacy.db");
        let manifest_path = root_dir.join("legacy.manifest.json");
        std::fs::write(&snapshot_path, b"legacy").unwrap();
        std::fs::write(
            &manifest_path,
            r#"{"manifest_version":"balance-history-snapshot-manifest:v3","file_name":"legacy.db"}"#,
        )
        .unwrap();
        let output = Arc::new(IndexOutput::new(Arc::new(SyncStatusManager::new())));
        let error = SnapshotInstaller::new(config.clone(), live_db, output)
            .install(CoreSnapshotData {
                file: snapshot_path,
                manifest_file: manifest_path,
            })
            .unwrap_err();
        assert!(error.contains("Failed to load required core snapshot manifest"));

        let reopened_db = BalanceHistoryDB::open(config, BalanceHistoryDBMode::Normal).unwrap();
        assert_eq!(reopened_db.get_btc_block_height().unwrap(), 3);
        assert!(
            reopened_db
                .get_snapshot_install_provenance()
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn test_create_snapshot_rejects_non_current_export_height() {
        let root_dir = temp_root("snapshot_non_current_height_rejected");
        let config = Arc::new(test_config_with_root(&root_dir));

        let live_db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        live_db.put_btc_block_height(10).unwrap();
        live_db
            .put_block_commits_async(&[BlockCommitEntry {
                block_height: 10,
                btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
                balance_delta_root: [11u8; 32],
                block_commit: [12u8; 32],
            }])
            .unwrap();
        let live_db = Arc::new(live_db);

        let status = Arc::new(SyncStatusManager::new());
        let output = Arc::new(IndexOutput::new(status));
        let snapshot_indexer = SnapshotIndexer::new(config.clone(), live_db, output);
        let err = snapshot_indexer.run(9).unwrap_err();
        assert!(err.contains("Split snapshot export requires exact current state"));
    }

    #[test]
    fn test_registry_export_rejects_source_advanced_past_core_height() {
        let root_dir = temp_root("registry_source_advanced_past_core");
        let config = Arc::new(test_config_with_root(&root_dir));
        let live_db =
            Arc::new(BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap());
        live_db.put_btc_block_height(10).unwrap();
        live_db
            .put_block_commits_async(&[BlockCommitEntry {
                block_height: 10,
                btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
                balance_delta_root: [11u8; 32],
                block_commit: [12u8; 32],
            }])
            .unwrap();

        let status = Arc::new(SyncStatusManager::new());
        let output = Arc::new(IndexOutput::new(status));
        let snapshot_indexer = SnapshotIndexer::new(config, live_db.clone(), output);
        let core_path = root_dir.join("core.db");
        let core = snapshot_indexer.run_core_to_path(10, &core_path).unwrap();

        live_db.put_btc_block_height(11).unwrap();
        let error = snapshot_indexer
            .run_registry_to_path(&core.manifest, &root_dir.join("registry.db"))
            .unwrap_err();
        assert!(error.contains("Split snapshot export requires exact current state"));
    }

    #[test]
    fn test_create_snapshot_manifest_hash_matches_finalized_db_file() {
        let root_dir = temp_root("snapshot_manifest_hash_matches_finalized_db");
        let config = Arc::new(test_config_with_root(&root_dir));

        let live_db = BalanceHistoryDB::open(config.clone(), BalanceHistoryDBMode::Normal).unwrap();
        live_db.put_btc_block_height(10).unwrap();
        live_db
            .put_address_history_async(&vec![BalanceHistoryEntry {
                script_hash: ScriptBuf::from(vec![3u8; 32]).to_btc_script_hash(),
                block_height: 10,
                delta: 75,
                balance: 75,
            }])
            .unwrap();
        live_db
            .put_block_commits_async(&[BlockCommitEntry {
                block_height: 10,
                btc_block_hash: BlockHash::from_slice(&[10u8; 32]).unwrap(),
                balance_delta_root: [11u8; 32],
                block_commit: [12u8; 32],
            }])
            .unwrap();
        let live_db = Arc::new(live_db);

        let status = Arc::new(SyncStatusManager::new());
        let output = Arc::new(IndexOutput::new(status));
        let snapshot_indexer = SnapshotIndexer::new(config.clone(), live_db, output);
        snapshot_indexer.run(10).unwrap();

        let snapshot_path = root_dir
            .join("snapshots")
            .join("balance_history_core_10.db");
        let manifest_path = manifest_path_for_snapshot_file(&snapshot_path);
        let manifest = CoreSnapshotManifest::load(&manifest_path).unwrap();
        let actual_hash = SnapshotHash::calc_hash(&snapshot_path).unwrap();
        assert_eq!(manifest.file_sha256, actual_hash);
        let registry_path = root_dir.join("snapshots").join("script_registry_10.db");
        let registry_manifest =
            ScriptRegistryManifest::load(&manifest_path_for_snapshot_file(&registry_path)).unwrap();
        registry_manifest.validate_against_core(&manifest).unwrap();
        assert!(
            !snapshot_path.with_extension("db-wal").exists(),
            "finalized snapshot should not leave sqlite wal sidecar behind"
        );
        assert!(
            !snapshot_path.with_extension("db-shm").exists(),
            "finalized snapshot should not leave sqlite shm sidecar behind"
        );
    }

    #[test]
    fn snapshot_key_files_reject_duplicate_identity_fields() {
        let root_dir = temp_root("key_file_duplicate_identity");
        let signing_key_path = root_dir.join("snapshot-signing-key.json");
        std::fs::write(
            &signing_key_path,
            r#"{"key_id":"first","key_id":"second","secret_key_base64":"AA=="}"#,
        )
        .unwrap();
        let signing_error = SnapshotSigningKeyFile::load(&signing_key_path).unwrap_err();
        assert!(signing_error.contains("duplicate JSON key: key_id"));

        let trusted_keys_path = root_dir.join("snapshot-trusted-keys.json");
        std::fs::write(
            &trusted_keys_path,
            r#"{"keys":[{"key_id":"test","public_key_base64":"AA==","key_id":"other"}]}"#,
        )
        .unwrap();
        let trusted_error = SnapshotTrustedKeySet::load(&trusted_keys_path).unwrap_err();
        assert!(trusted_error.contains("duplicate JSON key: key_id"));
        std::fs::remove_dir_all(root_dir).unwrap();
    }
}
