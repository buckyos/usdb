# USDB Indexer JSON-RPC v1 设计草案

## 1. 目标与范围

本文定义 `usdb-indexer` 对外查询接口的 **v1 版本**（JSON-RPC 2.0），目标是：

- 覆盖矿工证协议核心查询：`pass` 状态、历史、活跃集合、能量、活跃余额快照。
- 明确区块高度语义，避免“当前视图污染历史视图”的歧义。
- 保持与 `balance-history` 的接口风格一致（HTTP + JSON-RPC 2.0，snake_case 方法名）。

> 说明：本文是协议与语义设计稿，便于先统一接口口径；实现可按阶段落地。

---

## 2. 传输与版本

- 传输协议：HTTP + JSON-RPC 2.0
- 编码：`application/json; charset=utf-8`
- 方法命名：snake_case
- 推荐监听：`127.0.0.1:<port>`（默认仅内网）

### 2.1 版本策略

- 采用语义化版本：`api_version = "1.0.0"`
- 破坏性变更仅允许通过主版本升级（`2.x`）
- v1 内允许新增可选字段，不删除既有字段

---

## 3. 统一语义约束（避免歧义）

### 3.1 高度语义（关键）

- 所有带 `block_height`/`at_height` 的查询均采用：
  - **包含边界**：查询高度 `h` 时，包含 `h` 区块内已落库事件（`<= h`）。
  - 即“`h` 看到变更后状态，`h-1` 看到变更前状态”。

### 3.2 历史视图优先

- v1 对矿工证状态类查询默认基于 `miner_pass_state_history` 计算快照，不直接依赖 `miner_passes` 当前状态。
- `miner_passes` 仅用于返回静态元数据（如 `mint_owner/eth_main/prev/invalid_code`）。

### 3.3 查询高度合法性

- 若请求高度 `> synced_block_height`，返回 `HEIGHT_NOT_SYNCED` 错误。
- 未传高度时，服务端使用当前 `synced_block_height`，并在结果中回传 `resolved_height`。

### 3.4 分页稳定性

- 分页查询必须带 `at_height`（或由首包回传 `resolved_height` 并在后续分页复用）。
- 排序固定并可重放，避免跨页重复/遗漏。

---

## 4. 数据模型

## 4.1 MinerPassState

枚举值：

- `active`
- `dormant`
- `consumed`
- `burned`
- `invalid`

## 4.2 PassSnapshot

```json
{
  "inscription_id": "txidi0",
  "inscription_number": 123,
  "mint_txid": "txid",
  "mint_block_height": 900123,
  "mint_owner": "<USDBScriptHash>",
  "eth_main": "0x...",
  "eth_collab": "0x... or null",
  "prev": ["txidi0"],
  "invalid_code": "INVALID_ETH_MAIN",
  "invalid_reason": "Invalid eth_main format",
  "owner": "<USDBScriptHash>",
  "state": "active",
  "satpoint": "txid:vout:offset",
  "last_event_id": 10086,
  "last_event_type": "state_update",
  "resolved_height": 900123
}
```

## 4.3 PassHistoryEvent

```json
{
  "event_id": 10086,
  "inscription_id": "txidi0",
  "block_height": 900123,
  "event_type": "owner_transfer",
  "state": "dormant",
  "owner": "<USDBScriptHash>",
  "satpoint": "txid:vout:offset"
}
```

## 4.4 PassEnergySnapshot

```json
{
  "inscription_id": "txidi0",
  "record_block_height": 900123,
  "query_block_height": 900130,
  "state": "dormant",
  "active_block_height": 900100,
  "owner_address": "<USDBScriptHash>",
  "owner_balance": 123000000,
  "owner_delta": -10000,
  "energy": 123456789
}
```

## 4.5 ActiveBalanceSnapshot

```json
{
  "block_height": 900123,
  "total_balance": 1234567890,
  "active_address_count": 4321
}
```

---

## 5. RPC 方法（v1 最小集合）

## 5.1 基础状态

### 1) `get_rpc_info`

返回接口版本与服务能力。

返回：

```json
{
  "service": "usdb-indexer",
  "api_version": "1.0.0",
  "network": "mainnet",
  "features": [
    "pass_snapshot",
    "pass_history",
    "active_passes_at_height",
    "energy_snapshot",
    "active_balance_snapshot"
  ]
}
```

### 2) `get_network_type`

返回网络类型（`mainnet`/`testnet`/`signet`/`regtest`）。

### 3) `get_sync_status`

返回索引进度与依赖高度。

返回建议：

```json
{
  "genesis_block_height": 900000,
  "synced_block_height": 900123,
  "latest_depend_synced_block_height": 900130,
  "current": 900123,
  "total": 900130,
  "message": "Syncing block 900124"
}
```

### 4) `get_synced_block_height`

返回 `usdb-indexer` 已持久化提交的最新高度（SQLite savepoint commit 后高度）。

---

## 5.2 矿工证（Pass）查询

### 5) `get_pass_snapshot`

按 inscription 查询某高度快照。

参数：

```json
{
  "inscription_id": "txidi0",
  "at_height": 900123
}
```

语义：

- 使用 `history <= at_height` 解析动态状态（`state/owner/satpoint`）。
- 静态字段来自 `miner_passes`（`mint_owner/eth_main/prev/invalid_*`）。
- 若 `at_height` 为空，则自动使用 `synced_block_height` 并返回 `resolved_height`。

### 6) `get_pass_history`

查询某 inscription 的历史事件流。

参数：

```json
{
  "inscription_id": "txidi0",
  "from_height": 900000,
  "to_height": 900200,
  "order": "asc",
  "page": 0,
  "page_size": 100
}
```

约束：

- 高度区间为闭区间 `[from_height, to_height]`。
- `order` 仅允许 `asc` / `desc`。
- `page` 从 `0` 开始。

### 7) `get_active_passes_at_height`

查询某高度活跃矿工证集合（历史视图）。

参数：

```json
{
  "at_height": 900123,
  "page": 0,
  "page_size": 1000
}
```

返回：

```json
{
  "resolved_height": 900123,
  "total": 1234,
  "items": [
    {
      "inscription_id": "txidi0",
      "owner": "<USDBScriptHash>"
    }
  ]
}
```

排序：

- 固定按 `(block_height DESC, event_id DESC)`。

### 8) `get_pass_stats_at_height`

查询某高度下的 pass 状态聚合统计（历史视图）。

参数：

```json
{
  "at_height": 900123
}
```

返回：

```json
{
  "resolved_height": 900123,
  "total_count": 10000,
  "active_count": 6000,
  "dormant_count": 3000,
  "consumed_count": 500,
  "burned_count": 200,
  "invalid_count": 300
}
```

### 9) `get_owner_active_pass_at_height`

查询某地址在高度 `h` 是否有活跃矿工证（按历史视图）。

参数：

```json
{
  "owner": "<USDBScriptHash>",
  "at_height": 900123
}
```

返回：

- `null`：无活跃 pass
- `PassSnapshot`：存在唯一活跃 pass
- 若出现多条，返回 `DUPLICATE_ACTIVE_OWNER`（硬错误）

### 10) `get_invalid_passes`

查询无效 mint 记录，便于外部排障。

参数：

```json
{
  "error_code": "INVALID_ETH_MAIN",
  "from_height": 900000,
  "to_height": 900200,
  "page": 0,
  "page_size": 100
}
```

返回包含：

- `resolved_height`：服务端最终解析高度。
- `total`：闭区间内总记录数（用于分页）。
- `items`：当前页无效 pass 列表。

---

## 5.3 能量查询

### 11) `get_pass_energy`

查询某 inscription 在目标高度的能量快照。

参数：

```json
{
  "inscription_id": "txidi0",
  "block_height": 900123,
  "mode": "at_or_before"
}
```

`mode` 枚举：

- `exact`：仅接受该高度存在记录
- `at_or_before`：返回 `<= block_height` 的最近记录（推荐默认）

### 12) `get_pass_energy_range`

查询某 inscription 在区间内的能量记录（用于可视化时间线）。

参数：

```json
{
  "inscription_id": "txidi0",
  "from_height": 900000,
  "to_height": 900200,
  "order": "desc",
  "page": 0,
  "page_size": 100
}
```

`order` 可选，允许 `asc` / `desc`，默认 `asc`。

返回包含：

- `resolved_height`：服务端最终解析高度。
- `total`：闭区间内总记录数（用于分页）。
- `items`：当前页记录。

### 13) `get_pass_energy_leaderboard`

查询某高度 pass 的能量排行榜（按 `energy DESC`）。

参数：

```json
{
  "at_height": 900123,
  "scope": "active",
  "page": 0,
  "page_size": 100
}
```

`scope` 可选，允许：

- `active`：仅 `active`（默认）
- `active_dormant`：`active + dormant`
- `all`：全部状态（`active/dormant/consumed/burned/invalid`）

返回：

```json
{
  "resolved_height": 900123,
  "total": 6000,
  "items": [
    {
      "inscription_id": "txidi0",
      "owner": "<USDBScriptHash>",
      "record_block_height": 900123,
      "state": "active",
      "energy": 123456789
    }
  ]
}
```

---

## 5.4 活跃地址余额快照

### 14) `get_active_balance_snapshot`

查询指定高度快照（精确高度）。

参数：

```json
{
  "block_height": 900123
}
```

返回：`ActiveBalanceSnapshot` 或 `SNAPSHOT_NOT_FOUND`。

### 15) `get_latest_active_balance_snapshot`

查询最近一次已落库快照。

---

## 5.5 管理

### 16) `stop`

触发索引器优雅停止（建议默认仅 localhost 可访问）。

---

## 6. 错误码（业务层）

除标准 JSON-RPC 错误外，建议统一扩展：

- `-32010 HEIGHT_NOT_SYNCED`
- `-32011 PASS_NOT_FOUND`
- `-32012 ENERGY_NOT_FOUND`
- `-32013 SNAPSHOT_NOT_FOUND`
- `-32014 DUPLICATE_ACTIVE_OWNER`
- `-32015 INVALID_PAGINATION`
- `-32016 INVALID_HEIGHT_RANGE`
- `-32017 INTERNAL_INVARIANT_BROKEN`

错误对象建议包含：

```json
{
  "code": -32010,
  "message": "HEIGHT_NOT_SYNCED",
  "data": {
    "requested_height": 900500,
    "synced_height": 900123
  }
}
```

---

## 7. 无歧义约束清单（实现必须遵守）

1. 高度查询全部采用 `<= h` 的包含边界语义。  
2. 只要请求带 `h`，返回中必须带 `resolved_height`。  
3. 分页查询必须固定排序，且文档公开排序键。  
4. 所有列表接口返回顺序必须稳定可重放。  
5. `owner_active_pass` 发现重复活跃 owner 必须报错，不可“取第一条”。  
6. `invalid` pass 必须可查到 `invalid_code` 与 `invalid_reason`。  
7. 业务错误码必须稳定，不得随意复用文案替代错误码。  

---

## 8. v1 实现建议（分阶段）

- **Phase A（最小可用）**：
  - `get_rpc_info`
  - `get_network_type`
  - `get_sync_status`
  - `get_synced_block_height`
  - `get_pass_snapshot`
  - `get_active_passes_at_height`
  - `get_pass_stats_at_height`
  - `get_pass_energy`
  - `get_active_balance_snapshot`
  - `get_latest_active_balance_snapshot`

- **Phase B（增强）**：
  - `get_pass_history`
  - `get_owner_active_pass_at_height`
  - `get_pass_energy_range`
  - `get_pass_energy_leaderboard`
  - `get_invalid_passes`
  - `stop`

---

## 9. 与当前实现的映射（便于开发）

- `miner_passes`：静态字段主表（含 `invalid_code/reason`）  
- `miner_pass_state_history`：历史事件与高度快照来源  
- `active_balance_snapshots`：活跃地址总余额快照  
- `pass_energy`（RocksDB）：能量记录  

建议默认以 `history` 作为状态口径，避免“当前状态污染历史重放”的歧义。
