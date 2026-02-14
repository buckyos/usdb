# Balance History JSON-RPC 文档

## 概述

`balance-history` 服务用于追踪并查询每个地址在不同区块高度的余额变化（`delta`）与余额总额（`balance`），并通过 JSON-RPC 暴露查询接口。

- 默认监听地址：`http://127.0.0.1:8099`
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

### 4) `get_address_balance`

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

### 5) `get_addresses_balances`

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

### 6) `stop`

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
curl -s http://127.0.0.1:8099 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_block_height","params":[],"id":1}'
```

```bash
curl -s http://127.0.0.1:8099 \
  -H 'Content-Type: application/json' \
  -d '{"jsonrpc":"2.0","method":"get_address_balance","params":[{"script_hash":"<USDBScriptHash>","block_height":800000,"block_range":null}],"id":2}'
```

## 兼容性说明

- 当前文档对应 `src/btc/balance-history/src/service/rpc.rs` 与 `src/btc/balance-history/src/service/server.rs` 的现状实现。
- 若后续新增字段或新方法，建议保持向后兼容（新增可选字段、避免破坏现有返回结构）。
