# Balance History JSON-RPC 文档

## 概述

`balance-history` 服务用于追踪并查询每个地址在不同区块高度的余额变化（`delta`）与余额总额（`balance`），并通过 JSON-RPC 暴露查询接口。

实现 review 与后续修复跟踪见：[balance-history-review-remediation-plan.md](./balance-history-review-remediation-plan.md)

- 默认监听地址：`http://127.0.0.1:28010`
- 传输协议：HTTP + JSON-RPC 2.0
- CORS：`AllowAny`

## 数据模型

### AddressBalance

```json
{
  "block_height": 123456,
  "balance": 100000,
  "delta": 5000
}
```

- `block_height`：区块高度
- `balance`：该高度后的余额总额（单位：satoshi）
- `delta`：该高度的余额变化量（单位：satoshi，可正可负）

### SyncStatus

```json
{
  "phase": "Indexing",
  "current": 800000,
  "total": 900000,
  "message": "Synced up to block height 800000"
}
```

- `phase`：同步阶段，枚举值为 `Initializing` / `Loading` / `Indexing` / `Synced`
- `current`：当前进度
- `total`：总进度
- `message`：可选状态信息

## 通用请求格式

```json
{
  "jsonrpc": "2.0",
  "method": "<method_name>",
  "params": [ ... ],
  "id": 1
}
```

## RPC 方法

### 1) `get_network_type`

返回服务当前 BTC 网络类型（例如 `mainnet` / `testnet` / `signet` / `regtest`）。

请求：

```json
{
  "jsonrpc": "2.0",
  "method": "get_network_type",
  "params": [],
  "id": 1
}
```

结果：

```json
"mainnet"
```

### 2) `get_block_height`

返回数据库当前已同步的 BTC 高度。

请求：

```json
{
  "jsonrpc": "2.0",
  "method": "get_block_height",
  "params": [],
  "id": 1
}
```

结果：

```json
812345
```

### 3) `get_sync_status`

返回同步状态。

请求：

```json
{
  "jsonrpc": "2.0",
  "method": "get_sync_status",
  "params": [],
  "id": 1
}
```

结果示例：

```json
{
  "phase": "Loading",
  "current": 420000,
  "total": 900000,
  "message": "Starting block load"
}
```

### 4) `get_readiness`

返回结构化 readiness 状态，用于区分：

1. RPC 是否可访问；
2. 普通查询是否可用；
3. 当前 stable snapshot 是否可被下游用于共识消费。

请求：

```json
{
  "jsonrpc": "2.0",
  "method": "get_readiness",
  "params": [],
  "id": 1
}
```

结果示例：

```json
{
  "service": "balance-history",
  "rpc_alive": true,
  "query_ready": true,
  "consensus_ready": false,
  "phase": "Indexing",
  "current": 800000,
  "total": 900000,
  "message": "Syncing blocks 799001 to 900000",
  "stable_height": 800000,
  "stable_block_hash": "....",
  "latest_block_commit": "....",
  "blockers": ["CatchingUp"]
}
```

说明：

1. 不应再用 `get_network_type` 代替 readiness 判断；
2. `rpc_alive=true` 只说明服务活着，不说明快照适合共识消费；
3. 下游若要做严格 gating，应使用 `consensus_ready=true`。

### 5) `get_snapshot_info`

返回当前 stable snapshot 元数据。

请求：

```json
{
  "jsonrpc": "2.0",
  "method": "get_snapshot_info",
  "params": [],
  "id": 1
}
```

结果示例：

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

说明：

- 当 stable snapshot 尚不完整，例如 stable height 已存在，但 `stable_block_hash` 或 `latest_block_commit` 尚不可用时，返回共享共识错误 `SNAPSHOT_NOT_READY`；
- 新的错误返回会携带结构化 `data`，其中包含当前 `stable_height`、`consensus_ready` 与 `actual_state`，供下游做自动判定。

### 6) `get_address_balance`

查询单个地址（script hash）余额历史。

参数对象：

```json
{
  "script_hash": "<USDBScriptHash>",
  "block_height": 800000,
  "block_range": { "start": 700000, "end": 800000 }
}
```

- `script_hash`：必填，USDBScriptHash 字符串
- `block_height`：可选，指定高度查询
- `block_range`：可选，区间查询，语义为 `[start, end)`

查询优先级（服务端行为）：

1. 若 `block_height` 有值，优先按高度查询；
2. 否则若 `block_range` 有值，按区间查询；
3. 两者都没有时，返回最新地址余额。

请求示例（按高度）：

```json
{
  "jsonrpc": "2.0",
  "method": "get_address_balance",
  "params": [
    {
      "script_hash": "<USDBScriptHash>",
      "block_height": 800000,
      "block_range": null
    }
  ],
  "id": 1
}
```

结果示例：

```json
[
  {
    "block_height": 800000,
    "balance": 123456789,
    "delta": -10000
  }
]
```

请求示例（按区间）：

```json
{
  "jsonrpc": "2.0",
  "method": "get_address_balance",
  "params": [
    {
      "script_hash": "<USDBScriptHash>",
      "block_height": null,
      "block_range": { "start": 799000, "end": 800000 }
    }
  ],
  "id": 1
}
```

说明：

- 当 `block_range` 为空区间（`start == end`）时返回空数组 `[]`。
- 若目标地址暂无数据，返回默认零值记录（`block_height=0, delta=0, balance=0`）。
- 当 `block_height` 或 `block_range` 超出当前 `stable_height` 时，返回共享共识错误 `HEIGHT_NOT_SYNCED`，而不是隐式回退到当前可用高度。

### 7) `get_addresses_balances`

批量查询多个地址余额历史。

参数对象：

```json
{
  "script_hashes": ["<USDBScriptHash-1>", "<USDBScriptHash-2>"],
  "block_height": null,
  "block_range": { "start": 799000, "end": 800000 }
}
```

结果：二维数组，外层顺序与 `script_hashes` 输入顺序一致，每个元素是对应地址的 `AddressBalance[]`。

- 对高度/区间的合法性约束与 `get_address_balance` 相同；
- 若任一请求高度越过当前 `stable_height`，返回共享共识错误 `HEIGHT_NOT_SYNCED`。

## 统一错误模型（共识查询层）

对外 JSON-RPC 仍然保留标准：

- `InvalidParams`
- `InternalError`

此外，`balance-history` 已开始接入跨服务共享的共识错误契约，当前已实际用于：

- `SNAPSHOT_NOT_READY` (`-32041`)
- `HEIGHT_NOT_SYNCED` (`-32040`)

错误示例：

```json
{
  "jsonrpc": "2.0",
  "error": {
    "code": -32040,
    "message": "HEIGHT_NOT_SYNCED",
    "data": {
      "service": "balance-history",
      "requested_height": 900130,
      "local_synced_height": null,
      "upstream_stable_height": 900123,
      "consensus_ready": false,
      "expected_state": {},
      "actual_state": {
        "stable_height": 900123,
        "stable_block_hash": "000000..."
      },
      "detail": "Requested height 900130 is above current stable height 900123"
    }
  },
  "id": 1
}
```

说明：

- `message` 是稳定的机器可读错误名；
- `data.actual_state` 描述服务当时实际看到的 stable 视图；
- 下游不应再仅靠错误字符串自由文本判断是否可重试或是否属于快照漂移。

### 7) `stop`

向服务发送停止信号，触发优雅退出。

请求：

```json
{
  "jsonrpc": "2.0",
  "method": "stop",
  "params": [],
  "id": 1
}
```

结果：

```json
null
```

## 错误处理

- 服务端内部错误使用 JSON-RPC `InternalError` 返回。
- `result` 解析失败或调用失败时，客户端会收到标准 JSON-RPC `error` 对象。
- 建议调用方记录 `method`、参数摘要和错误信息，便于排障。

## curl 调用示例

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

## 兼容性说明

- 当前文档对应 `src/btc/balance-history/src/service/rpc.rs` 与 `src/btc/balance-history/src/service/server.rs` 的现状实现。
- 若后续新增字段或新方法，建议保持向后兼容（新增可选字段、避免破坏现有返回结构）。
