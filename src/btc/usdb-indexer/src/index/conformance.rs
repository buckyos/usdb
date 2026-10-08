// Synthetic contracts shared by unit tests and explicitly opted-in regtest services.
/// Private rules scope shared by the Rust and Go acceptance catalogs.
pub const CONFORMANCE_SCOPE: &str = "miner-pass-upgrade-conformance";
/// Synthetic JSON version, deliberately distinct from future production schema numbers.
pub const CONFORMANCE_SCHEMA: &str = "conformance-miner-pass-schema:901";
/// Independent nested-binding grammar used to exercise a second schema transition.
pub const CONFORMANCE_SCHEMA_STRUCTURED: &str = "conformance-miner-pass-schema:902";
/// Admission-only rule that preserves old collabs while rejecting new ones.
pub const CONFORMANCE_STATE: &str = "conformance-miner-pass-state:no-new-collab";

/// Non-identity boundary conversion followed by double-rate raw growth.
pub const CONFORMANCE_ENERGY_DOUBLE: &str = "conformance-energy:double";
/// Identity boundary conversion followed by triple-rate raw growth.
pub const CONFORMANCE_ENERGY_TRIPLE: &str = "conformance-energy:triple";

/// Independently activated quarter-weight collaboration contribution.
pub const CONFORMANCE_EFFECTIVE: &str = "conformance-effective:quarter-collab";
/// Independently activated level increments at each thousand energy units.
pub const CONFORMANCE_LEVEL: &str = "conformance-level:thousands";
