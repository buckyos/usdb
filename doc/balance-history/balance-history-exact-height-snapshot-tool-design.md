# Balance-History Exact-Height Snapshot Tool Design

## 1. Purpose

This document defines the first implementation of a restartable tool that builds a complete
balance-history checkpoint at one exact BTC height. The tool reuses the `balance-history`
library and does not orchestrate a long-running balance-history service subprocess.

The produced checkpoint is intended for installation by another balance-history node and must
allow that node to continue indexing correctly. It is therefore not a historical balance-only
export.

## 2. Checkpoint invariant

A completed checkpoint at target height `H` must satisfy:

```text
durable balance-history height
    == balance state height
    == UTXO state height
    == latest block commit height
    == H
```

The checkpoint must contain every unspent UTXO in the builder workspace at `H`. A checkpoint
without the complete UTXO set is invalid and must not be published or installed as a resumable
checkpoint.

The BTC identity of a checkpoint is not height alone. Its immutable identity is:

```text
network + height + BTC block hash + consensus snapshot ID
```

This keeps artifacts from different same-height BTC branches distinct.

## 3. Storage layout

One builder root owns one mutable balance-history workspace and serializes all build jobs:

```text
<builder-root>/
|-- builder-state.json
|-- workspace/
|   |-- config.toml
|   `-- db/
|-- jobs/
|   `-- <height>/
|       `-- job.json
|-- snapshots/
|   `-- <height>/
|       `-- <btc-block-hash>/
|           |-- snapshot_<height>.db
|           |-- snapshot_<height>.manifest.json
|           |-- snapshot_<height>.manifest.sig   # optional
|           `-- complete.json
`-- tmp/
    `-- <height>-<job-id>/
```

The mutable RocksDB workspace is shared by monotonically increasing targets. Per-height job and
artifact directories are independent and immutable after completion. The design deliberately
does not copy a complete RocksDB directory for every height.

## 4. Persistent state

`builder-state.json` coordinates the single mutable workspace:

- format version and BTC network;
- latest completed checkpoint identity;
- active target height, if any.

`jobs/<height>/job.json` records one target's recoverable phase:

```text
Prepare -> Syncing -> Sealed -> Building -> Verifying -> Complete
```

The RocksDB durable height remains the source of truth for sync progress. JSON state is always
cross-checked against RocksDB and the published manifest after restart.

`complete.json` is the final commit marker for one artifact directory. It is written only after
the snapshot DB, manifest, optional signature, counts, state reference, file hash, and canonical
BTC block hash have all been verified. Only a job with a valid `complete.json` may advance the
builder's latest-completed pointer.

State files are written through a temporary file followed by an atomic rename. Snapshot files
are built in a temporary directory on the same filesystem and the whole directory is renamed to
its final block-hash path after verification.

## 5. Create workflow

For `create --height H` the tool:

1. acquires an exclusive lock scoped to the builder root;
2. loads or creates the job for `H` and rejects a different unfinished active target;
3. opens the workspace with `max_sync_block_height = H`;
4. resumes indexing until the durable height is exactly `H`;
5. flushes RocksDB and seals the block hash, block commit, and state reference at `H`;
6. builds a full snapshot including all live UTXOs in a temporary artifact directory;
7. reopens and verifies the generated SQLite DB, metadata counts, manifest hash, state reference,
   and optional signature metadata;
8. atomically publishes the artifact directory and writes `complete.json`;
9. updates the builder state and clears the active job.

The tool never advances the workspace beyond `H` before the checkpoint for `H` is complete.
After completion, a later target can incrementally continue from the same workspace.

## 6. Restart and idempotency

Re-running `create --height H` is the resume operation:

- workspace height below `H`: continue indexing;
- workspace height equal to `H`: skip sync and continue build or verification;
- workspace height above `H`: fail closed;
- interrupted temporary artifact: remove it and rebuild from the sealed workspace;
- valid completed artifact for the same height and block hash: return it idempotently;
- unfinished job for a different height: reject the new request.

The first version restarts snapshot export from the beginning instead of persisting SQLite
row-level progress. Sync progress is preserved in RocksDB, which is the expensive part of a new
builder.

If an unfinished job must be abandoned after its workspace has advanced beyond the latest
completed checkpoint, recovery must restore the latest completed snapshot into the workspace.
The first version does not silently retarget or roll back an active job.

## 7. Reorg handling

The normal balance-history reorg reconciliation runs while syncing. Before sealing and before
publishing, the tool must verify that the workspace block hash at `H` matches the BTC node's
canonical block hash at `H` and an optional operator-provided expected block hash.

If a previously completed height is later replaced by a BTC reorg, its artifact is retained for
audit but is no longer a canonical base for a new job. The builder must reconcile its workspace
and create a new artifact under the replacement block hash before advancing from that height.

The configured stable lag is an indexing boundary, not BTC finality. Public release operations
may require a larger explicit confirmation depth and a pinned expected block hash.

## 8. First-version CLI

```text
balance-history-snapshot-tool create --height <H> [--expected-block-hash <HASH>]
balance-history-snapshot-tool status [--height <H>]
balance-history-snapshot-tool list
balance-history-snapshot-tool verify --height <H> [--block-hash <HASH>]
```

For example, initialize a dedicated builder and then incrementally create the next height:

```bash
cargo run --manifest-path src/btc/Cargo.toml -p balance-history-snapshot-tool -- \
  --root-dir /data/balance-history-snapshot-builder \
  create --height 840000 --expected-block-hash <HASH_840000> \
  --config /etc/usdb/balance-history/config.toml

cargo run --manifest-path src/btc/Cargo.toml -p balance-history-snapshot-tool -- \
  --root-dir /data/balance-history-snapshot-builder \
  create --height 840001 --expected-block-hash <HASH_840001>
```

Common options select the builder root and may request JSON output. The builder root contains the
balance-history configuration used by the workspace. There is one active job per builder root;
parallel targets require separate roots.

The first `create` for a new builder root must pass `--config <balance-history-config.toml>`.
Later create/resume operations reuse the atomically copied workspace config and reject a different
config file. Height `0` is rejected because the current balance-history block-commit history starts
at height `1`. In `--json` mode stdout contains only the command result; operational logs remain in
the builder root's log directory.

## 9. Required validation

The first implementation must cover:

- initial sync and complete full-UTXO checkpoint creation;
- incremental `H -> H+1` creation from the shared workspace;
- process restart while syncing and after sealing;
- interrupted partial artifact cleanup and rebuild;
- idempotent replay of a completed request;
- rejection when workspace height is above target;
- rejection of a conflicting active target;
- expected block hash mismatch;
- same-height reorg before publication;
- generated DB integrity and exact metadata counts;
- install followed by spending a UTXO created before the snapshot height;
- manifest/signature and atomic-publication failure paths.

## 10. Deferred work

The first version does not include:

- multiple concurrent jobs sharing one workspace;
- SQLite row- or shard-level export resume;
- automatic rollback to an older requested target;
- automatic deletion of superseded branch artifacts;
- a public release finality policy.

## 11. First-version implementation

The first implementation is the `balance-history-snapshot-tool` Rust workspace crate. It directly
uses the `balance-history` library for exact-height synchronization and snapshot export. Its
`create`, `status`, `list`, and `verify` commands persist the state machine and expose JSON output
for automation.

The deterministic regtest entrypoints are:

```text
src/btc/balance-history/scripts/regtest_exact_height_snapshot_tool.sh
src/btc/balance-history/scripts/regtest_exact_height_snapshot_restart.sh
src/btc/balance-history/scripts/regtest_exact_height_snapshot_same_height_reorg.sh
src/btc/balance-history/scripts/regtest_exact_height_snapshot_install_spend.sh
```

Together they cover initial full-UTXO publication, completed-request replay, artifact
verification, incremental `H -> H+1` creation, and cross-process resume after every durable
builder transition (`syncing`, `sealed`, `building`, `verifying`, `published`, and
`job_complete`). They also retain and verify both artifacts across a same-height BTC branch
replacement, require an explicit block hash when one height is ambiguous, continue from the
replacement branch at `H+1`, and install a generated checkpoint before spending an output that
predates the checkpoint.

The legacy snapshot install/recovery scripts also use manifest verification and advance each
transaction height into the service's configured stable view before asserting indexed state.
Remaining follow-up coverage is production-scale export/install performance, disk and atomic
publication failure injection, and optional signer/signature failure paths.
