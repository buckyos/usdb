use super::{DiscoveredInscription, InscriptionSource, InscriptionSourceFuture};
use crate::config::ConfigManagerRef;
use bitcoincore_rpc::bitcoin::Block;
use ord::InscriptionId;
use ordinals::SatPoint;
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Debug, Clone)]
struct FixtureInscription {
    inscription_id: InscriptionId,
    inscription_number: i32,
    timestamp: Option<u32>,
    satpoint: Option<SatPoint>,
    content_type: Option<String>,
    content_string: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FixtureFileRaw {
    blocks: HashMap<String, Vec<FixtureInscriptionRaw>>,
}

#[derive(Debug, Deserialize)]
struct FixtureInscriptionRaw {
    inscription_id: String,
    inscription_number: i32,
    timestamp: Option<u32>,
    satpoint: Option<String>,
    content_type: Option<String>,
    content_string: Option<String>,
}

pub struct FixtureInscriptionSource {
    fixture_path: PathBuf,
    blocks: HashMap<u32, Vec<FixtureInscription>>,
}

impl FixtureInscriptionSource {
    pub fn new(config: ConfigManagerRef) -> Result<Self, String> {
        let fixture_file = config
            .config()
            .usdb
            .inscription_fixture_file
            .clone()
            .ok_or_else(|| {
                "inscription_fixture_file is required when inscription_source is fixture"
                    .to_string()
            })?;

        let fixture_path = Self::resolve_fixture_path(config.clone(), &fixture_file);
        let raw_data = std::fs::read_to_string(&fixture_path).map_err(|e| {
            let msg = format!(
                "Failed to read inscription fixture file {}: {}",
                fixture_path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        let raw: FixtureFileRaw = serde_json::from_str(&raw_data).map_err(|e| {
            let msg = format!(
                "Failed to parse inscription fixture JSON {}: {}",
                fixture_path.display(),
                e
            );
            error!("{}", msg);
            msg
        })?;

        let blocks = Self::parse_blocks(raw.blocks)?;
        info!(
            "Fixture inscription source loaded: module=inscription_source_fixture, fixture_path={}, block_count={}",
            fixture_path.display(),
            blocks.len()
        );

        Ok(Self {
            fixture_path,
            blocks,
        })
    }

    fn resolve_fixture_path(config: ConfigManagerRef, fixture_file: &str) -> PathBuf {
        let path = PathBuf::from(fixture_file);
        if path.is_absolute() {
            path
        } else {
            config.root_dir().join(path)
        }
    }

    fn parse_blocks(
        raw_blocks: HashMap<String, Vec<FixtureInscriptionRaw>>,
    ) -> Result<HashMap<u32, Vec<FixtureInscription>>, String> {
        let mut parsed = HashMap::with_capacity(raw_blocks.len());
        for (height_text, raw_items) in raw_blocks {
            let height = height_text
                .parse::<u32>()
                .map_err(|e| format!("Invalid fixture block height key {}: {}", height_text, e))?;

            let mut items = Vec::with_capacity(raw_items.len());
            for raw in raw_items {
                let inscription_id = InscriptionId::from_str(&raw.inscription_id).map_err(|e| {
                    format!(
                        "Invalid fixture inscription_id {} at block {}: {}",
                        raw.inscription_id, height, e
                    )
                })?;
                let satpoint = match raw.satpoint {
                    Some(text) => Some(SatPoint::from_str(&text).map_err(|e| {
                        format!(
                            "Invalid fixture satpoint {} at block {}: {}",
                            text, height, e
                        )
                    })?),
                    None => None,
                };

                items.push(FixtureInscription {
                    inscription_id,
                    inscription_number: raw.inscription_number,
                    timestamp: raw.timestamp,
                    satpoint,
                    content_type: raw.content_type,
                    content_string: raw.content_string,
                });
            }
            parsed.insert(height, items);
        }

        Ok(parsed)
    }
}

impl InscriptionSource for FixtureInscriptionSource {
    fn source_name(&self) -> &'static str {
        "fixture"
    }

    fn load_block_inscriptions<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredInscription>, String>> {
        Box::pin(async move {
            let default_timestamp = block_hint
                .as_ref()
                .map(|block| block.header.time)
                .unwrap_or(0);
            let items = self.blocks.get(&block_height).cloned().unwrap_or_default();

            let discovered = items
                .into_iter()
                .map(|item| DiscoveredInscription {
                    inscription_id: item.inscription_id,
                    inscription_number: item.inscription_number,
                    block_height,
                    timestamp: item.timestamp.unwrap_or(default_timestamp),
                    satpoint: item.satpoint,
                    content_type: item.content_type,
                    content_string: item.content_string,
                })
                .collect::<Vec<_>>();

            debug!(
                "Loaded fixture inscriptions: module=inscription_source_fixture, fixture_path={}, block_height={}, count={}",
                self.fixture_path.display(),
                block_height,
                discovered.len()
            );
            Ok(discovered)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_id(text: &str) -> InscriptionId {
        InscriptionId::from_str(text).expect("valid inscription id")
    }

    #[test]
    fn parse_blocks_accepts_valid_fixture_rows() {
        let mut raw_blocks = HashMap::new();
        raw_blocks.insert(
            "100".to_string(),
            vec![FixtureInscriptionRaw {
                inscription_id: "0000000000000000000000000000000000000000000000000000000000000000i0"
                    .to_string(),
                inscription_number: 42,
                timestamp: Some(1234),
                satpoint: Some(
                    "0000000000000000000000000000000000000000000000000000000000000000:0:0"
                        .to_string(),
                ),
                content_type: Some("text/plain;charset=utf-8".to_string()),
                content_string: Some("{}".to_string()),
            }],
        );

        let parsed = FixtureInscriptionSource::parse_blocks(raw_blocks).expect("parse fixture");
        let items = parsed.get(&100).expect("height=100 exists");
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].inscription_id,
            parse_id("0000000000000000000000000000000000000000000000000000000000000000i0")
        );
        assert_eq!(items[0].inscription_number, 42);
        assert_eq!(items[0].timestamp, Some(1234));
        assert_eq!(items[0].content_string.as_deref(), Some("{}"));
    }

    #[test]
    fn parse_blocks_rejects_invalid_height_key() {
        let mut raw_blocks = HashMap::new();
        raw_blocks.insert("bad-height".to_string(), Vec::new());

        let err = FixtureInscriptionSource::parse_blocks(raw_blocks).expect_err("must fail");
        assert!(err.contains("Invalid fixture block height key"));
    }

    #[test]
    fn parse_blocks_rejects_invalid_inscription_id() {
        let mut raw_blocks = HashMap::new();
        raw_blocks.insert(
            "200".to_string(),
            vec![FixtureInscriptionRaw {
                inscription_id: "invalid-inscription-id".to_string(),
                inscription_number: 1,
                timestamp: None,
                satpoint: None,
                content_type: None,
                content_string: None,
            }],
        );

        let err = FixtureInscriptionSource::parse_blocks(raw_blocks).expect_err("must fail");
        assert!(err.contains("Invalid fixture inscription_id"));
    }

    #[tokio::test]
    async fn load_block_inscriptions_defaults_missing_timestamp_to_zero() {
        let source = FixtureInscriptionSource {
            fixture_path: PathBuf::from("/tmp/mock-fixture.json"),
            blocks: HashMap::from([(
                300u32,
                vec![FixtureInscription {
                    inscription_id: parse_id(
                        "0000000000000000000000000000000000000000000000000000000000000000i1",
                    ),
                    inscription_number: 9,
                    timestamp: None,
                    satpoint: None,
                    content_type: Some("text/plain".to_string()),
                    content_string: Some("{}".to_string()),
                }],
            )]),
        };

        let loaded = source
            .load_block_inscriptions(300, None)
            .await
            .expect("load fixture");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].timestamp, 0);
        assert_eq!(loaded[0].block_height, 300);
    }
}
