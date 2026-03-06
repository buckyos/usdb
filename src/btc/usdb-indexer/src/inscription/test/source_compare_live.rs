use crate::btc::OrdClient;
use crate::config::ConfigManager;
use crate::inscription::{
    BitcoindInscriptionSource, CompareInscriptionSource, CompareTarget, InscriptionSource,
    OrdInscriptionSource,
};
use std::ops::RangeInclusive;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Once;
use std::time::Instant;
use usdb_util::{BTCRpcClient, LogConfig};

static TEST_LOGGER_INIT: Once = Once::new();

#[derive(Debug, Clone, Default)]
struct CompareRangeOptions {
    // Optional explicit height range; if not set, it falls back to env vars.
    range: Option<RangeInclusive<u32>>,
    // Optional progress print interval; if not set, it falls back to env var or default.
    progress_every: Option<u32>,
    // Optional config root directory used by ConfigManager::load.
    config_root: Option<PathBuf>,
    // Optional fail-fast switch for compare mismatch handling.
    fail_fast: Option<bool>,
    // Optional compare target; if not set, it falls back to env var or default.
    compare_target: Option<CompareTarget>,
}

fn required_env_u32(name: &str) -> u32 {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("Environment variable {} is required", name))
        .parse::<u32>()
        .unwrap_or_else(|_| panic!("Environment variable {} must be a u32", name))
}

fn optional_env_u32(name: &str, default: u32) -> u32 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse::<u32>().ok())
        .unwrap_or(default)
}

fn optional_env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "1" | "true" | "yes" | "y" | "on" => true,
            "0" | "false" | "no" | "n" | "off" => false,
            _ => default,
        },
        Err(_) => default,
    }
}

fn optional_env_compare_target(name: &str, default: CompareTarget) -> CompareTarget {
    match std::env::var(name) {
        Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
            "raw" | "raw_inscription" | "raw_inscriptions" => CompareTarget::RawInscription,
            "usdb" | "mint" | "mints" | "usdb_mint" | "usdb_mints" => CompareTarget::UsdbMint,
            _ => default,
        },
        Err(_) => default,
    }
}

fn resolve_height_range(options: &CompareRangeOptions) -> RangeInclusive<u32> {
    if let Some(range) = options.range.clone() {
        return range;
    }

    let start_height = required_env_u32("USDB_COMPARE_START_HEIGHT");
    let end_height = required_env_u32("USDB_COMPARE_END_HEIGHT");
    assert!(
        end_height >= start_height,
        "USDB_COMPARE_END_HEIGHT must be >= USDB_COMPARE_START_HEIGHT"
    );
    start_height..=end_height
}

fn load_compare_config(config_root: Option<PathBuf>) -> Arc<ConfigManager> {
    Arc::new(ConfigManager::load(config_root).expect("Failed to load config for compare test"))
}

fn init_test_logging() {
    TEST_LOGGER_INIT.call_once(|| {
        usdb_util::init_log(
            LogConfig::new(usdb_util::USDB_INDEXER_SERVICE_NAME)
                .enable_file(false)
                .enable_console(true),
        );
    });
}

async fn check_btc_service_alive(btc_client: &BTCRpcClient) -> Result<u32, String> {
    let latest_height = btc_client.get_latest_block_height()?;
    println!(
        "BTC service health check: latest block height = {}, module=inscription_source_compare_test",
        latest_height
    );
    Ok(latest_height)
}

async fn check_ord_service_alive(ord_client: &OrdClient) -> Result<u32, String> {
    let latest_height = ord_client.get_latest_block_height().await?;
    println!(
        "ORD service health check: latest block height = {}, module=inscription_source_compare_test",
        latest_height
    );

    // Prefer probing with a real inscription id to validate non-empty /inscriptions payload.
    let mut probe_ids = Vec::new();
    let search_depth = 64u32;
    for offset in 0..search_depth {
        let probe_height = latest_height.saturating_sub(offset);
        let ids = ord_client.get_inscription_by_block(probe_height).await?;
        if !ids.is_empty() {
            probe_ids = ids;
            break;
        }
    }

    if probe_ids.is_empty() {
        // Fallback: empty payload still validates endpoint JSON behavior in minimal mode.
        let _ = ord_client.get_inscriptions(&[]).await?;
    } else {
        let sample = vec![probe_ids[0]];
        let _ = ord_client.get_inscriptions(&sample).await?;
    }
    Ok(latest_height)
}

async fn check_compare_services_ready(
    config: &ConfigManager,
    btc_client: &BTCRpcClient,
) -> Result<(), String> {
    let btc_height = check_btc_service_alive(btc_client).await?;
    let ord_client = OrdClient::new(config.config().ordinals.rpc_url())?;
    let ord_height = check_ord_service_alive(&ord_client).await?;

    info!(
        "Compare service health check passed: module=inscription_source_compare_test, btc_height={}, ord_height={}",
        btc_height, ord_height
    );
    Ok(())
}

async fn run_compare_ord_and_bitcoind_with_options(options: CompareRangeOptions) {
    init_test_logging();

    let height_range = resolve_height_range(&options);
    let start_height = *height_range.start();
    let end_height = *height_range.end();
    let progress_every = options
        .progress_every
        .unwrap_or_else(|| optional_env_u32("USDB_COMPARE_PROGRESS_EVERY", 500))
        .max(1);
    let config_root = options.config_root.or_else(|| {
        std::env::var("USDB_COMPARE_CONFIG_ROOT")
            .ok()
            .map(PathBuf::from)
    });
    let fail_fast = options
        .fail_fast
        .unwrap_or_else(|| optional_env_bool("USDB_COMPARE_FAIL_FAST", true));
    let compare_target = options.compare_target.unwrap_or_else(|| {
        optional_env_compare_target("USDB_COMPARE_TARGET", CompareTarget::RawInscription)
    });

    let config = load_compare_config(config_root);

    let btc_client = Arc::new(
        BTCRpcClient::new(
            config.config().bitcoin.rpc_url(),
            config.config().bitcoin.auth(),
        )
        .expect("Failed to create BTC RPC client for compare test"),
    );
    check_compare_services_ready(&config, &btc_client)
        .await
        .unwrap_or_else(|e| panic!("Compare service health check failed: {}", e));

    let ord_source: Arc<dyn InscriptionSource> = Arc::new(
        OrdInscriptionSource::new(config.clone())
            .expect("Failed to create ord inscription source for compare test"),
    );
    let bitcoind_source: Arc<dyn InscriptionSource> =
        Arc::new(BitcoindInscriptionSource::new(btc_client.clone()));

    let compare_source = CompareInscriptionSource::new_with_target(
        ord_source,
        bitcoind_source,
        fail_fast,
        compare_target,
    );

    let total_blocks = end_height - start_height + 1;
    let begin = Instant::now();
    let mut scanned = 0u32;
    let mut total_items = 0usize;

    for height in height_range {
        let block = Arc::new(
            btc_client
                .get_block(height)
                .unwrap_or_else(|e| panic!("Failed to load block {}: {}", height, e)),
        );

        let count = match compare_target {
            CompareTarget::RawInscription => compare_source
                .load_block_inscriptions(height, Some(block))
                .await
                .unwrap_or_else(|e| panic!("Compare failed at block {}: {}", height, e))
                .len(),
            CompareTarget::UsdbMint => compare_source
                .load_block_mints(height, Some(block))
                .await
                .unwrap_or_else(|e| panic!("Compare failed at block {}: {}", height, e))
                .len(),
        };
        total_items += count;

        scanned += 1;
        if scanned % progress_every == 0 || scanned == total_blocks {
            println!(
                "inscription compare progress: compare_target={}, scanned={}/{}, current_height={}, total_items={}, elapsed_ms={}",
                compare_target.as_str(),
                scanned,
                total_blocks,
                height,
                total_items,
                begin.elapsed().as_millis()
            );
        }
    }

    println!(
        "inscription compare finished: compare_target={}, start_height={}, end_height={}, scanned_blocks={}, total_items={}, elapsed_ms={}",
        compare_target.as_str(),
        start_height,
        end_height,
        scanned,
        total_items,
        begin.elapsed().as_millis()
    );
}

async fn run_compare_ord_and_bitcoind_on_range(height_range: RangeInclusive<u32>) {
    run_compare_ord_and_bitcoind_with_options(CompareRangeOptions {
        range: Some(height_range),
        compare_target: Some(CompareTarget::RawInscription),
        ..CompareRangeOptions::default()
    })
    .await;
}

#[tokio::test]
#[ignore = "Requires running bitcoind and ord service with reachable RPC endpoints"]
async fn test_compare_ord_and_bitcoind_on_height_range() {
    init_test_logging();

    // Default mode: read block range from env vars.
    run_compare_ord_and_bitcoind_with_options(CompareRangeOptions::default()).await;

    // Optional manual mode example:
    // run_compare_ord_and_bitcoind_on_range(900_000..=900_100).await;
}

#[tokio::test]
#[ignore = "Requires running bitcoind RPC endpoint"]
async fn test_btc_service_health_check() {
    init_test_logging();

    let config_root = std::env::var("USDB_COMPARE_CONFIG_ROOT")
        .ok()
        .map(PathBuf::from);
    let config = load_compare_config(config_root);
    let btc_client = BTCRpcClient::new(
        config.config().bitcoin.rpc_url(),
        config.config().bitcoin.auth(),
    )
    .expect("Failed to create BTC RPC client for health check");

    let height = check_btc_service_alive(&btc_client)
        .await
        .unwrap_or_else(|e| panic!("BTC service health check failed: {}", e));
    assert!(
        height > 0,
        "BTC latest block height should be greater than 0"
    );
}

#[tokio::test]
#[ignore = "Requires running ord RPC endpoint"]
async fn test_ord_service_health_check() {
    init_test_logging();

    let config_root = std::env::var("USDB_COMPARE_CONFIG_ROOT")
        .ok()
        .map(PathBuf::from);
    let config = load_compare_config(config_root);
    let ord_client = OrdClient::new(config.config().ordinals.rpc_url())
        .expect("Failed to create ord client for health check");

    let height = check_ord_service_alive(&ord_client)
        .await
        .unwrap_or_else(|e| panic!("ORD service health check failed: {}", e));
    assert!(
        height > 0,
        "ORD latest block height should be greater than 0"
    );
}

#[tokio::test]
#[ignore = "Manual helper for debug on fixed range"]
async fn test_compare_ord_and_bitcoind_on_small_fixed_range() {
    init_test_logging();

    run_compare_ord_and_bitcoind_on_range(800_000..=900_005).await;
}
