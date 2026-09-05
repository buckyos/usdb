# Balance History JSON-RPC Documentation

## Overview

`balance-history` tracks per-address balance changes (`delta`) and resulting balances (`balance`) across block heights, and exposes query APIs via JSON-RPC.

Implementation review findings and the remediation tracker are documented in [balance-history-review-remediation-plan.md](./balance-history-review-remediation-plan.md).

- Default endpoint: `http://127.0.0.1:28010`
- Transport: HTTP + JSON-RPC 2.0
- CORS policy: `AllowAny`

## Data Models

### AddressBalance

```json
{
  "block_height": 123456,
  "balance": 100000,
  "delta": 5000
}
```

- `block_height`: block height
- `balance`: resulting balance at that height (satoshi)
- `delta`: balance change at that height (satoshi, can be negative)

### SyncStatus

```json
{
  "phase": "Indexing",
  "current": 800000,
  "total": 900000,
  "message": "Synced up to block height 800000"
}
```

- `phase`: one of `Initializing` / `Loading` / `Indexing` / `Synced`
- `current`: current progress
- `total`: total progress
- `message`: optional status message

## Common Request Format

```json
{
  "jsonrpc": "2.0",
  "method": "<method_name>",
  "params": [ ... ],
  "id": 1
}
```

## RPC Methods

### 1) `get_network_type`

Returns current BTC network type (for example `mainnet`, `testnet`, `signet`, `regtest`).

### 2) `get_block_height`

Returns the latest synced BTC height stored in database.

### 3) `get_sync_status`

Returns current sync status.

### 4) `get_readiness`

Returns structured readiness state for:

1. plain RPC liveness;
2. ordinary query serving;
3. strict downstream consensus use.

Downstream callers should gate on `consensus_ready=true` instead of treating
`get_network_type` reachability as readiness.

`query_ready=false` is enforced by the server, not merely advisory. Ordinary
DB-backed balance, history, UTXO, block-commit, snapshot/state-ref, and script
registry queries fail closed with `SNAPSHOT_NOT_READY`. Liveness, status,
readiness, snapshot provenance, and shutdown endpoints remain available for
diagnostics and recovery.

The response also exposes `balance_query_floor` and `history_query_floor`, with
the same semantics as `get_snapshot_info` below.

The response also includes `script_registry`, a display-only diagnostic summary:

```json
{
  "script_registry": {
    "state": "ready",
    "coverage_mode": "snapshot_plus_sidecar",
    "capabilities": {
      "script_registry_lookup": true,
      "script_registry_complete_coverage": true
    },
    "overlay_estimated_count": 123456,
    "base_height": 963800,
    "base_block_hash": "....",
    "core_snapshot_id": "....",
    "registry_artifact_id": "....",
    "expected_count": 1541365559,
    "policy": "auxiliary_seen_scripts_non_consensus_v1",
    "last_error": null
  }
}
```

- `state`: lifecycle of the optional sidecar. Failed/conflict states only reduce
  historical reverse-lookup coverage and never block core readiness.
- `coverage_mode`: `full_replay`, `snapshot_plus_sidecar`, or `post_snapshot_only`.
- `capabilities.script_registry_complete_coverage`: whether a miss is definitive.
- `overlay_estimated_count`: estimated RocksDB overlay rows, not an exact count.
- base, core snapshot, artifact, and expected-count fields identify the active immutable sidecar.
- `policy`: machine-readable semantics. The current policy means the registry is a non-consensus seen-script cache populated by indexing and snapshot import.

### 5) `get_snapshot_info`

Returns metadata for the current stable snapshot.

Example result:

```json
{
  "stable_height": 812345,
  "stable_block_hash": "000000...",
  "latest_block_commit": "4f7c...",
  "balance_query_floor": 800000,
  "history_query_floor": 800001,
  "stable_lag": 10,
  "balance_history_api_version": "1.0.0",
  "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
  "commit_protocol_version": "1.0.0",
  "commit_hash_algo": "sha256"
}
```

When the stable snapshot is not yet complete, this method now returns the
shared consensus error `SNAPSHOT_NOT_READY` with structured JSON `data`.

- `balance_query_floor` is the earliest complete at-or-before point-balance height.
- `history_query_floor` is the earliest complete exact-delta/history-range height.
- A genesis-synced node reports both as `0`. A node installed from a compact
  snapshot at height `H` reports `H` and `H + 1` respectively.
- Pre-snapshot block commits remain available for audit, but do not claim that
  the corresponding balance state is retained.

### 6) `get_address_balance`

Queries balance history for one script hash.

Input object:

```json
{
  "script_hash": "<BtcScriptHash>",
  "block_height": 800000,
  "block_range": { "start": 700000, "end": 800000 }
}
```

- `script_hash`: required, `BtcScriptHash` string (the reversed SHA-256 of a Bitcoin `scriptPubKey`, matching Electrum RPC)
- `block_height`: optional, point query at a specific height
- `block_range`: optional, range query with `[start, end)` semantics

Server-side precedence:

1. If `block_height` is set, point query is used.
2. Else if `block_range` is set, range query is used.
3. If both are absent, latest balance is returned.

Notes:

- Empty range (`start == end`) returns `[]`.
- If no data exists for the address, service returns a zero entry: `block_height=0, delta=0, balance=0`.
- If `block_height` or `block_range` exceeds current `stable_height`, the
  method returns shared consensus error `HEIGHT_NOT_SYNCED`.
- A point height below `balance_query_floor`, or a range start below
  `history_query_floor`, returns `STATE_NOT_RETAINED`.

### 7) `get_addresses_balances`

Batch version of `get_address_balance`.

- Input: `script_hashes[]` plus optional `block_height` / `block_range`.
- Output: 2D array, outer order matches input `script_hashes` order.
- Height/range validation matches `get_address_balance`, including
  `HEIGHT_NOT_SYNCED` for future stable heights and `STATE_NOT_RETAINED` below
  the corresponding retention floor.

### 8) `resolve_script_hashes`

Batch resolves `script_hash -> scriptPubKey -> BTC address?`.

This method is for display and diagnostics only. It does not participate in
balance-history block commits and does not change existing balance query
semantics. Result order matches the input `script_hashes` order.

Input object:

```json
{
  "script_hashes": ["<BtcScriptHash-1>", "<BtcScriptHash-2>"],
  "include_script_pubkey": false
}
```

- `script_hashes`: required, at most 1000 items per request.
- `include_script_pubkey`: optional, default `false`; when true, returns raw scriptPubKey hex.

Example result:

```json
{
  "network": "regtest",
  "registry": {
    "state": "ready",
    "coverage_mode": "snapshot_plus_sidecar",
    "capabilities": {
      "script_registry_lookup": true,
      "script_registry_complete_coverage": true
    },
    "overlay_estimated_count": 42,
    "base_height": 963800,
    "base_block_hash": "....",
    "core_snapshot_id": "....",
    "registry_artifact_id": "....",
    "expected_count": 1541365559,
    "policy": "auxiliary_seen_scripts_non_consensus_v1",
    "last_error": null
  },
  "items": [
    {
      "script_hash": "<BtcScriptHash>",
      "status": "found_overlay",
      "source": "overlay",
      "script_pubkey": null,
      "address": "bcrt1p...",
      "address_type": "p2tr",
      "standard": true
    },
    {
      "script_hash": "<missing-BtcScriptHash>",
      "status": "not_found",
      "source": null,
      "script_pubkey": null,
      "address": null,
      "address_type": null,
      "standard": false
    }
  ]
}
```

Notes:

- Lookup order is RocksDB overlay first, then immutable SQLite base for misses.
- `found_overlay` and `found_base` identify the source of a valid mapping.
- `not_found` is only returned with complete coverage. `unresolved` means the node lacks
  historical sidecar coverage and cannot decide whether an old mapping exists.
- `conflict` means the stored value failed hash validation; it never changes balance or consensus readiness.
- `address=null` means a scriptPubKey exists but cannot be encoded as a standard address on the current BTC network.
- `address_type` is a display classification such as `p2tr`, `p2wpkh`, `p2wsh`, `p2sh`, `p2pkh`, `op_return`, or `non_standard`.

### 9) `stop`

Sends shutdown signal to service for graceful stop.

## Error Handling

- Transport-level issues still use JSON-RPC standard errors such as `InvalidParams`
  and `InternalError`.
- Consensus-sensitive query failures are being migrated to the shared BTC-side
  error contract. Currently adopted here:
  - `HEIGHT_NOT_SYNCED` (`-32040`)
  - `SNAPSHOT_NOT_READY` (`-32041`)
- These errors include structured `data` with fields such as:
  - `service`
  - `requested_height`
  - `upstream_stable_height`
  - `consensus_ready`
  - `actual_state`

Example:

```json
{
  "code": -32040,
  "message": "HEIGHT_NOT_SYNCED",
  "data": {
    "service": "balance-history",
    "requested_height": 900130,
    "upstream_stable_height": 900123,
    "consensus_ready": false,
    "actual_state": {
      "stable_height": 900123,
      "stable_block_hash": "000000..."
    }
  }
}
```

## curl Examples

```bash
curl -s http://127.0.0.1:28010 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_block_height","params":[],"id":1}'
```

```bash
curl -s http://127.0.0.1:28010 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_address_balance","params":[{"script_hash":"<BtcScriptHash>","block_height":800000,"block_range":null}],"id":2}'
```

## Compatibility Notes

- This document reflects current implementation in:
  - `src/btc/balance-history/src/service/rpc.rs`
  - `src/btc/balance-history/src/service/server.rs`
- Future additions should keep backward compatibility (prefer optional fields and non-breaking response changes).
