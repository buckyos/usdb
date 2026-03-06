use super::{
    DiscoveredInscription, DiscoveredMint, InscriptionSource, InscriptionSourceFuture,
    map_usdb_mints_from_inscriptions,
};
use bitcoincore_rpc::bitcoin::Block;
use std::collections::BTreeMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareTarget {
    UsdbMint,
    RawInscription,
}

impl CompareTarget {
    pub fn as_str(&self) -> &'static str {
        match self {
            CompareTarget::UsdbMint => "usdb_mint",
            CompareTarget::RawInscription => "raw_inscription",
        }
    }
}

pub struct CompareInscriptionSource {
    primary: Arc<dyn InscriptionSource>,
    shadow: Arc<dyn InscriptionSource>,
    fail_fast: bool,
    target: CompareTarget,
}

impl CompareInscriptionSource {
    pub fn new(
        primary: Arc<dyn InscriptionSource>,
        shadow: Arc<dyn InscriptionSource>,
        fail_fast: bool,
    ) -> Self {
        Self::new_with_target(primary, shadow, fail_fast, CompareTarget::UsdbMint)
    }

    pub fn new_with_target(
        primary: Arc<dyn InscriptionSource>,
        shadow: Arc<dyn InscriptionSource>,
        fail_fast: bool,
        target: CompareTarget,
    ) -> Self {
        Self {
            primary,
            shadow,
            fail_fast,
            target,
        }
    }

    fn compare_items<T, FK, FC>(
        &self,
        block_height: u32,
        target_label: &str,
        primary_items: &[T],
        shadow_items: &[T],
        key_fn: FK,
        content_fn: FC,
    ) -> Result<(), String>
    where
        FK: Fn(&T) -> String,
        FC: Fn(&T) -> Option<&str>,
    {
        let mut primary_map = BTreeMap::<String, Option<String>>::new();
        let mut shadow_map = BTreeMap::<String, Option<String>>::new();

        for item in primary_items {
            primary_map.insert(key_fn(item), content_fn(item).map(|s| s.to_string()));
        }
        for item in shadow_items {
            shadow_map.insert(key_fn(item), content_fn(item).map(|s| s.to_string()));
        }

        let mut only_primary = Vec::new();
        let mut only_shadow = Vec::new();
        let mut content_mismatch = Vec::new();

        for (inscription_id, primary_content) in &primary_map {
            match shadow_map.get(inscription_id) {
                Some(shadow_content) => {
                    if primary_content != shadow_content {
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
            info!(
                "Inscription source match: module=inscription_source_compare, block_height={}, compare_target={}, primary_source={}, shadow_source={}, count={}",
                block_height,
                target_label,
                self.primary.source_name(),
                self.shadow.source_name(),
                primary_items.len()
            );
            return Ok(());
        }

        warn!(
            "Inscription source mismatch: module=inscription_source_compare, block_height={}, compare_target={}, primary_source={}, shadow_source={}, primary_count={}, shadow_count={}, only_primary_count={}, only_shadow_count={}, content_mismatch_count={}",
            block_height,
            target_label,
            self.primary.source_name(),
            self.shadow.source_name(),
            primary_items.len(),
            shadow_items.len(),
            only_primary.len(),
            only_shadow.len(),
            content_mismatch.len()
        );

        if !only_primary.is_empty() {
            let sample: Vec<_> = only_primary.iter().take(5).cloned().collect();
            warn!(
                "Inscription source mismatch details: module=inscription_source_compare, block_height={}, compare_target={}, only_primary_sample={:?}",
                block_height, target_label, sample
            );
        }
        if !only_shadow.is_empty() {
            let sample: Vec<_> = only_shadow.iter().take(5).cloned().collect();
            warn!(
                "Inscription source mismatch details: module=inscription_source_compare, block_height={}, compare_target={}, only_shadow_sample={:?}",
                block_height, target_label, sample
            );
        }
        if !content_mismatch.is_empty() {
            let sample: Vec<_> = content_mismatch.iter().take(5).cloned().collect();
            warn!(
                "Inscription source mismatch details: module=inscription_source_compare, block_height={}, compare_target={}, content_mismatch_sample={:?}",
                block_height, target_label, sample
            );

            if let Some(inscription_id) = content_mismatch.first() {
                let primary_content = primary_map.get(inscription_id).cloned().flatten();
                let shadow_content = shadow_map.get(inscription_id).cloned().flatten();
                let preview_len = 160usize;
                let primary_preview = primary_content
                    .as_deref()
                    .map(|s| s.chars().take(preview_len).collect::<String>())
                    .unwrap_or_else(|| "<none>".to_string())
                    .replace('\n', "\\n")
                    .replace('\r', "\\r");
                let shadow_preview = shadow_content
                    .as_deref()
                    .map(|s| s.chars().take(preview_len).collect::<String>())
                    .unwrap_or_else(|| "<none>".to_string())
                    .replace('\n', "\\n")
                    .replace('\r', "\\r");
                let primary_len = primary_content.as_ref().map(|s| s.len()).unwrap_or(0);
                let shadow_len = shadow_content.as_ref().map(|s| s.len()).unwrap_or(0);

                warn!(
                    "Inscription content mismatch detail: module=inscription_source_compare, block_height={}, compare_target={}, inscription_id={}, primary_len={}, shadow_len={}, primary_preview=\"{}\", shadow_preview=\"{}\"",
                    block_height,
                    target_label,
                    inscription_id,
                    primary_len,
                    shadow_len,
                    primary_preview,
                    shadow_preview
                );
            }
        }

        if self.fail_fast {
            let msg = format!(
                "Inscription source compare failed at block {}: compare_target={}, only_primary={}, only_shadow={}, content_mismatch={}",
                block_height,
                target_label,
                only_primary.len(),
                only_shadow.len(),
                content_mismatch.len()
            );
            return Err(msg);
        }

        Ok(())
    }

    fn compare_block_mints(
        &self,
        block_height: u32,
        primary_mints: &[DiscoveredMint],
        shadow_mints: &[DiscoveredMint],
    ) -> Result<(), String> {
        self.compare_items(
            block_height,
            CompareTarget::UsdbMint.as_str(),
            primary_mints,
            shadow_mints,
            |mint| mint.inscription_id.to_string(),
            |mint| Some(mint.content_string.as_str()),
        )
    }

    fn compare_block_inscriptions(
        &self,
        block_height: u32,
        primary_inscriptions: &[DiscoveredInscription],
        shadow_inscriptions: &[DiscoveredInscription],
    ) -> Result<(), String> {
        self.compare_items(
            block_height,
            CompareTarget::RawInscription.as_str(),
            primary_inscriptions,
            shadow_inscriptions,
            |item| item.inscription_id.to_string(),
            |item| item.content_string.as_deref(),
        )
    }
}

impl InscriptionSource for CompareInscriptionSource {
    fn source_name(&self) -> &'static str {
        "compare"
    }

    fn load_block_inscriptions<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredInscription>, String>> {
        Box::pin(async move {
            let primary_inscriptions = self
                .primary
                .load_block_inscriptions(block_height, block_hint.clone())
                .await?;
            let shadow_inscriptions = self
                .shadow
                .load_block_inscriptions(block_height, block_hint)
                .await?;

            if self.target == CompareTarget::RawInscription {
                self.compare_block_inscriptions(
                    block_height,
                    &primary_inscriptions,
                    &shadow_inscriptions,
                )?;
            }

            Ok(primary_inscriptions)
        })
    }

    fn load_block_mints<'a>(
        &'a self,
        block_height: u32,
        block_hint: Option<Arc<Block>>,
    ) -> InscriptionSourceFuture<'a, Result<Vec<DiscoveredMint>, String>> {
        Box::pin(async move {
            if self.target == CompareTarget::UsdbMint {
                let primary_mints = self
                    .primary
                    .load_block_mints(block_height, block_hint.clone())
                    .await?;
                let shadow_mints = self
                    .shadow
                    .load_block_mints(block_height, block_hint)
                    .await?;

                self.compare_block_mints(block_height, &primary_mints, &shadow_mints)?;

                return Ok(primary_mints);
            }

            let primary_inscriptions = self
                .primary
                .load_block_inscriptions(block_height, block_hint.clone())
                .await?;
            let shadow_inscriptions = self
                .shadow
                .load_block_inscriptions(block_height, block_hint)
                .await?;

            self.compare_block_inscriptions(
                block_height,
                &primary_inscriptions,
                &shadow_inscriptions,
            )?;

            map_usdb_mints_from_inscriptions(primary_inscriptions)
        })
    }
}
