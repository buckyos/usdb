//! Immutable, height-indexed views of an existing BTC activation registry.
//!
//! Inspecting declared versions is separate from obtaining an executable indexer
//! context. Neither operation changes registry identities or activates new code.
use std::collections::BTreeSet;
use std::ops::RangeInclusive;
use std::sync::Arc;

use crate::{
    ActivationRegistryError, ActivationStatus, ActiveVersionSet, BtcActivationRegistry,
    BtcActivationRegistryScope, VersionFamily,
};

#[derive(Debug, Clone)]
struct RuleEpoch {
    start_height: u32,
    versions: Arc<ActiveVersionSet>,
    version_set_id: Arc<str>,
    indexer_support: Result<(), ActivationRegistryError>,
}

/// Read-only rule history for one exact, structurally validated registry revision.
/// Unknown future versions may be inspected, but cannot produce executable contexts.
#[derive(Debug, Clone)]
pub struct BtcRuleTimeline {
    registry_id: Arc<str>,
    registry_schema: String,
    scope: Arc<BtcActivationRegistryScope>,
    epochs: Vec<RuleEpoch>,
}

/// Supported indexer rules at one BTC height, created only by a validated timeline.
/// Private fields prevent callers from relabeling a context with another height or ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcRuleContext {
    registry_id: Arc<str>,
    scope: Arc<BtcActivationRegistryScope>,
    btc_height: u32,
    versions: Arc<ActiveVersionSet>,
    version_set_id: Arc<str>,
}

impl BtcRuleContext {
    /// Exact revision whose historical rules were selected.
    pub fn activation_registry_id(&self) -> &str {
        &self.registry_id
    }

    /// BTC source, USDB rules scope and stable lag bound to this context.
    pub fn scope(&self) -> &BtcActivationRegistryScope {
        &self.scope
    }

    /// Canonical BTC event or query height, not the USDB block number.
    pub fn btc_height(&self) -> u32 {
        self.btc_height
    }

    /// Complete version set supported by the current indexer implementation.
    pub fn active_version_set(&self) -> &ActiveVersionSet {
        &self.versions
    }

    /// Unchanged UIP-0008 identity of the selected version set.
    pub fn active_version_set_id(&self) -> &str {
        &self.version_set_id
    }
}

/// One declared rule interval, clipped to an inclusive caller-supplied height range.
/// This is an inspection result, not proof that the indexer supports these versions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcRuleInterval<'a> {
    /// First included block; an activation at this height already applies.
    pub start_height: u32,
    /// Last included block. Inclusive bounds allow representing `u32::MAX` safely.
    pub end_height: u32,
    /// Versions declared by the registry throughout this interval.
    pub active_version_set: &'a ActiveVersionSet,
}

/// An unsupported interval reported by a read-only indexer capability check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtcRuleSupportIssue {
    /// First included unsupported height in the requested range.
    pub start_height: u32,
    /// Last included unsupported height in the requested range.
    pub end_height: u32,
    /// Original structured error, preserved for RPC error classification.
    pub error: ActivationRegistryError,
}

impl BtcRuleTimeline {
    /// Validate and cache a registry without requiring support for future versions.
    /// All version families activated at the same height share one interval boundary.
    pub fn new(registry: &BtcActivationRegistry) -> Result<Self, ActivationRegistryError> {
        let result = (|| {
            registry.validate()?;
            let heights = registry
                .records
                .iter()
                .filter(|record| record.status == ActivationStatus::Active)
                .map(|record| record.activation_height as u32)
                .collect::<BTreeSet<_>>();
            // validate() guarantees every record height fits u32. Reuse the canonical
            // lookup here rather than inventing a second version-selection algorithm.
            let epochs = heights
                .into_iter()
                .map(|start_height| {
                    let versions = registry.lookup_active_version_set(start_height)?;
                    Ok(RuleEpoch {
                        start_height,
                        version_set_id: versions.active_version_set_id().into(),
                        indexer_support: versions.validate_btc_indexer(),
                        versions: Arc::new(versions),
                    })
                })
                .collect::<Result<Vec<_>, ActivationRegistryError>>()?;
            Ok(Self {
                registry_id: registry.activation_registry_id().into(),
                registry_schema: registry.schema_version.clone(),
                scope: Arc::new(registry.scope.clone()),
                epochs,
            })
        })();
        result.inspect_err(|error| {
            error!(
                "Failed to build BTC rule timeline: network_id={}, rules_scope={}, error={error}",
                registry.scope.network_id,
                registry.scope.rules_scope()
            );
        })
    }

    /// Exact registry identity, including audit metadata and future declarations.
    pub fn activation_registry_id(&self) -> &str {
        &self.registry_id
    }

    /// Immutable BTC source and rules domain of this timeline.
    pub fn scope(&self) -> &BtcActivationRegistryScope {
        &self.scope
    }

    fn epoch_index(&self, height: u32) -> Result<usize, ActivationRegistryError> {
        self.epochs
            .partition_point(|epoch| epoch.start_height <= height)
            .checked_sub(1)
            .ok_or_else(|| {
                let detail = format!(
                    "No active BTC rules: registry_id={}, network_id={}, rules_scope={}, block_height={height}",
                    self.registry_id, self.scope.network_id, self.scope.rules_scope()
                );
                error!("{detail}");
                ActivationRegistryError::ActivationRecordNotFound(detail)
            })
    }

    /// Inspect declared versions in O(log boundaries), including unsupported versions.
    pub fn version_set_at(
        &self,
        height: u32,
    ) -> Result<&ActiveVersionSet, ActivationRegistryError> {
        Ok(&self.epochs[self.epoch_index(height)?].versions)
    }

    /// Resolve a supported, immutable execution context before any block mutation.
    pub fn indexer_context_at(
        &self,
        height: u32,
    ) -> Result<BtcRuleContext, ActivationRegistryError> {
        let epoch = &self.epochs[self.epoch_index(height)?];
        epoch.indexer_support.clone().inspect_err(|error| {
            self.log_support_error(height, height, error);
        })?;
        Ok(BtcRuleContext {
            registry_id: self.registry_id.clone(),
            scope: self.scope.clone(),
            btc_height: height,
            versions: epoch.versions.clone(),
            version_set_id: epoch.version_set_id.clone(),
        })
    }

    /// Enumerate declared intervals covering every height in a nonempty inclusive range.
    /// Planned/deferred/superseded records never create boundaries. Work is proportional
    /// to the number of boundaries, not the number of BTC blocks in the range.
    pub fn intervals(
        &self,
        heights: RangeInclusive<u32>,
    ) -> Result<Vec<BtcRuleInterval<'_>>, ActivationRegistryError> {
        let (start, end) = (*heights.start(), *heights.end());
        if heights.is_empty() {
            let detail = format!(
                "Invalid BTC rule range: registry_id={}, start_height={start}, end_height={end}",
                self.registry_id
            );
            error!("{detail}");
            return Err(ActivationRegistryError::InvalidRecord(detail));
        }
        let first = self.epoch_index(start)?;
        let after_last = self
            .epochs
            .partition_point(|epoch| epoch.start_height <= end);
        Ok((first..after_last)
            .map(|index| BtcRuleInterval {
                start_height: start.max(self.epochs[index].start_height),
                end_height: self
                    .epochs
                    .get(index + 1)
                    .map_or(end, |next| end.min(next.start_height - 1)),
                active_version_set: &self.epochs[index].versions,
            })
            .collect())
    }

    /// Report unsupported declared intervals without changing the current pin or data.
    /// An error means the range itself cannot be resolved; issues describe unsupported code.
    pub fn indexer_support_issues(
        &self,
        heights: RangeInclusive<u32>,
    ) -> Result<Vec<BtcRuleSupportIssue>, ActivationRegistryError> {
        let intervals = self.intervals(heights)?;
        Ok(intervals
            .into_iter()
            .filter_map(|interval| {
                interval
                    .active_version_set
                    .validate_btc_indexer()
                    .err()
                    .map(|error| BtcRuleSupportIssue {
                        start_height: interval.start_height,
                        end_height: interval.end_height,
                        error,
                    })
            })
            .collect())
    }

    /// Require support for every interval, including an unsupported intermediate epoch
    /// followed by a supported one. This checks rule support, not future migration functions.
    pub fn validate_indexer_range(
        &self,
        heights: RangeInclusive<u32>,
    ) -> Result<(), ActivationRegistryError> {
        if let Some(issue) = self.indexer_support_issues(heights)?.into_iter().next() {
            self.log_support_error(issue.start_height, issue.end_height, &issue.error);
            return Err(issue.error);
        }
        Ok(())
    }

    fn log_support_error(&self, start: u32, end: u32, error: &ActivationRegistryError) {
        error!(
            "Unsupported BTC rule interval: registry_id={}, network_id={}, rules_scope={}, start_height={start}, end_height={end}, error={error}",
            self.registry_id,
            self.scope.network_id,
            self.scope.rules_scope()
        );
    }

    /// Compare complete declared rule histories on [origin, through], not just the last set.
    /// Both timelines must refer to the same source/scope/stable lag and registry schema.
    /// Audit notes and future declarations may differ. Equality does not authorize changing
    /// a dataset binding, establish code support, or prove a state conversion compatible.
    pub fn ensure_same_history(
        &self,
        other: &Self,
        origin: u32,
        through: u32,
    ) -> Result<(), ActivationRegistryError> {
        let result = (|| {
            if self.scope != other.scope || self.registry_schema != other.registry_schema {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "BTC rule history domain mismatch: left_registry={}, right_registry={}, left_schema={}, right_schema={}, left_scope={:?}, right_scope={:?}, origin={origin}, through={through}",
                    self.registry_id,
                    other.registry_id,
                    self.registry_schema,
                    other.registry_schema,
                    self.scope,
                    other.scope
                )));
            }
            let left = self.intervals(origin..=through)?;
            let right = other.intervals(origin..=through)?;
            let (mut i, mut j) = (0, 0);
            while i < left.len() && j < right.len() {
                let (a, b) = (&left[i], &right[j]);
                if a.active_version_set != b.active_version_set {
                    let family = VersionFamily::ALL.into_iter().find(|family| {
                        a.active_version_set.get(*family) != b.active_version_set.get(*family)
                    });
                    return Err(ActivationRegistryError::InvalidRecord(format!(
                        "BTC rule history differs: left_registry={}, right_registry={}, origin={origin}, through={through}, first_difference_height={}, family={family:?}, left_versions={}, right_versions={}",
                        self.registry_id,
                        other.registry_id,
                        a.start_height.max(b.start_height),
                        a.active_version_set.active_version_set_id(),
                        b.active_version_set.active_version_set_id()
                    )));
                }
                if a.end_height <= b.end_height {
                    i += 1;
                }
                if b.end_height <= a.end_height {
                    j += 1;
                }
            }
            Ok(())
        })();
        result.inspect_err(|error| error!("Failed to compare BTC rule histories: {error}"))
    }
}
