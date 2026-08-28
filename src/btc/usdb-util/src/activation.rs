use bitcoincore_rpc::bitcoin::Network;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{Display, Formatter};
use std::sync::OnceLock;

/// Schema identifier for one network-scoped UIP-0008 BTC activation registry.
pub const ACTIVATION_REGISTRY_SCHEMA_VERSION: &str = "uip-0008-btc-activation-registry:v2";
/// Hash domain used by the canonical network-scoped BTC registry encoding.
pub const ACTIVATION_REGISTRY_HASH_DOMAIN: &str = "usdb-btc-activation-registry:v2";
/// Hash domain used by the canonical active-version-set encoding.
pub const ACTIVE_VERSION_SET_HASH_DOMAIN: &str = "usdb-active-version-set:v1";
/// Hash algorithm used by registry and active-version-set identifiers.
pub const ACTIVATION_ID_HASH_ALGO: &str = "sha256";
/// Schema identifier for the audit-only cross-chain release manifest.
pub const RELEASE_MANIFEST_SCHEMA_VERSION: &str = "uip-0008-cross-chain-release-manifest:v3";

/// UIP-0001 inscription schema implemented by the current BTC indexer.
pub const INSCRIPTION_SCHEMA_VERSION_V1: &str = "uip-0001-miner-pass-inscription:v1";
/// UIP-0002 pass state-machine version implemented by the current BTC indexer.
pub const PASS_STATE_MACHINE_VERSION_V1: &str = "uip-0002-pass-state-machine:v1";
/// UIP-0003 raw-energy formula implemented by the current BTC indexer.
pub const ENERGY_FORMULA_VERSION_V1: &str = "uip-0003-pass-energy-formula:v1";
/// UIP-0004 effective-energy formula implemented by the current BTC indexer.
pub const EFFECTIVE_ENERGY_FORMULA_VERSION_V1: &str = "uip-0004-collab-leader-effective-energy:v1";
/// UIP-0005 level and difficulty-factor formula implemented by the current BTC indexer.
pub const LEVEL_FORMULA_VERSION_V1: &str = "uip-0005-level-and-real-difficulty:v1";
/// Historical query and pagination semantics implemented by the current indexer RPC.
pub const QUERY_SEMANTICS_VERSION_V1: &str = "uip-0006-economic-query-semantics:v1";
/// Local-state commit protocol that binds a UIP-0008 active version set.
pub const COMMIT_PROTOCOL_VERSION_V1: &str = "uip-0008-usdb-local-state-commit:v1";
/// Balance-history lookup semantics implemented by the current service.
pub const BALANCE_HISTORY_SEMANTICS_VERSION_V1: &str = "balance-snapshot-at-or-before:v1";

const BTC_INDEXER_V1_FAMILIES: [VersionFamily; 9] = [
    VersionFamily::InscriptionSchemaVersion,
    VersionFamily::PassStateMachineVersion,
    VersionFamily::EnergyFormulaVersion,
    VersionFamily::EffectiveEnergyFormulaVersion,
    VersionFamily::LevelFormulaVersion,
    VersionFamily::QuerySemanticsVersion,
    VersionFamily::StateViewVersion,
    VersionFamily::CommitProtocolVersion,
    VersionFamily::BalanceHistorySemanticsVersion,
];

const EMBEDDED_BTC_MAINNET_REGISTRY_JSON: &str =
    include_str!("../activation-registry/btc-mainnet.json");
const EMBEDDED_BTC_REGTEST_REGISTRY_JSON: &str =
    include_str!("../activation-registry/btc-regtest.json");
const EMBEDDED_BTC_REGTEST_REGISTRY_REVISION_2_JSON: &str =
    include_str!("../activation-registry/btc-regtest-revision-2.json");
const EMBEDDED_RELEASE_MANIFEST_JSON: &str = include_str!("../release-manifest.json");

static EMBEDDED_BTC_MAINNET_CATALOG: OnceLock<
    Result<BtcActivationRegistryCatalog, ActivationRegistryError>,
> = OnceLock::new();
static EMBEDDED_BTC_REGTEST_CATALOG: OnceLock<
    Result<BtcActivationRegistryCatalog, ActivationRegistryError>,
> = OnceLock::new();
static EMBEDDED_RELEASE_MANIFEST: OnceLock<
    Result<CrossChainReleaseManifest, ActivationRegistryError>,
> = OnceLock::new();

/// Protocol-version family names standardized by UIP-0008.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum VersionFamily {
    InscriptionSchemaVersion,
    PassStateMachineVersion,
    EnergyFormulaVersion,
    EffectiveEnergyFormulaVersion,
    LevelFormulaVersion,
    QuerySemanticsVersion,
    StateViewVersion,
    PayloadVersion,
    DifficultyPolicyVersion,
    RewardRuleVersion,
    CoinbaseEmissionPolicyVersion,
    FeeSplitPolicyVersion,
    CollaborationEfficiencyPolicyVersion,
    PricePolicyVersion,
    QuotePolicyVersion,
    AuxPoolPolicyVersion,
    CommitProtocolVersion,
    BalanceHistorySemanticsVersion,
}

impl VersionFamily {
    /// Stable field order used by active-version-set canonical encoding.
    pub const ALL: [Self; 18] = [
        Self::InscriptionSchemaVersion,
        Self::PassStateMachineVersion,
        Self::EnergyFormulaVersion,
        Self::EffectiveEnergyFormulaVersion,
        Self::LevelFormulaVersion,
        Self::QuerySemanticsVersion,
        Self::StateViewVersion,
        Self::PayloadVersion,
        Self::DifficultyPolicyVersion,
        Self::RewardRuleVersion,
        Self::CoinbaseEmissionPolicyVersion,
        Self::FeeSplitPolicyVersion,
        Self::CollaborationEfficiencyPolicyVersion,
        Self::PricePolicyVersion,
        Self::QuotePolicyVersion,
        Self::AuxPoolPolicyVersion,
        Self::CommitProtocolVersion,
        Self::BalanceHistorySemanticsVersion,
    ];

    /// Canonical snake-case field name used by registry JSON and hash inputs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InscriptionSchemaVersion => "inscription_schema_version",
            Self::PassStateMachineVersion => "pass_state_machine_version",
            Self::EnergyFormulaVersion => "energy_formula_version",
            Self::EffectiveEnergyFormulaVersion => "effective_energy_formula_version",
            Self::LevelFormulaVersion => "level_formula_version",
            Self::QuerySemanticsVersion => "query_semantics_version",
            Self::StateViewVersion => "state_view_version",
            Self::PayloadVersion => "payload_version",
            Self::DifficultyPolicyVersion => "difficulty_policy_version",
            Self::RewardRuleVersion => "reward_rule_version",
            Self::CoinbaseEmissionPolicyVersion => "coinbase_emission_policy_version",
            Self::FeeSplitPolicyVersion => "fee_split_policy_version",
            Self::CollaborationEfficiencyPolicyVersion => "collaboration_efficiency_policy_version",
            Self::PricePolicyVersion => "price_policy_version",
            Self::QuotePolicyVersion => "quote_policy_version",
            Self::AuxPoolPolicyVersion => "aux_pool_policy_version",
            Self::CommitProtocolVersion => "commit_protocol_version",
            Self::BalanceHistorySemanticsVersion => "balance_history_semantics_version",
        }
    }

    fn integer_max(self) -> Option<u64> {
        match self {
            Self::PayloadVersion => Some(u8::MAX as u64),
            Self::PricePolicyVersion => Some(u32::MAX as u64),
            Self::DifficultyPolicyVersion
            | Self::RewardRuleVersion
            | Self::CoinbaseEmissionPolicyVersion
            | Self::FeeSplitPolicyVersion
            | Self::CollaborationEfficiencyPolicyVersion
            | Self::QuotePolicyVersion
            | Self::AuxPoolPolicyVersion => Some(u16::MAX as u64),
            _ => None,
        }
    }
}

/// Typed value stored by one version family.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(untagged)]
pub enum VersionValue {
    String(String),
    Integer(u64),
}

impl Display for VersionValue {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::String(value) => f.write_str(value),
            Self::Integer(value) => write!(f, "{}", value),
        }
    }
}

/// Network class used by BTC registries and audit-only release bindings.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ActivationNetworkType {
    Mainnet,
    Testnet,
    Signet,
    Regtest,
    Devnet,
    Local,
}

impl ActivationNetworkType {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "mainnet",
            Self::Testnet => "testnet",
            Self::Signet => "signet",
            Self::Regtest => "regtest",
            Self::Devnet => "devnet",
            Self::Local => "local",
        }
    }
}

/// Lifecycle status of one registry record.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ActivationStatus {
    Planned,
    Active,
    Deferred,
    Superseded,
}

impl ActivationStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "Planned",
            Self::Active => "Active",
            Self::Deferred => "Deferred",
            Self::Superseded => "Superseded",
        }
    }
}

/// Immutable BTC network scope shared by every record in one registry file.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BtcActivationRegistryScope {
    /// Network class used to prevent one registry from serving another network.
    pub network_type: ActivationNetworkType,
    /// Canonical network identifier, such as `btc-mainnet` or `btc-regtest`.
    pub network_id: String,
    /// Number of canonical BTC confirmations excluded from the exposed stable view.
    ///
    /// This is a network protocol value committed by `activation_registry_id`,
    /// not an operator tuning parameter.
    pub stable_lag_blocks: u32,
}

impl BtcActivationRegistryScope {
    fn network_identity(network: Network) -> (ActivationNetworkType, &'static str) {
        match network {
            Network::Bitcoin => (ActivationNetworkType::Mainnet, "btc-mainnet"),
            Network::Testnet => (ActivationNetworkType::Testnet, "btc-testnet3"),
            Network::Testnet4 => (ActivationNetworkType::Testnet, "btc-testnet4"),
            Network::Signet => (ActivationNetworkType::Signet, "btc-signet"),
            Network::Regtest => (ActivationNetworkType::Regtest, "btc-regtest"),
        }
    }
}

/// One height-indexed record in a network-scoped BTC activation registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BtcActivationRecord {
    /// UIP that defines the activated behavior.
    pub uip: String,
    /// BTC-side version family selected by this record.
    pub version_family: VersionFamily,
    /// Concrete protocol or formula version selected at the activation height.
    pub version_value: VersionValue,
    /// First BTC block height interpreted with this version.
    pub activation_height: u64,
    /// Lifecycle status; only `Active` records affect runtime lookup.
    pub status: ActivationStatus,
    /// Previous active version, required for subsequent active records.
    pub supersedes: Option<VersionValue>,
    /// Human-readable release and review context.
    pub notes: String,
}

/// Versions active in one exact chain context.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(transparent)]
pub struct ActiveVersionSet(BTreeMap<VersionFamily, VersionValue>);

impl ActiveVersionSet {
    /// Returns one active value, or `None` when the family is not activated.
    pub fn get(&self, family: VersionFamily) -> Option<&VersionValue> {
        self.0.get(&family)
    }

    /// Returns the active string value and rejects missing or incorrectly typed families.
    pub fn require_string(&self, family: VersionFamily) -> Result<&str, ActivationRegistryError> {
        match self.get(family) {
            Some(VersionValue::String(value)) => Ok(value),
            Some(value) => Err(ActivationRegistryError::InvalidRecord(format!(
                "{} must be a string, got {}",
                family.as_str(),
                value
            ))),
            None => Err(ActivationRegistryError::ActivationRecordNotFound(format!(
                "missing active {}",
                family.as_str()
            ))),
        }
    }

    /// Returns the active integer value and rejects missing or incorrectly typed families.
    pub fn require_integer(&self, family: VersionFamily) -> Result<u64, ActivationRegistryError> {
        match self.get(family) {
            Some(VersionValue::Integer(value)) => Ok(*value),
            Some(value) => Err(ActivationRegistryError::InvalidRecord(format!(
                "{} must be an integer, got {}",
                family.as_str(),
                value
            ))),
            None => Err(ActivationRegistryError::ActivationRecordNotFound(format!(
                "missing active {}",
                family.as_str()
            ))),
        }
    }

    /// Computes the canonical SHA-256 identity of this version set.
    pub fn active_version_set_id(&self) -> String {
        let mut hasher = Sha256::new();
        update_string(&mut hasher, ACTIVE_VERSION_SET_HASH_DOMAIN);
        for family in VersionFamily::ALL {
            update_string(&mut hasher, family.as_str());
            match self.get(family) {
                Some(value) => {
                    hasher.update([1]);
                    update_version_value(&mut hasher, value);
                }
                None => hasher.update([0]),
            }
        }
        encode_hex(&hasher.finalize())
    }

    /// Verifies that all BTC indexer families select the currently implemented v1 rules.
    pub fn validate_btc_indexer_v1(&self) -> Result<(), ActivationRegistryError> {
        for (family, value) in &self.0 {
            if !BTC_INDEXER_V1_FAMILIES.contains(family) {
                return Err(ActivationRegistryError::VersionNotSupported {
                    family: *family,
                    value: value.to_string(),
                });
            }
        }
        self.require_supported_string(
            VersionFamily::InscriptionSchemaVersion,
            INSCRIPTION_SCHEMA_VERSION_V1,
        )?;
        self.require_supported_string(
            VersionFamily::PassStateMachineVersion,
            PASS_STATE_MACHINE_VERSION_V1,
        )?;
        self.require_supported_string(
            VersionFamily::EnergyFormulaVersion,
            ENERGY_FORMULA_VERSION_V1,
        )?;
        self.require_supported_string(
            VersionFamily::EffectiveEnergyFormulaVersion,
            EFFECTIVE_ENERGY_FORMULA_VERSION_V1,
        )?;
        self.require_supported_string(
            VersionFamily::LevelFormulaVersion,
            LEVEL_FORMULA_VERSION_V1,
        )?;
        self.require_supported_string(
            VersionFamily::QuerySemanticsVersion,
            QUERY_SEMANTICS_VERSION_V1,
        )?;
        self.require_supported_string(
            VersionFamily::StateViewVersion,
            crate::USDB_ECONOMIC_STATE_VIEW_VERSION,
        )?;
        self.require_supported_string(
            VersionFamily::CommitProtocolVersion,
            COMMIT_PROTOCOL_VERSION_V1,
        )?;
        self.require_supported_string(
            VersionFamily::BalanceHistorySemanticsVersion,
            BALANCE_HISTORY_SEMANTICS_VERSION_V1,
        )
    }

    /// Verifies the balance-history family without interpreting indexer formulas.
    pub fn validate_balance_history_v1(&self) -> Result<(), ActivationRegistryError> {
        self.require_supported_string(
            VersionFamily::BalanceHistorySemanticsVersion,
            BALANCE_HISTORY_SEMANTICS_VERSION_V1,
        )
    }

    fn require_supported_string(
        &self,
        family: VersionFamily,
        expected: &str,
    ) -> Result<(), ActivationRegistryError> {
        let actual = self.require_string(family)?;
        if actual != expected {
            return Err(ActivationRegistryError::VersionNotSupported {
                family,
                value: actual.to_string(),
            });
        }
        Ok(())
    }
}

/// Parsed machine-readable activation registry for exactly one BTC network.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BtcActivationRegistry {
    /// Registry schema and canonical-encoding version.
    pub schema_version: String,
    /// BTC network whose historical heights this file is allowed to interpret.
    pub scope: BtcActivationRegistryScope,
    /// Height-indexed activation records for the scoped BTC network.
    pub records: Vec<BtcActivationRecord>,
}

impl BtcActivationRegistry {
    /// Parses registry JSON, rejects unknown fields, and validates BTC invariants.
    pub fn from_json(json: &str) -> Result<Self, ActivationRegistryError> {
        let registry: Self = serde_json::from_str(json)
            .map_err(|error| ActivationRegistryError::Parse(error.to_string()))?;
        registry.validate()?;
        Ok(registry)
    }

    /// Validates schema, network scope, BTC family types, conflicts, and supersedes chains.
    pub fn validate(&self) -> Result<(), ActivationRegistryError> {
        if self.schema_version != ACTIVATION_REGISTRY_SCHEMA_VERSION {
            return Err(ActivationRegistryError::UnsupportedSchemaVersion(
                self.schema_version.clone(),
            ));
        }
        if self.scope.network_id.is_empty() {
            return Err(ActivationRegistryError::InvalidRecord(
                "BTC activation registry has an empty network_id".to_string(),
            ));
        }
        if self.scope.stable_lag_blocks == 0 {
            return Err(ActivationRegistryError::InvalidRecord(format!(
                "BTC activation registry {} has zero stable_lag_blocks",
                self.scope.network_id
            )));
        }
        if matches!(
            self.scope.network_type,
            ActivationNetworkType::Devnet | ActivationNetworkType::Local
        ) {
            return Err(ActivationRegistryError::InvalidRecord(format!(
                "BTC activation registry {} has invalid network_type {}",
                self.scope.network_id,
                self.scope.network_type.as_str()
            )));
        }
        if self.records.is_empty() {
            return Err(ActivationRegistryError::InvalidRecord(format!(
                "BTC activation registry {} has no records",
                self.scope.network_id
            )));
        }

        let mut exact_records = BTreeSet::new();
        let mut active_groups: BTreeMap<VersionFamily, Vec<&BtcActivationRecord>> = BTreeMap::new();

        for record in &self.records {
            self.validate_record(record)?;
            let exact_key = canonical_record_sort_key(record);
            if !exact_records.insert(exact_key) {
                return Err(ActivationRegistryError::ActivationRecordConflict(format!(
                    "duplicate {} activation record on {}",
                    record.version_family.as_str(),
                    self.scope.network_id
                )));
            }
            if record.status == ActivationStatus::Active {
                active_groups
                    .entry(record.version_family)
                    .or_default()
                    .push(record);
            }
        }

        for (family, records) in &mut active_groups {
            records.sort_by_key(|record| record.activation_height);
            let mut previous: Option<&BtcActivationRecord> = None;
            for record in records.iter().copied() {
                if let Some(previous) = previous {
                    if record.activation_height == previous.activation_height {
                        return Err(ActivationRegistryError::ActivationRecordConflict(format!(
                            "multiple active {} records at BTC height {} on {}",
                            family.as_str(),
                            record.activation_height,
                            self.scope.network_id
                        )));
                    }
                    if record.supersedes.as_ref() != Some(&previous.version_value) {
                        return Err(ActivationRegistryError::InvalidRecord(format!(
                            "active {}={} at BTC height {} on {} must supersede previous value {}",
                            family.as_str(),
                            record.version_value,
                            record.activation_height,
                            self.scope.network_id,
                            previous.version_value
                        )));
                    }
                } else if record.supersedes.is_some() {
                    return Err(ActivationRegistryError::InvalidRecord(format!(
                        "first active {} record on {} cannot declare supersedes",
                        family.as_str(),
                        self.scope.network_id
                    )));
                }
                previous = Some(record);
            }
        }
        Ok(())
    }

    /// Verifies that this registry is the embedded artifact selected for `network`.
    pub fn validate_network(&self, network: Network) -> Result<(), ActivationRegistryError> {
        let (expected_type, expected_id) = BtcActivationRegistryScope::network_identity(network);
        if self.scope.network_type != expected_type || self.scope.network_id != expected_id {
            return Err(ActivationRegistryError::InvalidRecord(format!(
                "BTC activation registry scope mismatch: selected network={}, expected_type={}, expected_id={}, actual_type={}, actual_id={}",
                network,
                expected_type.as_str(),
                expected_id,
                self.scope.network_type.as_str(),
                self.scope.network_id
            )));
        }
        Ok(())
    }

    /// Returns the immutable stable-view lag committed by this registry revision.
    pub const fn stable_lag_blocks(&self) -> u32 {
        self.scope.stable_lag_blocks
    }

    /// Resolves all active BTC-side families at one exact historical BTC height.
    pub fn lookup_active_version_set(
        &self,
        block_height: u32,
    ) -> Result<ActiveVersionSet, ActivationRegistryError> {
        let block_height = u64::from(block_height);
        let mut selected: BTreeMap<VersionFamily, &BtcActivationRecord> = BTreeMap::new();
        for record in &self.records {
            if record.status != ActivationStatus::Active || record.activation_height > block_height
            {
                continue;
            }
            match selected.get(&record.version_family) {
                Some(selected) if selected.activation_height >= record.activation_height => {}
                _ => {
                    selected.insert(record.version_family, record);
                }
            }
        }

        if selected.is_empty() {
            return Err(ActivationRegistryError::ActivationRecordNotFound(format!(
                "no active BTC records for network_type={} network_id={} height={}",
                self.scope.network_type.as_str(),
                self.scope.network_id,
                block_height
            )));
        }
        Ok(ActiveVersionSet(
            selected
                .into_iter()
                .map(|(family, record)| (family, record.version_value.clone()))
                .collect(),
        ))
    }

    /// Computes the network-scoped registry ID from an explicit canonical encoding.
    pub fn activation_registry_id(&self) -> String {
        let mut records = self.records.iter().collect::<Vec<_>>();
        records.sort_by_key(|record| canonical_record_sort_key(record));

        let mut hasher = Sha256::new();
        update_string(&mut hasher, ACTIVATION_REGISTRY_HASH_DOMAIN);
        update_string(&mut hasher, &self.schema_version);
        update_string(&mut hasher, "BTC");
        update_string(&mut hasher, self.scope.network_type.as_str());
        update_string(&mut hasher, &self.scope.network_id);
        update_string(&mut hasher, "stable_lag_blocks");
        hasher.update(self.scope.stable_lag_blocks.to_be_bytes());
        update_string(&mut hasher, "btc_height");
        hasher.update((records.len() as u32).to_be_bytes());
        for record in records {
            update_string(&mut hasher, &record.uip);
            update_string(&mut hasher, record.version_family.as_str());
            update_version_value(&mut hasher, &record.version_value);
            hasher.update(record.activation_height.to_be_bytes());
            update_string(&mut hasher, record.status.as_str());
            match &record.supersedes {
                Some(value) => {
                    hasher.update([1]);
                    update_version_value(&mut hasher, value);
                }
                None => hasher.update([0]),
            }
            update_string(&mut hasher, &record.notes);
        }
        encode_hex(&hasher.finalize())
    }

    fn validate_record(&self, record: &BtcActivationRecord) -> Result<(), ActivationRegistryError> {
        if !record.uip.starts_with("UIP-") || record.notes.is_empty() {
            return Err(ActivationRegistryError::InvalidRecord(format!(
                "BTC activation record has incomplete identity: {:?}",
                record
            )));
        }
        if record.activation_height > u64::from(u32::MAX) {
            return Err(ActivationRegistryError::InvalidRecord(format!(
                "BTC activation height {} exceeds the payload uint32 range",
                record.activation_height
            )));
        }
        if !BTC_INDEXER_V1_FAMILIES.contains(&record.version_family) {
            return Err(ActivationRegistryError::InvalidRecord(format!(
                "BTC registry {} cannot contain USDB-chain family {}",
                self.scope.network_id,
                record.version_family.as_str()
            )));
        }
        match (record.version_family.integer_max(), &record.version_value) {
            (Some(max), VersionValue::Integer(value)) if *value <= max => {}
            (Some(_), value) => {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "{} has invalid integer value {}",
                    record.version_family.as_str(),
                    value
                )));
            }
            (None, VersionValue::String(value)) if !value.is_empty() => {}
            (None, value) => {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "{} has invalid string value {}",
                    record.version_family.as_str(),
                    value
                )));
            }
        }
        Ok(())
    }
}

/// Immutable revision catalog for one BTC network activation registry.
///
/// Revisions are ordered oldest to newest. A later revision must preserve every
/// earlier record exactly, and newly active records must extend their version
/// family beyond the previous activation height. This keeps historical lookups
/// reproducible while allowing a future activation to be announced before it
/// becomes active.
#[derive(Debug, Clone)]
pub struct BtcActivationRegistryCatalog {
    current_registry_id: String,
    registry_ids: Vec<String>,
    registries: BTreeMap<String, BtcActivationRegistry>,
}

impl BtcActivationRegistryCatalog {
    /// Builds and validates an ordered catalog whose final revision is current.
    pub fn from_revisions(
        revisions: Vec<BtcActivationRegistry>,
    ) -> Result<Self, ActivationRegistryError> {
        let current_registry_id = revisions
            .last()
            .map(BtcActivationRegistry::activation_registry_id)
            .ok_or_else(|| {
                ActivationRegistryError::InvalidRecord(
                    "BTC activation registry catalog has no revisions".to_string(),
                )
            })?;
        Self::from_revisions_with_current(revisions, &current_registry_id)
    }

    /// Builds a catalog with an explicitly selected default revision.
    pub fn from_revisions_with_current(
        revisions: Vec<BtcActivationRegistry>,
        current_registry_id: &str,
    ) -> Result<Self, ActivationRegistryError> {
        if revisions.is_empty() {
            return Err(ActivationRegistryError::InvalidRecord(
                "BTC activation registry catalog has no revisions".to_string(),
            ));
        }

        let expected_scope = revisions[0].scope.clone();
        let mut registry_ids = Vec::with_capacity(revisions.len());
        let mut registries = BTreeMap::new();
        let mut previous_records: Option<BTreeSet<CanonicalRecordSortKey>> = None;
        let mut previous_active_heights = BTreeMap::<VersionFamily, u64>::new();

        for revision in revisions {
            revision.validate()?;
            if revision.scope != expected_scope {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "BTC activation registry catalog mixes scopes {} and {}",
                    expected_scope.network_id, revision.scope.network_id
                )));
            }

            let registry_id = revision.activation_registry_id();
            if registries.contains_key(&registry_id) {
                return Err(ActivationRegistryError::ActivationRecordConflict(format!(
                    "duplicate BTC activation registry revision {}",
                    registry_id
                )));
            }

            let current_records = revision
                .records
                .iter()
                .map(canonical_record_sort_key)
                .collect::<BTreeSet<_>>();
            if let Some(previous_records) = previous_records.as_ref() {
                if !previous_records.is_subset(&current_records) {
                    return Err(ActivationRegistryError::InvalidRecord(format!(
                        "BTC activation registry revision {} rewrites prior records",
                        registry_id
                    )));
                }
                for record in &revision.records {
                    let key = canonical_record_sort_key(record);
                    if previous_records.contains(&key) || record.status != ActivationStatus::Active
                    {
                        continue;
                    }
                    if let Some(previous_height) =
                        previous_active_heights.get(&record.version_family)
                        && record.activation_height <= *previous_height
                    {
                        return Err(ActivationRegistryError::InvalidRecord(format!(
                            "BTC activation registry revision {} inserts historical {} activation at height {}",
                            registry_id,
                            record.version_family.as_str(),
                            record.activation_height
                        )));
                    }
                }
            }

            previous_active_heights.clear();
            for record in &revision.records {
                if record.status == ActivationStatus::Active {
                    previous_active_heights
                        .entry(record.version_family)
                        .and_modify(|height| *height = (*height).max(record.activation_height))
                        .or_insert(record.activation_height);
                }
            }
            previous_records = Some(current_records);
            registry_ids.push(registry_id.clone());
            registries.insert(registry_id, revision);
        }

        if !registries.contains_key(current_registry_id) {
            return Err(ActivationRegistryError::ActivationRecordNotFound(format!(
                "BTC activation registry catalog does not contain current revision {}",
                current_registry_id
            )));
        }
        Ok(Self {
            current_registry_id: current_registry_id.to_string(),
            registry_ids,
            registries,
        })
    }

    /// Returns the explicitly selected current revision for unpinned local queries.
    pub fn current_registry(&self) -> &BtcActivationRegistry {
        self.registries
            .get(&self.current_registry_id)
            .expect("validated catalog must contain its current registry")
    }

    /// Returns the canonical identity of the selected current revision.
    pub fn current_registry_id(&self) -> &str {
        &self.current_registry_id
    }

    /// Returns one immutable revision by canonical registry ID.
    pub fn registry_by_id(
        &self,
        registry_id: &str,
    ) -> Result<&BtcActivationRegistry, ActivationRegistryError> {
        self.registries.get(registry_id).ok_or_else(|| {
            ActivationRegistryError::ActivationRecordNotFound(format!(
                "BTC activation registry catalog {} does not contain revision {}",
                self.current_registry().scope.network_id,
                registry_id
            ))
        })
    }

    /// Returns registry IDs in immutable revision order, oldest first.
    pub fn registry_ids(&self) -> &[String] {
        &self.registry_ids
    }
}

/// BTC registry identity recorded in the audit-only cross-chain release manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BtcRegistryReleaseBinding {
    /// BTC network class of the referenced registry artifact.
    pub network_type: ActivationNetworkType,
    /// Canonical BTC network identifier of the referenced registry artifact.
    pub network_id: String,
    /// Repository-relative path of the referenced registry artifact.
    pub artifact: String,
    /// Monotonic immutable revision number within this BTC network catalog.
    pub revision: u32,
    /// Whether this revision is the BTC service's current default.
    pub current: bool,
    /// Canonical network-scoped activation registry ID.
    pub activation_registry_id: String,
}

/// One USDB-chain activation's BTC registry binding recorded for release auditing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsdbChainActivationReleaseBinding {
    /// USDB block where this binding becomes active.
    pub block: u64,
    /// Immutable BTC registry revision selected from this USDB block onward.
    pub btc_activation_registry_id: String,
    /// Maximum consecutive USDB blocks allowed to reuse one exact BTC anchor.
    pub btc_anchor_max_age_blocks: u32,
    /// Complete USDB-chain policy versions selected by this activation.
    pub versions: UsdbConsensusVersionsReleaseBinding,
}

/// Complete USDB-chain consensus policy versions recorded for release auditing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsdbConsensusVersionsReleaseBinding {
    /// Profile selector payload codec version.
    pub payload_version: u8,
    /// BTC-anchor parent/child transition policy version.
    pub btc_anchor_policy_version: u16,
    /// USDB PoW difficulty policy version.
    pub difficulty_policy_version: u16,
    /// Coinbase reward-allocation rule version.
    pub reward_rule_version: u16,
    /// Total coinbase emission policy version.
    pub coinbase_emission_policy_version: u16,
    /// Transaction-fee split policy version.
    pub fee_split_policy_version: u16,
    /// Collaboration efficiency/K policy version.
    pub collaboration_efficiency_policy_version: u16,
    /// Fixed or dynamic price policy version.
    pub price_policy_version: u32,
    /// Miner quote policy version.
    pub quote_policy_version: u16,
    /// Auxiliary hashpower-pool policy version.
    pub aux_pool_policy_version: u16,
}

/// USDB chain-config identity recorded for release auditing, never runtime lookup.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct UsdbChainConfigReleaseBinding {
    /// USDB network class of the chain-config artifact.
    pub network_type: ActivationNetworkType,
    /// Human-readable USDB network identifier.
    pub network_id: String,
    /// EIP-155 chain ID committed by the USDB genesis configuration.
    pub chain_id: u64,
    /// Canonical 32-byte USDB genesis block hash in lowercase hexadecimal.
    pub genesis_hash: String,
    /// Height-indexed BTC registry bindings committed by this USDB chain config.
    pub activations: Vec<UsdbChainActivationReleaseBinding>,
    /// Repository source that owns the USDB-chain activation schedule.
    pub source: String,
    /// Machine-readable declaration that USDB chain config is authoritative.
    pub activation_authority: String,
}

/// Audit-only manifest binding independently owned BTC and USDB-chain release artifacts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CrossChainReleaseManifest {
    /// Manifest schema version; this is not a consensus activation version.
    pub schema_version: String,
    /// Human-readable release identifier.
    pub release_id: String,
    /// BTC network-scoped registry artifacts shipped with the release.
    pub btc_activation_registries: Vec<BtcRegistryReleaseBinding>,
    /// USDB chain configs whose own genesis/chain config remains authoritative.
    pub usdb_chain_configs: Vec<UsdbChainConfigReleaseBinding>,
    /// Operator-facing explanation of the non-consensus manifest boundary.
    pub notes: String,
}

impl CrossChainReleaseManifest {
    /// Parses and validates the audit-only release manifest.
    pub fn from_json(json: &str) -> Result<Self, ActivationRegistryError> {
        let manifest: Self = serde_json::from_str(json)
            .map_err(|error| ActivationRegistryError::Parse(error.to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Validates release identities without interpreting USDB-chain activation rules.
    pub fn validate(&self) -> Result<(), ActivationRegistryError> {
        if self.schema_version != RELEASE_MANIFEST_SCHEMA_VERSION {
            return Err(ActivationRegistryError::UnsupportedSchemaVersion(
                self.schema_version.clone(),
            ));
        }
        if self.release_id.is_empty()
            || self.notes.is_empty()
            || self.btc_activation_registries.is_empty()
            || self.usdb_chain_configs.is_empty()
        {
            return Err(ActivationRegistryError::InvalidRecord(
                "cross-chain release manifest is incomplete".to_string(),
            ));
        }

        let mut btc_registry_keys = BTreeSet::new();
        let mut btc_registry_ids = BTreeSet::new();
        let mut btc_revisions: BTreeMap<&str, BTreeSet<u32>> = BTreeMap::new();
        let mut btc_current_networks = BTreeSet::new();
        for binding in &self.btc_activation_registries {
            if binding.network_id.is_empty()
                || binding.artifact.is_empty()
                || binding.revision == 0
                || !is_canonical_hex_32(&binding.activation_registry_id)
                || !btc_registry_keys.insert((binding.network_id.clone(), binding.revision))
                || !btc_registry_ids.insert(binding.activation_registry_id.clone())
                || (binding.current && !btc_current_networks.insert(binding.network_id.as_str()))
            {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "invalid BTC release binding for network {}",
                    binding.network_id
                )));
            }
            btc_revisions
                .entry(binding.network_id.as_str())
                .or_default()
                .insert(binding.revision);
        }
        for (network_id, revisions) in &btc_revisions {
            if !btc_current_networks.contains(network_id)
                || revisions
                    .iter()
                    .copied()
                    .ne(1..=u32::try_from(revisions.len()).map_err(|_| {
                        ActivationRegistryError::InvalidRecord(format!(
                            "too many BTC registry revisions for network {network_id}"
                        ))
                    })?)
            {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "invalid BTC registry revision sequence for network {network_id}"
                )));
            }
        }

        let mut usdb_network_ids = BTreeSet::new();
        for binding in &self.usdb_chain_configs {
            if binding.network_id.is_empty()
                || binding.chain_id == 0
                || !is_canonical_hex_32(&binding.genesis_hash)
                || binding.activations.is_empty()
                || binding.source.is_empty()
                || binding.activation_authority != "chain_config.usdb.activations"
                || !usdb_network_ids.insert(binding.network_id.clone())
            {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "invalid USDB-chain release binding for network {}",
                    binding.network_id
                )));
            }
            let mut previous_block = None;
            for activation in &binding.activations {
                if !btc_registry_ids.contains(&activation.btc_activation_registry_id)
                    || activation.btc_anchor_max_age_blocks == 0
                    || activation.versions.payload_version == 0
                    || activation.versions.btc_anchor_policy_version == 0
                    || activation.versions.difficulty_policy_version == 0
                    || previous_block.is_some_and(|block| activation.block <= block)
                {
                    return Err(ActivationRegistryError::InvalidRecord(format!(
                        "invalid USDB-chain activation binding for network {} at block {}",
                        binding.network_id, activation.block
                    )));
                }
                previous_block = Some(activation.block);
            }
            if binding.activations[0].block != 0 {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "USDB-chain release binding {} does not activate from genesis",
                    binding.network_id
                )));
            }
        }
        Ok(())
    }

    fn validate_embedded_btc_bindings(&self) -> Result<(), ActivationRegistryError> {
        for (network, expected_artifacts) in [
            (
                Network::Bitcoin,
                &["activation-registry/btc-mainnet.json"][..],
            ),
            (
                Network::Regtest,
                &[
                    "activation-registry/btc-regtest.json",
                    "activation-registry/btc-regtest-revision-2.json",
                ][..],
            ),
        ] {
            let catalog = embedded_btc_activation_registry_catalog(network)?;
            if catalog.registry_ids().len() != expected_artifacts.len() {
                return Err(ActivationRegistryError::InvalidRecord(format!(
                    "release manifest artifact list does not cover BTC registry catalog {}",
                    catalog.current_registry().scope.network_id
                )));
            }
            for (index, registry_id) in catalog.registry_ids().iter().enumerate() {
                let registry = catalog.registry_by_id(registry_id)?;
                let revision = u32::try_from(index + 1).map_err(|_| {
                    ActivationRegistryError::InvalidRecord(
                        "BTC registry revision number overflow".to_string(),
                    )
                })?;
                let binding = self
                    .btc_activation_registries
                    .iter()
                    .find(|binding| {
                        binding.network_id == registry.scope.network_id
                            && binding.revision == revision
                    })
                    .ok_or_else(|| {
                        ActivationRegistryError::InvalidRecord(format!(
                            "release manifest is missing BTC registry {} revision {}",
                            registry.scope.network_id, revision
                        ))
                    })?;
                if binding.network_type != registry.scope.network_type
                    || binding.artifact != expected_artifacts[index]
                    || binding.current != (registry_id == catalog.current_registry_id())
                    || binding.activation_registry_id.as_str() != registry_id
                {
                    return Err(ActivationRegistryError::InvalidRecord(format!(
                        "release manifest BTC registry identity mismatch for {} revision {}",
                        registry.scope.network_id, revision
                    )));
                }
            }
        }
        if self.btc_activation_registries.len() != 3 {
            return Err(ActivationRegistryError::InvalidRecord(
                "release manifest contains a BTC registry not embedded by this binary".to_string(),
            ));
        }
        Ok(())
    }
}

/// Structured failures used by startup validation, historical RPC, and validator replay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationRegistryError {
    Parse(String),
    UnsupportedSchemaVersion(String),
    InvalidRecord(String),
    ActivationRecordNotFound(String),
    ActivationRecordConflict(String),
    VersionNotSupported {
        family: VersionFamily,
        value: String,
    },
}

impl Display for ActivationRegistryError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Parse(detail) => write!(f, "failed to parse activation registry: {}", detail),
            Self::UnsupportedSchemaVersion(version) => {
                write!(f, "unsupported activation registry schema {}", version)
            }
            Self::InvalidRecord(detail) => write!(f, "invalid activation record: {}", detail),
            Self::ActivationRecordNotFound(detail) => {
                write!(f, "activation record not found: {}", detail)
            }
            Self::ActivationRecordConflict(detail) => {
                write!(f, "activation record conflict: {}", detail)
            }
            Self::VersionNotSupported { family, value } => {
                write!(f, "version not supported: {}={}", family.as_str(), value)
            }
        }
    }
}

impl std::error::Error for ActivationRegistryError {}

/// Returns the immutable BTC registry revision catalog embedded for a network.
///
/// Testnet, testnet4, and signet deliberately fail closed until their own
/// network-scoped registry artifacts are added and reviewed.
pub fn embedded_btc_activation_registry_catalog(
    network: Network,
) -> Result<&'static BtcActivationRegistryCatalog, ActivationRegistryError> {
    let (cell, json_revisions): (
        &OnceLock<Result<BtcActivationRegistryCatalog, ActivationRegistryError>>,
        &[&str],
    ) = match network {
        Network::Bitcoin => (
            &EMBEDDED_BTC_MAINNET_CATALOG,
            &[EMBEDDED_BTC_MAINNET_REGISTRY_JSON],
        ),
        Network::Regtest => (
            &EMBEDDED_BTC_REGTEST_CATALOG,
            &[
                EMBEDDED_BTC_REGTEST_REGISTRY_JSON,
                EMBEDDED_BTC_REGTEST_REGISTRY_REVISION_2_JSON,
            ],
        ),
        Network::Testnet | Network::Testnet4 | Network::Signet => {
            return Err(ActivationRegistryError::ActivationRecordNotFound(format!(
                "no embedded BTC activation registry for network {}",
                network
            )));
        }
    };
    cell.get_or_init(|| {
        let revisions = json_revisions
            .iter()
            .map(|json| {
                let registry = BtcActivationRegistry::from_json(json)?;
                registry.validate_network(network)?;
                Ok(registry)
            })
            .collect::<Result<Vec<_>, ActivationRegistryError>>()?;
        let current_registry_id = revisions[0].activation_registry_id();
        BtcActivationRegistryCatalog::from_revisions_with_current(revisions, &current_registry_id)
    })
    .as_ref()
    .map_err(Clone::clone)
}

/// Returns the current embedded registry revision for the selected network.
pub fn embedded_btc_activation_registry(
    network: Network,
) -> Result<&'static BtcActivationRegistry, ActivationRegistryError> {
    embedded_btc_activation_registry_catalog(network).map(|catalog| catalog.current_registry())
}

/// Returns the stable-view lag committed by the current registry for `network`.
pub fn embedded_btc_stable_lag_blocks(network: Network) -> Result<u32, ActivationRegistryError> {
    embedded_btc_activation_registry(network).map(BtcActivationRegistry::stable_lag_blocks)
}

/// Returns the audit-only manifest that binds BTC registry IDs to USDB chain configs.
///
/// The manifest never resolves USDB-chain consensus versions. USDB nodes must use
/// their local genesis and `ChainConfig.usdb.activations` for that purpose.
pub fn embedded_cross_chain_release_manifest()
-> Result<&'static CrossChainReleaseManifest, ActivationRegistryError> {
    EMBEDDED_RELEASE_MANIFEST
        .get_or_init(|| {
            let manifest = CrossChainReleaseManifest::from_json(EMBEDDED_RELEASE_MANIFEST_JSON)?;
            manifest.validate_embedded_btc_bindings()?;
            Ok(manifest)
        })
        .as_ref()
        .map_err(Clone::clone)
}

type CanonicalRecordSortKey = (
    VersionFamily,
    u64,
    ActivationStatus,
    String,
    VersionValue,
    Option<VersionValue>,
    String,
);

fn canonical_record_sort_key(record: &BtcActivationRecord) -> CanonicalRecordSortKey {
    (
        record.version_family,
        record.activation_height,
        record.status,
        record.uip.clone(),
        record.version_value.clone(),
        record.supersedes.clone(),
        record.notes.clone(),
    )
}

fn update_string(hasher: &mut Sha256, value: &str) {
    hasher.update((value.len() as u32).to_be_bytes());
    hasher.update(value.as_bytes());
}

fn update_version_value(hasher: &mut Sha256, value: &VersionValue) {
    match value {
        VersionValue::String(value) => {
            hasher.update([0]);
            update_string(hasher, value);
        }
        VersionValue::Integer(value) => {
            hasher.update([1]);
            hasher.update(value.to_be_bytes());
        }
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut output, "{:02x}", byte);
    }
    output
}

fn is_canonical_hex_32(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry(records: Vec<BtcActivationRecord>) -> BtcActivationRegistry {
        BtcActivationRegistry {
            schema_version: ACTIVATION_REGISTRY_SCHEMA_VERSION.to_string(),
            scope: BtcActivationRegistryScope {
                network_type: ActivationNetworkType::Regtest,
                network_id: "btc-regtest".to_string(),
                stable_lag_blocks: 10,
            },
            records,
        }
    }

    fn record(
        family: VersionFamily,
        value: VersionValue,
        height: u64,
        supersedes: Option<VersionValue>,
    ) -> BtcActivationRecord {
        BtcActivationRecord {
            uip: "UIP-0008".to_string(),
            version_family: family,
            version_value: value,
            activation_height: height,
            status: ActivationStatus::Active,
            supersedes,
            notes: "test record".to_string(),
        }
    }

    fn embedded_regtest_with_energy_v2_at(height: u64) -> BtcActivationRegistry {
        let mut registry = embedded_btc_activation_registry(Network::Regtest)
            .unwrap()
            .clone();
        registry.records.push(record(
            VersionFamily::EnergyFormulaVersion,
            VersionValue::String("uip-0003-pass-energy-formula:v2".to_string()),
            height,
            Some(VersionValue::String(ENERGY_FORMULA_VERSION_V1.to_string())),
        ));
        registry
    }

    #[test]
    fn embedded_registries_resolve_supported_btc_versions() {
        for (network, expected_registry_id) in [
            (
                Network::Bitcoin,
                "a6350cd6a68755ea64edf537f35c1eca4421a970e2ecfd67aaa29075aae57224",
            ),
            (
                Network::Regtest,
                "bfd8c7e41ab4035db64e52eb9ea55050c08211c2ae4c2a88d8b2fc17ae1718b0",
            ),
        ] {
            let registry = embedded_btc_activation_registry(network).unwrap();
            let versions = registry.lookup_active_version_set(0).unwrap();
            registry.validate_network(network).unwrap();
            versions.validate_btc_indexer_v1().unwrap();
            assert_eq!(registry.activation_registry_id(), expected_registry_id);
            assert_eq!(registry.stable_lag_blocks(), 10);
            assert_eq!(
                versions.active_version_set_id(),
                "01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691"
            );
        }
        assert_ne!(
            embedded_btc_activation_registry(Network::Bitcoin)
                .unwrap()
                .activation_registry_id(),
            embedded_btc_activation_registry(Network::Regtest)
                .unwrap()
                .activation_registry_id()
        );
    }

    #[test]
    fn registry_catalog_preserves_old_revision_and_selects_current_revision() {
        let previous = embedded_btc_activation_registry(Network::Regtest)
            .unwrap()
            .clone();
        let current = embedded_regtest_with_energy_v2_at(100);
        let previous_id = previous.activation_registry_id();
        let current_id = current.activation_registry_id();

        let catalog =
            BtcActivationRegistryCatalog::from_revisions(vec![previous, current]).unwrap();
        assert_eq!(
            catalog.registry_ids(),
            &[previous_id.clone(), current_id.clone()]
        );
        assert_eq!(catalog.current_registry_id(), current_id);
        assert_eq!(
            catalog
                .registry_by_id(&previous_id)
                .unwrap()
                .lookup_active_version_set(100)
                .unwrap()
                .require_string(VersionFamily::EnergyFormulaVersion)
                .unwrap(),
            ENERGY_FORMULA_VERSION_V1
        );
        assert_eq!(
            catalog
                .current_registry()
                .lookup_active_version_set(100)
                .unwrap()
                .require_string(VersionFamily::EnergyFormulaVersion)
                .unwrap(),
            "uip-0003-pass-energy-formula:v2"
        );
    }

    #[test]
    fn registry_catalog_rejects_revision_that_rewrites_prior_record() {
        let previous = embedded_btc_activation_registry(Network::Regtest)
            .unwrap()
            .clone();
        let mut rewritten = previous.clone();
        rewritten.records[0].notes = "rewritten historical record".to_string();

        assert!(matches!(
            BtcActivationRegistryCatalog::from_revisions(vec![previous, rewritten]),
            Err(ActivationRegistryError::InvalidRecord(detail))
                if detail.contains("rewrites prior records")
        ));
    }

    #[test]
    fn registry_rejects_zero_stable_lag() {
        let mut registry = embedded_btc_activation_registry(Network::Regtest)
            .unwrap()
            .clone();
        registry.scope.stable_lag_blocks = 0;
        assert!(matches!(
            registry.validate(),
            Err(ActivationRegistryError::InvalidRecord(detail))
                if detail.contains("zero stable_lag_blocks")
        ));
    }

    #[test]
    fn registry_catalog_rejects_stable_lag_change_between_revisions() {
        let previous = embedded_btc_activation_registry(Network::Regtest)
            .unwrap()
            .clone();
        let mut changed = embedded_regtest_with_energy_v2_at(100);
        changed.scope.stable_lag_blocks = previous.scope.stable_lag_blocks + 1;

        assert!(matches!(
            BtcActivationRegistryCatalog::from_revisions(vec![previous, changed]),
            Err(ActivationRegistryError::InvalidRecord(detail))
                if detail.contains("mixes scopes")
        ));
    }

    #[test]
    fn registry_id_commits_stable_lag() {
        let registry = embedded_btc_activation_registry(Network::Regtest)
            .unwrap()
            .clone();
        let original_id = registry.activation_registry_id();
        let mut changed = registry;
        changed.scope.stable_lag_blocks += 1;
        assert_ne!(changed.activation_registry_id(), original_id);
    }

    #[test]
    fn duplicate_registry_json_field_is_rejected() {
        let json = format!(
            r#"{{
                "schema_version": "{0}",
                "schema_version": "{0}",
                "scope": {{"network_type":"regtest","network_id":"btc-regtest","stable_lag_blocks":10}},
                "records": []
            }}"#,
            ACTIVATION_REGISTRY_SCHEMA_VERSION
        );
        assert!(matches!(
            BtcActivationRegistry::from_json(&json),
            Err(ActivationRegistryError::Parse(_))
        ));
    }

    #[test]
    fn unknown_registry_json_field_is_rejected() {
        let json = format!(
            r#"{{
                "schema_version": "{}",
                "scope": {{"network_type":"regtest","network_id":"btc-regtest","stable_lag_blocks":10}},
                "records": [],
                "unexpected": true
            }}"#,
            ACTIVATION_REGISTRY_SCHEMA_VERSION
        );
        assert!(matches!(
            BtcActivationRegistry::from_json(&json),
            Err(ActivationRegistryError::Parse(_))
        ));
    }

    #[test]
    fn public_registry_rejects_manual_activation_override() {
        let json = format!(
            r#"{{
                "schema_version": "{}",
                "scope": {{"network_type":"mainnet","network_id":"btc-mainnet","stable_lag_blocks":10}},
                "records": [{{
                    "uip":"UIP-0003",
                    "version_family":"energy_formula_version",
                    "version_value":"{}",
                    "activation_height":0,
                    "activation_anchor":"manual",
                    "status":"Active",
                    "supersedes":null,
                    "notes":"invalid public manual override"
                }}]
            }}"#,
            ACTIVATION_REGISTRY_SCHEMA_VERSION, ENERGY_FORMULA_VERSION_V1
        );
        match BtcActivationRegistry::from_json(&json) {
            Err(ActivationRegistryError::Parse(message)) => {
                assert!(message.contains("activation_anchor"));
            }
            result => panic!("expected public manual override to fail parsing: {result:?}"),
        }
    }

    #[test]
    fn btc_indexer_rejects_extra_active_family() {
        let registry = embedded_btc_activation_registry(Network::Regtest).unwrap();
        let mut versions = registry.lookup_active_version_set(0).unwrap();
        versions
            .0
            .insert(VersionFamily::PayloadVersion, VersionValue::Integer(1));

        assert!(matches!(
            versions.validate_btc_indexer_v1(),
            Err(ActivationRegistryError::VersionNotSupported {
                family: VersionFamily::PayloadVersion,
                ..
            })
        ));
    }

    #[test]
    fn lookup_uses_historical_activation_boundary() {
        let v1 = VersionValue::String("v1".to_string());
        let v2 = VersionValue::String("v2".to_string());
        let registry = registry(vec![
            record(VersionFamily::EnergyFormulaVersion, v1.clone(), 0, None),
            record(
                VersionFamily::EnergyFormulaVersion,
                v2.clone(),
                100,
                Some(v1.clone()),
            ),
        ]);
        registry.validate().unwrap();
        let before = registry.lookup_active_version_set(99).unwrap();
        let at = registry.lookup_active_version_set(100).unwrap();
        let after = registry.lookup_active_version_set(101).unwrap();
        assert_eq!(before.get(VersionFamily::EnergyFormulaVersion), Some(&v1));
        assert_eq!(at.get(VersionFamily::EnergyFormulaVersion), Some(&v2));
        assert_eq!(after.get(VersionFamily::EnergyFormulaVersion), Some(&v2));
    }

    #[test]
    fn unsupported_formula_version_fails_closed_at_activation_boundary() {
        let registry = embedded_regtest_with_energy_v2_at(100);
        registry.validate().unwrap();

        registry
            .lookup_active_version_set(99)
            .unwrap()
            .validate_btc_indexer_v1()
            .unwrap();
        assert!(matches!(
            registry
                .lookup_active_version_set(100)
                .unwrap()
                .validate_btc_indexer_v1(),
            Err(ActivationRegistryError::VersionNotSupported {
                family: VersionFamily::EnergyFormulaVersion,
                ..
            })
        ));
    }

    #[test]
    fn cross_activation_reorg_and_restart_preserve_version_and_commit_identity() {
        let registry = embedded_regtest_with_energy_v2_at(100);
        registry.validate().unwrap();
        let encoded = serde_json::to_string(&registry).unwrap();
        let reloaded = BtcActivationRegistry::from_json(&encoded).unwrap();

        let before_id = reloaded
            .lookup_active_version_set(99)
            .unwrap()
            .active_version_set_id();
        let at_id = reloaded
            .lookup_active_version_set(100)
            .unwrap()
            .active_version_set_id();
        let after_id = reloaded
            .lookup_active_version_set(101)
            .unwrap()
            .active_version_set_id();
        assert_ne!(before_id, at_id);
        assert_eq!(after_id, at_id);

        let rollback_id = reloaded
            .lookup_active_version_set(99)
            .unwrap()
            .active_version_set_id();
        let replay_id = reloaded
            .lookup_active_version_set(100)
            .unwrap()
            .active_version_set_id();
        assert_eq!(rollback_id, before_id);
        assert_eq!(replay_id, at_id);

        let mut identity = crate::LocalStateCommitIdentity {
            commit_protocol_version: COMMIT_PROTOCOL_VERSION_V1.to_string(),
            upstream_snapshot_id: "11".repeat(32),
            active_version_set_id: before_id,
            local_synced_block_height: 100,
            latest_pass_block_commit: None,
            latest_active_balance_snapshot: None,
        };
        let before_commit = crate::build_local_state_commit(&identity);
        identity.active_version_set_id = at_id;
        let activated_commit = crate::build_local_state_commit(&identity);
        assert_ne!(before_commit, activated_commit);
    }

    #[test]
    fn duplicate_active_height_is_rejected() {
        let registry = registry(vec![
            record(
                VersionFamily::EnergyFormulaVersion,
                VersionValue::String("v1".to_string()),
                0,
                None,
            ),
            record(
                VersionFamily::EnergyFormulaVersion,
                VersionValue::String("v2".to_string()),
                0,
                Some(VersionValue::String("v1".to_string())),
            ),
        ]);
        assert!(matches!(
            registry.validate(),
            Err(ActivationRegistryError::ActivationRecordConflict(_))
        ));
    }

    #[test]
    fn registry_scope_must_match_selected_network() {
        let registry = registry(vec![record(
            VersionFamily::EnergyFormulaVersion,
            VersionValue::String("v1".to_string()),
            0,
            None,
        )]);
        assert!(matches!(
            registry.validate_network(Network::Bitcoin),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));
    }

    #[test]
    fn btc_registry_rejects_non_bitcoin_network_classes() {
        for network_type in [ActivationNetworkType::Devnet, ActivationNetworkType::Local] {
            let mut registry = registry(vec![record(
                VersionFamily::EnergyFormulaVersion,
                VersionValue::String("v1".to_string()),
                0,
                None,
            )]);
            registry.scope.network_type = network_type;
            assert!(matches!(
                registry.validate(),
                Err(ActivationRegistryError::InvalidRecord(_))
            ));
        }
    }

    #[test]
    fn planned_record_does_not_activate_a_version() {
        let mut planned = record(
            VersionFamily::EnergyFormulaVersion,
            VersionValue::String("v2".to_string()),
            0,
            None,
        );
        planned.status = ActivationStatus::Planned;
        let registry = registry(vec![planned]);
        registry.validate().unwrap();

        assert!(matches!(
            registry.lookup_active_version_set(0),
            Err(ActivationRegistryError::ActivationRecordNotFound(_))
        ));
    }

    #[test]
    fn version_family_value_type_is_enforced() {
        let registry = registry(vec![record(
            VersionFamily::EnergyFormulaVersion,
            VersionValue::Integer(1),
            0,
            None,
        )]);
        assert!(matches!(
            registry.validate(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));
    }

    #[test]
    fn activation_height_must_fit_payload_btc_height() {
        let registry = registry(vec![record(
            VersionFamily::EnergyFormulaVersion,
            VersionValue::String("v1".to_string()),
            u64::from(u32::MAX) + 1,
            None,
        )]);
        assert!(matches!(
            registry.validate(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));
    }

    #[test]
    fn usdb_chain_family_is_rejected_by_btc_registry() {
        let registry = registry(vec![record(
            VersionFamily::PayloadVersion,
            VersionValue::Integer(1),
            0,
            None,
        )]);
        assert!(matches!(
            registry.validate(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));
    }

    #[test]
    fn networks_without_an_embedded_registry_fail_closed() {
        for network in [Network::Testnet, Network::Testnet4, Network::Signet] {
            assert!(
                !BtcActivationRegistryScope::network_identity(network)
                    .1
                    .is_empty()
            );
            assert!(matches!(
                embedded_btc_activation_registry(network),
                Err(ActivationRegistryError::ActivationRecordNotFound(_))
            ));
        }
    }

    #[test]
    fn registry_id_is_independent_of_json_record_order() {
        let first = record(
            VersionFamily::EnergyFormulaVersion,
            VersionValue::String("v1".to_string()),
            0,
            None,
        );
        let second = record(
            VersionFamily::LevelFormulaVersion,
            VersionValue::String("v1".to_string()),
            0,
            None,
        );
        let forward = registry(vec![first.clone(), second.clone()]);
        let reverse = registry(vec![second, first]);
        forward.validate().unwrap();
        reverse.validate().unwrap();
        assert_eq!(
            forward.activation_registry_id(),
            reverse.activation_registry_id()
        );
    }

    #[test]
    fn registry_id_is_scoped_to_one_btc_network() {
        let records = vec![record(
            VersionFamily::EnergyFormulaVersion,
            VersionValue::String("v1".to_string()),
            0,
            None,
        )];
        let regtest = registry(records.clone());
        let mut mainnet = registry(records);
        mainnet.scope = BtcActivationRegistryScope {
            network_type: ActivationNetworkType::Mainnet,
            network_id: "btc-mainnet".to_string(),
            stable_lag_blocks: 10,
        };
        assert_ne!(
            regtest.activation_registry_id(),
            mainnet.activation_registry_id()
        );
    }

    #[test]
    fn release_manifest_matches_embedded_btc_registries() {
        let manifest = embedded_cross_chain_release_manifest().unwrap();
        assert_eq!(manifest.btc_activation_registries.len(), 3);
        assert_eq!(manifest.usdb_chain_configs.len(), 1);
        let usdb_chain = &manifest.usdb_chain_configs[0];
        assert_eq!(usdb_chain.chain_id, 20_260_323);
        assert_eq!(
            usdb_chain.genesis_hash,
            "2ddb7bab1cf85a02d71048927f42a99a8412d937d87b792c73949d967754d9ae"
        );
        assert_eq!(
            usdb_chain.activation_authority,
            "chain_config.usdb.activations"
        );
        assert_eq!(usdb_chain.activations.len(), 1);
        assert_eq!(usdb_chain.activations[0].block, 0);
        assert_eq!(usdb_chain.activations[0].btc_anchor_max_age_blocks, 6_650);
        assert_eq!(usdb_chain.activations[0].versions.payload_version, 1);
        assert_eq!(
            usdb_chain.activations[0].versions.btc_anchor_policy_version,
            1
        );
        assert_eq!(
            usdb_chain.activations[0].btc_activation_registry_id,
            embedded_btc_activation_registry(Network::Regtest)
                .unwrap()
                .activation_registry_id()
        );
    }

    #[test]
    fn release_manifest_rejects_tampered_authority_and_registry_id() {
        let manifest =
            CrossChainReleaseManifest::from_json(EMBEDDED_RELEASE_MANIFEST_JSON).unwrap();

        let mut invalid_authority = manifest.clone();
        invalid_authority.usdb_chain_configs[0].activation_authority = "rpc".to_string();
        assert!(matches!(
            invalid_authority.validate(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));

        let mut invalid_registry_id = manifest;
        invalid_registry_id.btc_activation_registries[0].activation_registry_id =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        assert!(matches!(
            invalid_registry_id.validate_embedded_btc_bindings(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));

        let mut invalid_chain_activation =
            CrossChainReleaseManifest::from_json(EMBEDDED_RELEASE_MANIFEST_JSON).unwrap();
        invalid_chain_activation.usdb_chain_configs[0].activations[0].btc_activation_registry_id =
            "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".to_string();
        assert!(matches!(
            invalid_chain_activation.validate(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));

        let mut invalid_anchor_age =
            CrossChainReleaseManifest::from_json(EMBEDDED_RELEASE_MANIFEST_JSON).unwrap();
        invalid_anchor_age.usdb_chain_configs[0].activations[0].btc_anchor_max_age_blocks = 0;
        assert!(matches!(
            invalid_anchor_age.validate(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));

        let mut invalid_versions =
            CrossChainReleaseManifest::from_json(EMBEDDED_RELEASE_MANIFEST_JSON).unwrap();
        invalid_versions.usdb_chain_configs[0].activations[0]
            .versions
            .btc_anchor_policy_version = 0;
        assert!(matches!(
            invalid_versions.validate(),
            Err(ActivationRegistryError::InvalidRecord(_))
        ));
    }
}
