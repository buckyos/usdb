# Balance History JSON-RPC Documentation

## Overview

`balance-history` tracks per-address balance changes (`delta`) and resulting balances (`balance`) across block heights, and exposes query APIs via JSON-RPC.

- Default endpoint: `http://127.0.0.1:8099`
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

### 4) `get_address_balance`

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

### 5) `get_addresses_balances`

Batch version of `get_address_balance`.

- Input: `script_hashes[]` plus optional `block_height` / `block_range`.
- Output: 2D array, outer order matches input `script_hashes` order.

### 6) `stop`

Sends shutdown signal to service for graceful stop.

## Error Handling

- Internal failures are returned as JSON-RPC `InternalError`.
- RPC clients should log `method`, parameter summary, and raw error payload for troubleshooting.

## curl Examples

```bash
curl -s http://127.0.0.1:8099 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_block_height","params":[],"id":1}'
```

```bash
curl -s http://127.0.0.1:8099 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_address_balance","params":[{"script_hash":"<USDBScriptHash>","block_height":800000,"block_range":null}],"id":2}'
```

## Compatibility Notes

- This document reflects current implementation in:
  - `src/btc/balance-history/src/service/rpc.rs`
  - `src/btc/balance-history/src/service/server.rs`
- Future additions should keep backward compatibility (prefer optional fields and non-breaking response changes).
