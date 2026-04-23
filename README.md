# USDB

[English](./README.md) | [中文](./README.zh.md)

USDB is not a standalone "indexing and query" project. It is the **BTC-side infrastructure repository for a BTC-staked, miner-certificate inscription based dual-chain system**.

Its job is to turn Bitcoin blocks, balance history, inscription state, and miner certificate (`pass`) derived state into a set of **replayable, auditable external state services** that can be consumed by ETHW miners and validators.

At the system level, this repository exists to answer a harder question than "can we query the data":

- What exactly was the BTC-side external state at height `H`?
- Can that historical state still be replayed after reorg, restart, or crash?
- If an ETHW validator receives `(height, state ref, pass info)` in a block body, can it replay and verify that payload later under historical semantics?

In other words, this repository holds the most critical BTC-side foundation of the overall system:

- `balance-history`: address balance history, stable height/hash, and block-commit views
- `usdb-indexer`: pass state, active balance snapshots, energy, candidate sets, and validator-facing derived state
- a systematic test framework around reorg, restart, historical replay, validator payloads, and world simulation

These capabilities ultimately serve ETHW block production, candidate selection, and block validation.

For broader background and the full dual-chain integration target, see:
- [USDB Dual-Chain Consensus Integration Risks and Upgrade Checklist](./doc/usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
- [BTC Consensus RPC Error Contract Design](./doc/btc-consensus-rpc-error-contract-design.md)

## Why This Exists

If the upper ETHW system uses BTC-side miner-certificate state as external input, for example:

- miner eligibility
- pass state
- historical energy
- candidate-set and winner selection results

then "the data is queryable" is not enough. The system must also solve:

- stable anchors under reorg
- historical state replay
- cross-service snapshot / system-state identity
- the distinction between `rpc_alive` and `consensus_ready`
- a unified error model and historical validation semantics

This repository is where those concerns are engineered into a coherent BTC-side foundation before that state is exposed to ETHW.

## Repository Layout

```text
.
├── doc/                     # design, protocol, testing, and risk docs
│   ├── balance-history/
│   └── usdb-indexer/
├── src/
│   └── btc/
│       ├── balance-history/       # BTC address balance history service
│       ├── balance-history-cli/   # related CLI
│       ├── usdb-indexer/          # USDB derived-state indexer
│       ├── usdb-indexer-cli/      # related CLI
│       └── usdb-util/             # shared types, versions, commit/hash helpers
└── web/
    ├── balance-history-browser/
    └── usdb-indexer-browser/
```

The BTC Rust workspace lives at [src/btc/Cargo.toml](./src/btc/Cargo.toml). Current workspace members include:

- `balance-history`
- `balance-history-cli`
- `usdb-indexer`
- `usdb-indexer-cli`
- `usdb-util`

## Core Components

### `balance-history`

Responsibilities:

- build an address balance history view from `bitcoind`
- expose stable height, block hash, and block-level commit information
- provide historical `state ref` and auditable snapshot information to downstream consumers
- serve as the BTC balance fact layer for miner-certificate derived state

Docs:
- [Balance-History Docs](./doc/balance-history/README.md)

### `usdb-indexer`

Responsibilities:

- build the miner-certificate / pass state machine on top of BTC data and `balance-history`
- maintain historical `state ref`, local state commit, and system state id
- expose active balance snapshots, energy, leaderboard, and historical replay queries
- provide stable interfaces for ETHW miners, candidate-set selection logic, and validators

Docs:
- [USDB-Indexer Docs](./doc/usdb-indexer/README.md)

### Testing / Simulation

One of the strongest parts of this repository today is its testing infrastructure. It already covers:

- smoke / e2e
- reorg / same-height reorg / multi-block reorg
- restart / pending recovery / crash consistency
- historical state-ref replay
- validator block body / candidate set / version mismatch / upgrade path
- world-sim / deterministic reorg / determinism / soak

If you want to understand the project through its test system, start with:

- [USDB-Indexer Regtest Framework](./doc/usdb-indexer/usdb-indexer-regtest-framework.md)
- [USDB-Indexer Next-Stage Combined Test Plan](./doc/usdb-indexer/usdb-indexer-next-stage-combined-test-plan.md)
- [USDB-Indexer Validator Block-Body E2E Design](./doc/usdb-indexer/usdb-indexer-validator-block-body-e2e-design.md)

## Current State

At the current stage, the repository can be understood as:

- **a fairly complete BTC-side miner-certificate foundation**
  - snapshot identity
  - local state commit
  - system state id
  - readiness contract
  - shared consensus RPC errors
  - historical state-ref replay
- **a working validator-style replay stack for ETHW consumers**
  - single pass
  - multi-pass / candidate set
  - version mismatch / upgrade
  - restart / crash / not-ready window
  - world-sim sampled validation
- **real prune / retention floor** is still a future evolution
  - `STATE_NOT_RETAINED` is currently based on a simplified `genesis_block_height` model

## Getting Started

### 1. Read the docs

Suggested reading order:

1. [USDB Dual-Chain Consensus Integration Risks and Upgrade Checklist](./doc/usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
2. [BTC Consensus RPC Error Contract Design](./doc/btc-consensus-rpc-error-contract-design.md)
3. [USDB-Indexer Docs](./doc/usdb-indexer/README.md)
4. [Balance-History Docs](./doc/balance-history/README.md)

### 2. Run Rust tests

```bash
cargo test --manifest-path src/btc/Cargo.toml -p balance-history
cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer
```

### 3. Run focused regtests

For example, the focused `usdb-indexer` regression suite:

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/run_reorg_regression.sh
```

### 4. Run world-sim

```bash
bash src/btc/usdb-indexer/scripts/regtest_world_sim.sh
```

Or run the heavier validator candidate-set live entry:

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/run_live_validator_candidate_set.sh
```

## Design Themes

The design themes running through the repository are:

- **historical state must be replayable**
  - answer "which BTC external state should ETHW consume at height H?"
- **service alive does not mean consensus-ready**
  - distinguish `rpc_alive` from `consensus_ready`
- **errors must be structured**
  - `HEIGHT_NOT_SYNCED`
  - `SNAPSHOT_NOT_READY`
  - `*_MISMATCH`
  - `HISTORY_NOT_AVAILABLE`
  - `STATE_NOT_RETAINED`
- **reorg is a first-class design input**
  - not an exception path
- **tests are used to converge protocol semantics early**
  - regtest and world-sim are used to harden pass state, historical replay, validator payloads, and other edge cases before upper-layer integration

## Documentation Index

- overall design and risks
  - [USDB Dual-Chain Consensus Integration Risks and Upgrade Checklist](./doc/usdb-%E5%8F%8C%E9%93%BE%E5%85%B1%E8%AF%86%E6%8E%A5%E5%85%A5%E9%97%AE%E9%A2%98%E9%A3%8E%E9%99%A9%E4%B8%8E%E6%94%B9%E9%80%A0%E6%B8%85%E5%8D%95.md)
  - [BTC Consensus RPC Error Contract Design](./doc/btc-consensus-rpc-error-contract-design.md)
- BTC mint and ord runtime
  - [USDB BTC Mint Runtime Profiles](./doc/usdb-btc-mint-runtime-profiles.md)
  - [USDB BTC ord Roles and Mint Flow Memo](./doc/usdb-btc-ord-roles-and-mint-flow.md)
- subsystem indexes
  - [USDB-Indexer Docs](./doc/usdb-indexer/README.md)
  - [Balance-History Docs](./doc/balance-history/README.md)
- key testing and planning docs
  - [USDB-Indexer Regtest Framework](./doc/usdb-indexer/usdb-indexer-regtest-framework.md)
  - [USDB-Indexer Next-Stage Combined Test Plan](./doc/usdb-indexer/usdb-indexer-next-stage-combined-test-plan.md)
  - [USDB-Indexer Validator Block-Body E2E Design](./doc/usdb-indexer/usdb-indexer-validator-block-body-e2e-design.md)

## Status

The project is still under active development. At this stage it is better understood as:

- an architecture and protocol validation repository
- the BTC-side infrastructure layer of a BTC-staked miner-certificate system
- an engineering testbed for consensus-oriented BTC external state
- a high-intensity regtest / world-sim regression foundation

rather than a fully frozen, long-term compatibility product repository.
