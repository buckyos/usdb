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

### 5) `get_snapshot_info`

Returns metadata for the current stable snapshot.

Example result:

```json
{
  "stable_height": 812345,
  "stable_block_hash": "000000...",
  "latest_block_commit": "4f7c...",
  "stable_lag": 0,
  "balance_history_api_version": "1.0.0",
  "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
  "commit_protocol_version": "1.0.0",
  "commit_hash_algo": "sha256"
}
```

When the stable snapshot is not yet complete, this method now returns the
shared consensus error `SNAPSHOT_NOT_READY` with structured JSON `data`.

### 6) `get_address_balance`

Queries balance history for one script hash.

Input object:

```json
{
  "script_hash": "<USDBScriptHash>",
  "block_height": 800000,
  "block_range": { "start": 700000, "end": 800000 }
}
```

- `script_hash`: required, USDBScriptHash string
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

### 7) `get_addresses_balances`

Batch version of `get_address_balance`.

- Input: `script_hashes[]` plus optional `block_height` / `block_range`.
- Output: 2D array, outer order matches input `script_hashes` order.
- Height/range validation matches `get_address_balance`, including
  `HEIGHT_NOT_SYNCED` for future stable heights.

### 8) `stop`

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
  -d '{"jsonrpc":"2.0","method":"get_address_balance","params":[{"script_hash":"<USDBScriptHash>","block_height":800000,"block_range":null}],"id":2}'
```

## Compatibility Notes

- This document reflects current implementation in:
  - `src/btc/balance-history/src/service/rpc.rs`
  - `src/btc/balance-history/src/service/server.rs`
- Future additions should keep backward compatibility (prefer optional fields and non-breaking response changes).
