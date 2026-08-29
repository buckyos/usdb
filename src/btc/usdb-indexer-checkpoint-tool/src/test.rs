use crate::artifact::{
    build_operation_id, inventory_files, load_and_verify_checkpoint, save_json_atomic,
};
use crate::crypto::sign_manifest;
use crate::data::{IndexerDiskLayout, validate_indexer_data};
use crate::install::{InstallPairOptions, install_pair_for_test, publish_indexer_data};
use crate::*;
use balance_history::{
    BalanceHistoryConfig, BalanceHistoryDB, BalanceHistoryDBIdentity, BlockCommitEntry,
    HistoricalSnapshotStateRef, SnapshotConfig, SnapshotDB, SnapshotHash, SnapshotManifest,
    SnapshotMeta, SnapshotSigningKeyFile, SnapshotTrustMode, SnapshotTrustedKeySet,
    SnapshotTrustedPublicKey, build_consensus_snapshot_identity,
};
use base64::Engine as _;
use bitcoincore_rpc::bitcoin::hashes::Hash;
use bitcoincore_rpc::bitcoin::{BlockHash, Network};
use ed25519_dalek::{Signer, SigningKey};
use jsonrpc_core::IoHandler;
use jsonrpc_http_server::ServerBuilder;
use rocksdb::{ColumnFamilyDescriptor, DB, Options};
use rusqlite::Connection;
use rust_rocksdb::{self as rocksdb};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use usdb_util::{
    CONSENSUS_SNAPSHOT_ID_HASH_ALGO, CONSENSUS_SNAPSHOT_ID_VERSION,
    LocalStateActiveBalanceSnapshot, LocalStateCommitIdentity, SystemStateIdentity, VersionFamily,
    build_consensus_snapshot_id, build_local_state_commit, build_system_state_id,
    embedded_btc_activation_registry_catalog,
};

struct Fixture {
    root: PathBuf,
    source_root: PathBuf,
    artifact_dir: PathBuf,
    manifest_path: PathBuf,
    trusted_keys_path: PathBuf,
    balance_history_manifest_path: PathBuf,
    manifest: IndexerCheckpointManifest,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn temp_root(tag: &str) -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "usdb_indexer_checkpoint_{tag}_{}_{}",
        std::process::id(),
        nanos
    ));
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn write_indexer_config(root: &Path, height: u32) {
    std::fs::create_dir_all(root).unwrap();
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec_pretty(&json!({
            "bitcoin": {"network": "regtest"},
            "usdb": {"genesis_block_height": height}
        }))
        .unwrap(),
    )
    .unwrap();
}

fn write_indexer_data(root: &Path, height: u32, stable_hash: &str, block_commit: &str) {
    let data_dir = root.join("data");
    std::fs::create_dir_all(&data_dir).unwrap();
    let conn = Connection::open(data_dir.join("miner_pass.db")).unwrap();
    conn.execute_batch(
        "
        CREATE TABLE state (name TEXT PRIMARY KEY, value INTEGER);
        CREATE TABLE active_balance_snapshots (
            block_height INTEGER PRIMARY KEY,
            total_balance INTEGER NOT NULL,
            active_address_count INTEGER NOT NULL
        );
        CREATE TABLE pass_block_commits (
            block_height INTEGER PRIMARY KEY,
            balance_history_block_height INTEGER NOT NULL,
            balance_history_block_commit TEXT NOT NULL,
            mutation_root TEXT NOT NULL,
            block_commit TEXT NOT NULL,
            commit_protocol_version TEXT NOT NULL,
            commit_hash_algo TEXT NOT NULL
        );
        CREATE TABLE balance_history_snapshot_history (
            block_height INTEGER PRIMARY KEY,
            stable_block_hash TEXT NOT NULL,
            latest_block_commit TEXT NOT NULL,
            stable_lag INTEGER NOT NULL,
            commit_protocol_version TEXT NOT NULL,
            commit_hash_algo TEXT NOT NULL
        );
        ",
    )
    .unwrap();
    conn.execute(
        "INSERT INTO state (name, value) VALUES ('btc_synced_block_height', ?1)",
        [i64::from(height)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO active_balance_snapshots VALUES (?1, 0, 0)",
        [i64::from(height)],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO balance_history_snapshot_history VALUES (?1, ?2, ?3, 10, '1.0.0', 'sha256')",
        rusqlite::params![i64::from(height), stable_hash, block_commit],
    )
    .unwrap();
    drop(conn);

    let energy_dir = data_dir.join("energy");
    let mut options = Options::default();
    options.create_if_missing(true);
    options.create_missing_column_families(true);
    let db = DB::open_cf_descriptors(
        &options,
        &energy_dir,
        [
            ColumnFamilyDescriptor::new("pass_energy", Options::default()),
            ColumnFamilyDescriptor::new("meta", Options::default()),
        ],
    )
    .unwrap();
    let meta = db.cf_handle("meta").unwrap();
    db.put_cf(&meta, b"synced_block_height", height.to_be_bytes())
        .unwrap();
    db.flush().unwrap();
}

fn build_fixture(tag: &str) -> Fixture {
    let root = temp_root(tag);
    let source_root = root.join("source-indexer");
    let height = 1;
    let stable_hash = "11".repeat(32);
    let block_commit = "22".repeat(32);
    write_indexer_config(&source_root, height);
    write_indexer_data(&source_root, height, &stable_hash, &block_commit);

    let source_bh_config = BalanceHistoryConfig {
        root_dir: root.join("source-balance-history"),
        btc: usdb_util::BTCConfig {
            network: Network::Regtest,
            ..Default::default()
        },
        ..Default::default()
    };
    let consensus_identity =
        build_consensus_snapshot_identity(&source_bh_config, height, &stable_hash).unwrap();
    let snapshot_id = build_consensus_snapshot_id(&consensus_identity);
    let upstream = HistoricalSnapshotStateRef {
        block_height: height,
        stable_block_hash: stable_hash.clone(),
        latest_block_commit: block_commit.clone(),
        consensus_identity,
        snapshot_id: snapshot_id.clone(),
        snapshot_id_hash_algo: CONSENSUS_SNAPSHOT_ID_HASH_ALGO.to_string(),
        snapshot_id_version: CONSENSUS_SNAPSHOT_ID_VERSION.to_string(),
        commit_protocol_version: "1.0.0".to_string(),
        commit_hash_algo: "sha256".to_string(),
    };
    let catalog =
        embedded_btc_activation_registry_catalog(bitcoincore_rpc::bitcoin::Network::Regtest)
            .unwrap();
    let registry = catalog.current_registry();
    let active_versions = registry.lookup_active_version_set(height).unwrap();
    let active_version_set_id = active_versions.active_version_set_id();
    let commit_protocol_version = active_versions
        .require_string(VersionFamily::CommitProtocolVersion)
        .unwrap()
        .to_string();
    let local_state_commit = build_local_state_commit(&LocalStateCommitIdentity {
        commit_protocol_version,
        upstream_snapshot_id: snapshot_id.clone(),
        active_version_set_id: active_version_set_id.clone(),
        local_synced_block_height: height,
        latest_pass_block_commit: None,
        latest_active_balance_snapshot: Some(LocalStateActiveBalanceSnapshot {
            block_height: height,
            total_balance: 0,
            active_address_count: 0,
        }),
    });
    let system_state_id = build_system_state_id(&SystemStateIdentity {
        upstream_snapshot_id: snapshot_id.clone(),
        local_state_commit: local_state_commit.clone(),
    });
    let state_identity = IndexerCheckpointStateIdentity {
        block_height: height,
        stable_block_hash: stable_hash,
        latest_block_commit: block_commit,
        snapshot_id,
        activation_registry_id: registry.activation_registry_id(),
        active_version_set_id,
        local_state_commit,
        system_state_id,
    };
    let indexer_state_ref = json!({
        "block_height": height,
        "snapshot_info": {
            "stable_block_hash": state_identity.stable_block_hash,
            "latest_block_commit": state_identity.latest_block_commit,
            "snapshot_id": state_identity.snapshot_id,
        },
        "local_state_commit_info": {
            "activation_registry_id": state_identity.activation_registry_id,
            "active_version_set_id": state_identity.active_version_set_id,
            "local_state_commit": state_identity.local_state_commit,
        },
        "system_state_info": {"system_state_id": state_identity.system_state_id},
    });

    let artifact_parent = root.join("artifacts");
    let temporary_artifact = artifact_parent.join("temporary");
    std::fs::create_dir_all(&artifact_parent).unwrap();
    crate::artifact::copy_directory(&source_root.join("data"), &temporary_artifact.join("data"))
        .unwrap();
    let files = inventory_files(&temporary_artifact.join("data")).unwrap();
    let signing_key = SigningKey::from_bytes(&[7u8; 32]);
    let signing_key_file = SnapshotSigningKeyFile {
        key_id: "test-checkpoint-key".to_string(),
        secret_key_base64: base64::engine::general_purpose::STANDARD.encode(signing_key.to_bytes()),
    };
    let trusted_keys_path = root.join("trusted-keys.json");
    std::fs::write(
        &trusted_keys_path,
        serde_json::to_vec_pretty(&SnapshotTrustedKeySet {
            keys: vec![SnapshotTrustedPublicKey {
                key_id: signing_key_file.key_id.clone(),
                public_key_base64: base64::engine::general_purpose::STANDARD
                    .encode(signing_key.verifying_key().to_bytes()),
            }],
        })
        .unwrap(),
    )
    .unwrap();

    let balance_history_artifact = root.join("balance-history-artifact");
    std::fs::create_dir_all(&balance_history_artifact).unwrap();
    let snapshot_path = balance_history_artifact.join("snapshot.db");
    let snapshot_manifest_path = balance_history_artifact.join("snapshot.manifest.json");
    let snapshot_signature_path = balance_history_artifact.join("snapshot.manifest.sig");
    let commit = BlockCommitEntry {
        block_height: height,
        btc_block_hash: BlockHash::from_slice(&[0x11; 32]).unwrap(),
        balance_delta_root: [0x21; 32],
        block_commit: [0x22; 32],
    };
    {
        let mut snapshot_db = SnapshotDB::open(&snapshot_path).unwrap();
        snapshot_db
            .put_block_commit_entries(std::slice::from_ref(&commit))
            .unwrap();
        let mut meta = SnapshotMeta::new(
            height,
            BalanceHistoryDBIdentity::for_network(Network::Regtest),
        );
        meta.block_commit_count = 1;
        snapshot_db.update_meta(&meta).unwrap();
    }
    let mut snapshot_manifest = SnapshotManifest::build(
        "snapshot.db".to_string(),
        SnapshotHash::calc_hash(&snapshot_path).unwrap(),
        upstream.clone(),
        BalanceHistoryDBIdentity::for_network(Network::Regtest),
        None,
    );
    snapshot_manifest.signature_scheme = Some(CHECKPOINT_SIGNATURE_SCHEME.to_string());
    snapshot_manifest.signing_key_id = Some(signing_key_file.key_id.clone());
    snapshot_manifest.save(&snapshot_manifest_path).unwrap();
    let snapshot_signature = signing_key.sign(&snapshot_manifest.canonical_bytes().unwrap());
    std::fs::write(
        &snapshot_signature_path,
        base64::engine::general_purpose::STANDARD.encode(snapshot_signature.to_bytes()),
    )
    .unwrap();

    let binding = BalanceHistorySnapshotBinding {
        manifest_file_name: "snapshot.manifest.json".to_string(),
        manifest_sha256: crate::artifact::sha256_file(&snapshot_manifest_path).unwrap(),
        snapshot_file_name: "snapshot.db".to_string(),
        snapshot_file_sha256: snapshot_manifest.file_sha256.clone(),
        state_ref: upstream,
        balance_query_floor: height,
        history_query_floor: height + 1,
    };
    let operation_id =
        build_operation_id("test-bundle", 42, &binding, &state_identity, &files).unwrap();
    let artifact_dir_name = format!("usdb-indexer-checkpoint-1-{}", &operation_id[..16]);
    let artifact_dir = artifact_parent.join(&artifact_dir_name);
    std::fs::rename(&temporary_artifact, &artifact_dir).unwrap();
    let manifest = IndexerCheckpointManifest {
        manifest_version: INDEXER_CHECKPOINT_MANIFEST_VERSION.to_string(),
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
        data_schema_version: INDEXER_CHECKPOINT_DATA_SCHEMA_VERSION.to_string(),
        operation_id,
        network_bundle_id: "test-bundle".to_string(),
        chain_id: 42,
        btc_network: "regtest".to_string(),
        index_origin_height: height,
        checkpoint_height: height,
        artifact_dir_name,
        files,
        balance_history: binding,
        indexer_state_ref,
        state_identity,
        signature_scheme: CHECKPOINT_SIGNATURE_SCHEME.to_string(),
        signing_key_id: signing_key_file.key_id.clone(),
        generated_at: 1,
    };
    let manifest_path = artifact_dir.join(CHECKPOINT_MANIFEST_FILE);
    save_json_atomic(&manifest_path, &manifest).unwrap();
    sign_manifest(
        &manifest,
        &signing_key_file,
        &artifact_dir.join(CHECKPOINT_SIGNATURE_FILE),
    )
    .unwrap();
    Fixture {
        root,
        source_root,
        artifact_dir,
        manifest_path,
        trusted_keys_path,
        balance_history_manifest_path: snapshot_manifest_path,
        manifest,
    }
}

fn write_balance_history_config(root: &Path, trusted_keys_path: &Path) {
    std::fs::create_dir_all(root).unwrap();
    let config = BalanceHistoryConfig {
        root_dir: root.to_path_buf(),
        btc: usdb_util::BTCConfig {
            network: Network::Regtest,
            ..Default::default()
        },
        snapshot: SnapshotConfig {
            trust_mode: SnapshotTrustMode::Signed,
            signing_key_file: None,
            trusted_keys_file: Some(trusted_keys_path.to_path_buf()),
        },
        ..Default::default()
    };
    std::fs::write(
        root.join("config.toml"),
        toml::to_string_pretty(&config).unwrap(),
    )
    .unwrap();
}

fn install_options(fixture: &Fixture, tag: &str) -> InstallPairOptions {
    let indexer_root = fixture.root.join(format!("{tag}-indexer"));
    let balance_history_root = fixture.root.join(format!("{tag}-balance-history"));
    write_indexer_config(&indexer_root, fixture.manifest.checkpoint_height);
    write_balance_history_config(&balance_history_root, &fixture.trusted_keys_path);
    InstallPairOptions {
        checkpoint_manifest: fixture.manifest_path.clone(),
        balance_history_manifest: fixture.balance_history_manifest_path.clone(),
        trusted_keys: fixture.trusted_keys_path.clone(),
        indexer_root,
        balance_history_root,
        network_bundle_id: fixture.manifest.network_bundle_id.clone(),
        chain_id: fixture.manifest.chain_id,
        index_origin_height: fixture.manifest.index_origin_height,
        lock_timeout: Duration::from_secs(2),
    }
}

#[test]
fn offline_state_ref_recomputation_matches_manifest() {
    let fixture = build_fixture("offline_state_ref");
    let layout = IndexerDiskLayout::load(&fixture.source_root).unwrap();
    let rebuilt = validate_indexer_data(&layout, &fixture.manifest).unwrap();
    assert_eq!(rebuilt, fixture.manifest.state_identity);
}

#[test]
fn signed_checkpoint_rejects_file_tampering() {
    let fixture = build_fixture("tamper");
    load_and_verify_checkpoint(&fixture.manifest_path, &fixture.trusted_keys_path, true).unwrap();
    std::fs::write(fixture.artifact_dir.join("data/miner_pass.db"), b"tampered").unwrap();
    let error =
        load_and_verify_checkpoint(&fixture.manifest_path, &fixture.trusted_keys_path, true)
            .unwrap_err();
    assert!(error.contains("file inventory"));
}

#[test]
fn pair_verification_requires_exact_upstream_manifest_binding() {
    let fixture = build_fixture("pair_binding");
    load_and_verify_checkpoint_pair(
        &fixture.manifest_path,
        &fixture.balance_history_manifest_path,
        &fixture.trusted_keys_path,
    )
    .unwrap();

    let alternate_manifest = fixture
        .balance_history_manifest_path
        .with_file_name("alternate.manifest.json");
    let alternate_signature = fixture
        .balance_history_manifest_path
        .with_file_name("alternate.manifest.sig");
    std::fs::copy(&fixture.balance_history_manifest_path, &alternate_manifest).unwrap();
    std::fs::copy(
        fixture.balance_history_manifest_path.with_extension("sig"),
        &alternate_signature,
    )
    .unwrap();
    let error = load_and_verify_checkpoint_pair(
        &fixture.manifest_path,
        &alternate_manifest,
        &fixture.trusted_keys_path,
    )
    .unwrap_err();
    assert!(error.contains("does not match signed checkpoint binding"));
}

#[test]
fn indexer_publish_is_idempotent_after_rename_before_journal_update() {
    let fixture = build_fixture("publish_resume");
    let target_root = fixture.root.join("target-indexer");
    write_indexer_config(&target_root, fixture.manifest.checkpoint_height);
    let options = InstallPairOptions {
        checkpoint_manifest: fixture.manifest_path.clone(),
        balance_history_manifest: fixture.root.join("snapshot.manifest.json"),
        trusted_keys: fixture.trusted_keys_path.clone(),
        indexer_root: target_root.clone(),
        balance_history_root: fixture.root.join("balance-history"),
        network_bundle_id: fixture.manifest.network_bundle_id.clone(),
        chain_id: fixture.manifest.chain_id,
        index_origin_height: fixture.manifest.index_origin_height,
        lock_timeout: Duration::from_secs(1),
    };
    publish_indexer_data(&options, &fixture.manifest).unwrap();
    publish_indexer_data(&options, &fixture.manifest).unwrap();
    let target_layout = IndexerDiskLayout::load(&target_root).unwrap();
    assert_eq!(
        inventory_files(&target_layout.data_dir).unwrap(),
        fixture.manifest.files
    );
}

#[test]
fn operation_id_changes_when_file_digest_changes() {
    let fixture = build_fixture("operation_id");
    let mut files = fixture.manifest.files.clone();
    files[0].sha256 = format!("{:x}", Sha256::digest(b"different"));
    let changed = build_operation_id(
        &fixture.manifest.network_bundle_id,
        fixture.manifest.chain_id,
        &fixture.manifest.balance_history,
        &fixture.manifest.state_identity,
        &files,
    )
    .unwrap();
    assert_ne!(changed, fixture.manifest.operation_id);
}

#[tokio::test]
async fn paired_install_resumes_after_each_atomic_publish_boundary() {
    let fixture = build_fixture("paired_resume");
    for (tag, failure_point) in [
        ("indexer-staging", "indexer_staged"),
        ("after-indexer", "indexer_published"),
        ("balance-history-staging", "balance_history_installing"),
        ("after-balance-history", "balance_history_published"),
    ] {
        let options = install_options(&fixture, tag);
        unsafe { std::env::set_var("USDB_CHECKPOINT_FAIL_AFTER", failure_point) };
        let error = install_pair_for_test(options.clone(), tag)
            .await
            .unwrap_err();
        assert!(
            error.contains("Injected paired checkpoint failure"),
            "unexpected install error: {error}"
        );
        unsafe { std::env::remove_var("USDB_CHECKPOINT_FAIL_AFTER") };
        if failure_point == "balance_history_installing" {
            let partial_live = options
                .balance_history_root
                .join("db/balance_history/partial");
            std::fs::create_dir_all(partial_live.parent().unwrap()).unwrap();
            std::fs::write(partial_live, b"partial").unwrap();
            let partial_staging = options
                .balance_history_root
                .join("snapshot_install_staging_interrupted/partial");
            std::fs::create_dir_all(partial_staging.parent().unwrap()).unwrap();
            std::fs::write(partial_staging, b"partial").unwrap();
        }

        let report = install_pair_for_test(options.clone(), tag).await.unwrap();
        assert_eq!(report.stage, PairedInstallStage::Complete);
        assert_eq!(report.operation_id, fixture.manifest.operation_id);
        let indexer_layout = IndexerDiskLayout::load(&options.indexer_root).unwrap();
        validate_indexer_data(&indexer_layout, &fixture.manifest).unwrap();
        let bh_config =
            std::sync::Arc::new(BalanceHistoryConfig::load(&options.balance_history_root).unwrap());
        let bh_db = BalanceHistoryDB::open_read_only(bh_config).unwrap();
        assert_eq!(
            bh_db.get_btc_block_height().unwrap(),
            fixture.manifest.checkpoint_height
        );
        assert!(
            bh_db
                .get_snapshot_install_provenance()
                .unwrap()
                .unwrap()
                .signature_verified
        );
    }
}

fn start_rpc_server(
    readiness: serde_json::Value,
    state_ref: serde_json::Value,
) -> jsonrpc_http_server::Server {
    let mut io = IoHandler::new();
    io.add_sync_method("get_readiness", move |_| Ok(readiness.clone()));
    io.add_sync_method("get_state_ref_at_height", move |_| Ok(state_ref.clone()));
    ServerBuilder::new(io)
        .start_http(&"127.0.0.1:0".parse().unwrap())
        .unwrap()
}

#[tokio::test]
async fn recovery_recomputes_historical_state_refs_after_services_advance() {
    let fixture = build_fixture("recovery_state_ref");
    let indexer_server = start_rpc_server(
        json!({
            "consensus_ready": true,
            "synced_block_height": fixture.manifest.checkpoint_height + 2,
        }),
        fixture.manifest.indexer_state_ref.clone(),
    );
    let balance_history_server = start_rpc_server(
        json!({
            "consensus_ready": true,
            "stable_height": fixture.manifest.checkpoint_height + 2,
        }),
        serde_json::to_value(&fixture.manifest.balance_history.state_ref).unwrap(),
    );
    let indexer_root = fixture.root.join("recovery-indexer");
    let indexer_url = format!("http://{}", indexer_server.address());
    let balance_history_url = format!("http://{}", balance_history_server.address());

    let marker = verify_recovery(
        &fixture.manifest_path,
        &fixture.trusted_keys_path,
        &indexer_root,
        &indexer_url,
        &balance_history_url,
        Duration::from_secs(2),
    )
    .await
    .unwrap();
    assert_eq!(marker.operation_id, fixture.manifest.operation_id);
    assert_eq!(
        marker.system_state_id,
        fixture.manifest.state_identity.system_state_id
    );
    assert!(
        indexer_root
            .join("bootstrap/paired-checkpoint-recovery.done.json")
            .is_file()
    );

    tokio::task::spawn_blocking(move || {
        indexer_server.close();
        balance_history_server.close();
    })
    .await
    .unwrap();
}
