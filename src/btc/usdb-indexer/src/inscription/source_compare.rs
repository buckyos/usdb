use super::{DiscoveredMint, InscriptionSource, InscriptionSourceFuture};
use bitcoincore_rpc::bitcoin::Block;
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct CompareInscriptionSource {
    primary: Arc<dyn InscriptionSource>,
    shadow: Arc<dyn InscriptionSource>,
    fail_fast: bool,
}

impl CompareInscriptionSource {
    pub fn new(
        primary: Arc<dyn InscriptionSource>,
        shadow: Arc<dyn InscriptionSource>,
        fail_fast: bool,
    ) -> Self {
        Self {
            primary,
            shadow,
            fail_fast,
        }
    }

    fn compare_block_mints(
        &self,
        block_height: u32,
        primary_mints: &[DiscoveredMint],
        shadow_mints: &[DiscoveredMint],
    ) -> Result<(), String> {
        let mut primary_map = BTreeMap::<String, &DiscoveredMint>::new();
        let mut shadow_map = BTreeMap::<String, &DiscoveredMint>::new();

        for mint in primary_mints {
            primary_map.insert(mint.inscription_id.to_string(), mint);
        }
        for mint in shadow_mints {
            shadow_map.insert(mint.inscription_id.to_string(), mint);
        }

        let mut only_primary = Vec::new();
        let mut only_shadow = Vec::new();
        let mut content_mismatch = Vec::new();

        for (inscription_id, primary_mint) in &primary_map {
            match shadow_map.get(inscription_id) {
                Some(shadow_mint) => {
                    if primary_mint.content_string != shadow_mint.content_string {
                        content_mismatch.push(inscription_id.clone());
                    }
                }
                None => only_primary.push(inscription_id.clone()),
            }
        }

        for inscription_id in shadow_map.keys() {
            if !primary_map.contains_key(inscription_id) {
                only_shadow.push(inscription_id.clone());
            }
        }

        if only_primary.is_empty() && only_shadow.is_empty() && content_mismatch.is_empty() {
            return Ok(());
        }

        warn!(
            "Inscription source mismatch: module=inscription_source_compare, block_height={}, primary_source={}, shadow_source={}, primary_count={}, shadow_count={}, only_primary_count={}, only_shadow_count={}, content_mismatch_count={}",
            block_height,
            self.primary.source_name(),
            self.shadow.source_name(),
            primary_mints.len(),
            shadow_mints.len(),
            only_primary.len(),
            only_shadow.len(),
            content_mismatch.len()
        );

        if !only_primary.is_empty() {
            let sample: Vec<_> = only_primary.iter().take(5).cloned().collect();
            warn!(
                "Inscription source mismatch details: module=inscription_source_compare, block_height={}, only_primary_sample={:?}",
                block_height, sample
            );
        }
        if !only_shadow.is_empty() {
            let sample: Vec<_> = only_shadow.iter().take(5).cloned().collect();
            warn!(
                "Inscription source mismatch details: module=inscription_source_compare, block_height={}, only_shadow_sample={:?}",
                block_height, sample
            );
        }
        if !content_mismatch.is_empty() {
            let sample: Vec<_> = content_mismatch.iter().take(5).cloned().collect();
            warn!(
                "Inscription source mismatch details: module=inscription_source_compare, block_height={}, content_mismatch_sample={:?}",
                block_height, sample
            );
        }

        if self.fail_fast {
            let msg = format!(
                "Inscription source compare failed at block {}: only_primary={}, only_shadow={}, content_mismatch={}",
                block_height,
                only_primary.len(),
                only_shadow.len(),
                content_mismatch.len()
            );
            return Err(msg);
        }

        Ok(())
    }
}

impl InscriptionSource for CompareInscriptionSource {
    fn source_name(&self) -> &'static str {
        "compare"
    }

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>> {
        Box::pin(async move {
            let primary_mints = self
                .primary
                .load_block_mints(block_height, block_hint.clone())
                .await?;
            let shadow_mints = self
                .shadow
                .load_block_mints(block_height, block_hint)
                .await?;

            self.compare_block_mints(block_height, &primary_mints, &shadow_mints)?;

            Ok(primary_mints)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigManager;
    use crate::inscription::{BitcoindInscriptionSource, OrdInscriptionSource};
    use std::ops::RangeInclusive;
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::time::Instant;
    use usdb_util::BTCRpcClient;

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

    async fn run_compare_ord_and_bitcoind_with_options(options: CompareRangeOptions) {
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

        let config = Arc::new(
            ConfigManager::load(config_root).expect("Failed to load config for compare test"),
        );

        let btc_client = Arc::new(
            BTCRpcClient::new(
                config.config().bitcoin.rpc_url(),
                config.config().bitcoin.auth(),
            )
            .expect("Failed to create BTC RPC client for compare test"),
        );

        let ord_source: Arc<dyn InscriptionSource> = Arc::new(
            OrdInscriptionSource::new(config.clone())
                .expect("Failed to create ord inscription source for compare test"),
        );
        let bitcoind_source: Arc<dyn InscriptionSource> =
            Arc::new(BitcoindInscriptionSource::new(btc_client.clone()));

        let compare_source = CompareInscriptionSource::new(ord_source, bitcoind_source, fail_fast);

        let total_blocks = end_height - start_height + 1;
        let begin = Instant::now();
        let mut scanned = 0u32;
        let mut total_mints = 0usize;

        for height in height_range {
            let block = Arc::new(
                btc_client
                    .get_block(height)
                    .unwrap_or_else(|e| panic!("Failed to load block {}: {}", height, e)),
            );

            let mints = compare_source
                .load_block_mints(height, Some(block))
                .await
                .unwrap_or_else(|e| panic!("Compare failed at block {}: {}", height, e));
            total_mints += mints.len();

            scanned += 1;
            if scanned % progress_every == 0 || scanned == total_blocks {
                println!(
                    "inscription compare progress: scanned={}/{}, current_height={}, total_mints={}, elapsed_ms={}",
                    scanned,
                    total_blocks,
                    height,
                    total_mints,
                    begin.elapsed().as_millis()
                );
            }
        }

        println!(
            "inscription compare finished: start_height={}, end_height={}, scanned_blocks={}, total_mints={}, elapsed_ms={}",
            start_height,
            end_height,
            scanned,
            total_mints,
            begin.elapsed().as_millis()
        );
    }

    async fn run_compare_ord_and_bitcoind_on_range(height_range: RangeInclusive<u32>) {
        run_compare_ord_and_bitcoind_with_options(CompareRangeOptions {
            range: Some(height_range),
            ..CompareRangeOptions::default()
        })
        .await;
    }

    #[tokio::test]
    #[ignore = "Requires running bitcoind and ord service with reachable RPC endpoints"]
    async fn test_compare_ord_and_bitcoind_on_height_range() {
        // Default mode: read block range from env vars.
        run_compare_ord_and_bitcoind_with_options(CompareRangeOptions::default()).await;

        // Optional manual mode example:
        // run_compare_ord_and_bitcoind_on_range(900_000..=900_100).await;
    }
}
