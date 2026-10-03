//! A frozen legacy catalog must fail before the indexer opens or binds databases.
use crate::config::{ConfigManager, IndexerConfig};
use crate::index::InscriptionIndexer;
use crate::output::IndexOutput;
use crate::status::StatusManager;
use std::sync::Arc;

#[test]
fn legacy_default_cannot_silently_start_a_current_rule_dataset() {
    let root = std::env::temp_dir().join(format!(
        "usdb-v2-legacy-startup-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut config = IndexerConfig::default();
    config.bitcoin.auth = Some(usdb_util::BTCAuth::None);
    config.bitcoin.rpc_url = Some("http://127.0.0.1:1".into());
    config.usdb.genesis_block_height = 0;
    std::fs::write(
        root.join("config.json"),
        serde_json::to_vec(&config).unwrap(),
    )
    .unwrap();
    let config = Arc::new(ConfigManager::load(Some(root.clone())).unwrap());
    let data = config.data_dir();
    let status =
        Arc::new(StatusManager::new(config.clone(), Arc::new(IndexOutput::new())).unwrap());
    let error = InscriptionIndexer::new(config, status)
        .err()
        .expect("V1 must not start");
    assert!(
        error.contains("requires v2 and a fresh development network"),
        "{error}"
    );
    assert!(
        error.contains("inscription_schema_version=uip-0001-miner-pass-inscription:v1"),
        "{error}"
    );
    assert!(!data.join(crate::constants::MINER_PASS_DB_FILE).exists());
    assert!(!data.join("energy").exists());
    std::fs::remove_dir_all(root).unwrap();
}
