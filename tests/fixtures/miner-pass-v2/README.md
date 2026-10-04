# Isolated MinerPass v2 development catalogs

## Mint fixture version policy

Generic Rust mint/pass constructors use `usdb_util::MINER_PASS_MINT_SCHEMA_VERSION`.
Workflow tests keep that default unless the version itself is under test. Payload
parser contracts, invalid-version cases, historical records, canonical mutation
inputs and catalog/golden identities keep explicit versions; do not replace them
with a mutable current-version constant. This ensures that changing a default
cannot silently change both the implementation and its protocol oracle.

The shell/Python runners remain explicitly paired with the pinned catalog below;
a future schema change must review their payloads and catalog together, rather
than infer a version by parsing Rust source.

## Pinned catalogs

`catalog.json` pins `btc-regtest / miner-pass-v2-fixture`, stable lag 10, and
inscription schema v1 with independent state-machine v2 from BTC height 0. Every other
version family retains its existing value. `catalog-staged.json` adds a Planned
formula record as a second revision without activating another formula.

Rust tests explicitly select these files in temporary roots. Go embeds the
staged vector as an explicitly selected development scope; no default chain
configuration, old catalog identity, or deployment bundle selects it. Legacy
catalog metadata remains hashable, but neither Rust nor Go executes the legacy state machine v1.
A development network using the new binary needs a fresh dataset and a V2
catalog pinned by scope and ID. Public network parameters remain a release task.

Regenerate or check the paired Go vector from the USDB repository:

```bash
cargo run --manifest-path src/btc/Cargo.toml -p usdb-util \
  --bin generate_go_btc_activation_golden -- \
  --catalog tests/fixtures/miner-pass-v2/catalog-staged.json \
  ../go-ethereum/internal/usdb/testdata/miner_pass_v2_activation_golden.json --check
```

Remove `--check` only when intentionally regenerating reviewed fixture changes.
Production block tests use isolated HTTP Core/BH fixtures and temporary real
SQLite/RocksDB stores. These fixtures do not qualify a live Core/AssumeUTXO
bootstrap, wallet workflow or public deployment.

The shared regtest library and Geth profile runner now select this catalog
explicitly in generated test configurations. Run
`python3 tests/run_miner_pass_v2_profile_live.py --ord-bin /path/to/ord`
from the USDB root for the minimal live-service flow. See
[acceptance scope and artifacts](../../../doc/usdb-indexer/miner-pass-v2-live-acceptance.md).
The protocol, reorg, historical/validator, and world-sim matrices also use the
V2 fixture. See [matrix migration and evidence](../../../doc/usdb-indexer/miner-pass-v2-matrix-acceptance.md).
Migration and short local runs do not qualify the full nightly/weekly matrix.
