# Isolated MinerPass v2 development catalogs

`catalog.json` pins `btc-regtest / miner-pass-v2-fixture`, stable lag 10, and
paired inscription-schema/state-machine v2 from BTC height 0. Every other
version family retains its existing value. `catalog-staged.json` adds a Planned
formula record as a second revision without activating another formula.

Rust tests explicitly select these files in temporary roots. Go embeds the
staged vector as an explicitly selected development scope; no default chain
configuration, old catalog identity, or deployment bundle selects it. Legacy
catalog metadata remains hashable, but neither Rust nor Go executes V1 rules.
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
