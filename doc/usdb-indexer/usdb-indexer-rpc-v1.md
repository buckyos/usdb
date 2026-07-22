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
- 当前仍处于 dev / Draft 阶段，正式 v1 冻结前不提供旧请求或旧数据格式兼容承诺。
- 正式 v1 冻结后，破坏性变更仅允许通过主版本升级（`2.x`）。
- 正式 v1 内允许新增可选字段，不删除既有字段。

---

## 3. 统一语义约束（避免歧义）

### 3.1 高度语义（关键）

- 所有带 `block_height`/`at_height` 的查询均采用：
  - **包含边界**：查询高度 `h` 时，包含 `h` 区块内已落库事件（`<= h`）。
  - 即“`h` 看到变更后状态，`h-1` 看到变更前状态”。

### 3.2 历史视图优先

- v1 对矿工证状态类查询默认基于 `miner_pass_state_history` 计算快照，不直接依赖 `miner_passes` 当前状态。
- `miner_passes` 仅用于返回静态元数据（如 `mint_owner/usdb_main/prev/invalid_code`）。

### 3.3 查询高度合法性

- 若请求高度 `> synced_block_height`，返回 `HEIGHT_NOT_SYNCED` 错误。
- 普通查询未传高度时，服务端使用当前 `synced_block_height`，并在结果中回传 `resolved_height`。
- UIP-0006 economic view 不返回裸 `resolved_height`；目标高度和完整历史 identity 统一由 `external_state` 返回。

### 3.4 分页稳定性

- 普通数字分页查询必须带 `at_height`（或由首包回传 `resolved_height` 并在后续分页复用）。
- UIP-0006 candidate/breakdown 使用绑定完整 `external_state` 的 opaque cursor；后续页不得重新解析 current head。
- 排序固定并可重放，避免跨页重复/遗漏。

### 3.5 ETHW 验块必须绑定历史 state ref，而不是当前 head

如果 ETHW 区块在出块时记录了：

- `btc_height`
- `snapshot_id`
- `system_state_id`
- `pass info / energy`

那么验证方收到该区块后，必须基于 **`btc_height` 对应的历史状态** 进行校验，而不是直接读取当前 head 状态。

这意味着：

- BTC 侧即使已经从 `H` 前进到 `H+1`，仍然应当允许验证高度 `H` 的历史 `pass info`
- “当前 head 前进”不应直接导致 `SNAPSHOT_ID_MISMATCH / SYSTEM_STATE_ID_MISMATCH`
- 只有在服务能够重建高度 `H` 的历史状态，但重建出的 `snapshot_id / system_state_id` 与区块记录不一致时，才属于真正的 mismatch

因此，后续共识化接口需要补一层历史 state ref 查询能力，而不仅仅是“返回当前 snapshot/system state”。

更具体地说，当前这些接口：

- `get_snapshot_info`
- `get_local_state_commit_info`
- `get_system_state_info`

都只是 **current-head introspection**，只能回答“现在这台 `usdb-indexer` 的当前状态是什么”。

它们不能单独满足 ETHW 验块，因为 ETHW 校验需要的是：

- “高度 `H` 的历史 state ref 是什么”
- “我拿区块里固定的 `(snapshot_id, system_state_id)` 去校验高度 `H`，是否仍然一致”

所以 `v1` 当前态接口和后续历史 state ref 接口，在语义上必须明确分层。

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
  "mint_owner": "<BtcScriptHash>",
  "usdb_main": "0x...",
  "prev": ["txidi0"],
  "invalid_code": "INVALID_USDB_MAIN",
  "invalid_reason": "Invalid usdb_main format",
  "owner": "<BtcScriptHash>",
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
  "owner": "<BtcScriptHash>",
  "satpoint": "txid:vout:offset"
}
```

## 4.4 PassEnergySnapshot

```json
{
  "inscription_id": "txidi0",
  "record_block_height": 900123,
  "query_block_height": 900130,
  "state": "active",
  "active_block_height": 900100,
  "owner_address": "<BtcScriptHash>",
  "owner_balance": 123000000,
  "owner_delta": -10000,
  "raw_energy": "123456789",
  "collab_contribution": "0",
  "effective_energy": "123456789",
  "level": 19,
  "difficulty_factor_bps": 8100
}
```

`raw_energy` 是 pass 自身 raw energy 的 canonical decimal string 编码；内部按 `u128` 计算和存储，RPC 不使用 JSON number 表达 energy 字段。

`raw_energy` 保持 UIP-0003 raw energy 口径。`collab_contribution` 和 `effective_energy` 为 UIP-0004 / UIP-0006 派生查询面：active standard pass 返回运行时聚合的 collab contribution 与 `raw + contribution`，active collab pass 和 non-active pass 的 `effective_energy` 为 `"0"`。这些派生值不写回 raw energy ledger。

`level` 和 `difficulty_factor_bps` 按 UIP-0005 从 `effective_energy` 运行时派生，不写入 raw energy ledger，也不依赖 ETHW `base_difficulty` / `real_difficulty`。

## 4.5 ActiveBalanceSnapshot

```json
{
  "block_height": 900123,
  "total_balance": 1234567890,
  "active_address_count": 4321
}
```

## 4.6 HistoricalStateRef

这是为 ETHW 验块补充的历史状态引用结构。第一版接口已经落地，
用于回答“高度 `H` 上，这台服务承诺的历史 state ref 是什么”。

当前阶段的能力边界：

- 已支持 exact-height 历史 state ref 查询
- 已支持基于 `expected_state` 的 `SNAPSHOT_ID_MISMATCH / BLOCK_HASH_MISMATCH / VERSION_MISMATCH / ACTIVE_VERSION_SET_MISMATCH / LOCAL_STATE_COMMIT_MISMATCH / SYSTEM_STATE_ID_MISMATCH`
- historical activation lookup 还会区分 `ACTIVATION_RECORD_NOT_FOUND / ACTIVATION_RECORD_CONFLICT / VERSION_NOT_SUPPORTED / FORMULA_VERSION_MISMATCH / COMMIT_PROTOCOL_VERSION_MISMATCH`
- 已区分：
  - `STATE_NOT_RETAINED`：高度低于当前统一历史保留窗口下界（当前实现即 `genesis_block_height`）
  - `HISTORY_NOT_AVAILABLE`：高度仍在保留窗口内，但当前缺少构造历史 state ref 所需的辅助数据

建议字段：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "block_height": 900123,
  "snapshot_info": {
    "snapshot_id": "snapshot-...",
    "balance_history_stable_height": 900123,
    "stable_block_hash": "000000..."
  },
  "local_state_commit_info": {
    "local_state_commit": "local-..."
  },
  "system_state_info": {
    "system_state_id": "system-..."
  }
}
```

语义：

- 表示“高度 `H` 上，这台服务可重建并承诺的历史 state ref”
- 该对象将作为 ETHW 验块时的固定外部状态引用
- 第一版实现通过一个包装对象把 `snapshot_info / local_state_commit_info / system_state_info` 固定到同一历史高度上

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
    "historical_state_ref",
    "pass_economic_profile",
    "candidate_set_view",
    "collab_breakdown",
    "active_balance_snapshot"
  ],
  "economic_state_view_version": "uip-0006-usdb-economic-state-view:v1",
  "candidate_set_selection_rule": "uip-0006:effective-energy-desc-pass-id-asc:v1",
  "economic_page_max_limit": 500,
  "activation_registry_id": "..."
}
```

UIP-0006 client 不应仅凭服务可达性推断经济视图可用。当前 v1 要求：

- `service == "usdb-indexer"` 且 `api_version == "1.0.0"`。
- `features` 同时包含 `historical_state_ref`、`pass_economic_profile`、`candidate_set_view`、`collab_breakdown`。
- `economic_state_view_version` 与请求使用的 `view_version` 一致。
- `candidate_set_selection_rule` 与 UIP-0006 `candidate_set_view` ordering contract 一致；该字段不声明 USDB block-selection policy。
- `economic_page_max_limit > 0`；client 的首包 `limit` 不得超过该声明值。
- `activation_registry_id` 是节点为当前 BTC source network 内置的 UIP-0008 registry canonical SHA-256 id；不包含其他 BTC network 或 USDB chain config。

参考实现中的调用入口：

- Rust `service::client::RpcClient` 对四个历史/经济查询提供强类型方法。
- `usdb-indexer-cli` 提供 `state-ref`、`pass-economic-profile`、`candidate-set-view`、`collab-breakdown` 命令；`--context` 接受 `ConsensusQueryContext` JSON。
- control-plane 的 `/api/services/usdb-indexer/rpc` allowlist 放行这四个方法；`/api/system/overview` 同时返回 indexer 原始声明和 `capabilities.usdb_economic_state_view` 兼容性判断。

### 2) `get_network_type`

返回网络类型（`mainnet`/`testnet`/`signet`/`regtest`）。

### 3) `get_sync_status`

返回索引同步状态，包含本地 durable 已提交高度、上游稳定高度，以及仅用于进度展示的 `current/total`。

完整状态模型说明见：[usdb-indexer-sync-status-model.md](./usdb-indexer-sync-status-model.md)。

返回建议：

```json
{
  "genesis_block_height": 900000,
  "synced_block_height": 900123,
  "balance_history_stable_height": 900130,
  "current": 900123,
  "total": 900130,
  "message": "Syncing block 900124"
}
```

语义：

- `synced_block_height`：`usdb-indexer` 本地 durable 已提交高度。
- `balance_history_stable_height`：`balance-history` 当前稳定高度，也是 `usdb-indexer` 的同步 ceiling。
- `current` / `total`：仅用于进度条展示，不应当作新的高度语义字段解释。

### 4) `get_synced_block_height`

返回 `usdb-indexer` 已持久化提交的最新高度（SQLite savepoint commit 后高度）。

### 5) `get_snapshot_info`

返回当前 adopted upstream snapshot 元数据。

说明：

- 成功时返回当前本地采用的 `balance-history` snapshot 信息；
- 若当前还没有 adopted upstream snapshot anchor，则返回共享共识错误 `SNAPSHOT_NOT_READY`；
- 这条接口描述的是当前本地 adopted 的 upstream snapshot，不是按历史高度回放的 state ref。

### 6) `get_local_state_commit_info`

返回当前 locally durable core-state commit。

说明：

- 成功时返回 `local_state_commit` 与组成它的结构化字段；
- 若当前还没有可用的 adopted snapshot/local state，则返回共享共识错误 `SNAPSHOT_NOT_READY`。

### 7) `get_system_state_info`

返回当前 top-level system state id。

说明：

- 成功时返回 `system_state_id` 及其 identity；
- 若当前还没有完整的 current local/system state，则返回共享共识错误 `SNAPSHOT_NOT_READY`。

### 7.x) `get_state_ref_at_height`

这条接口已作为第一版历史 state ref 查询落地。

参数建议：

```json
{
  "block_height": 900123,
  "context": {
    "requested_height": 900123,
    "expected_state": {
      "snapshot_id": "snapshot-...",
      "local_state_commit": "local-...",
      "system_state_id": "system-..."
    }
  }
}
```

返回建议：

```json
{
  "block_height": 900123,
  "snapshot_info": {
    "snapshot_id": "snapshot-...",
    "balance_history_stable_height": 900123,
    "stable_block_hash": "000000..."
  },
  "local_state_commit_info": {
    "local_state_commit": "local-..."
  },
  "system_state_info": {
    "system_state_id": "system-..."
  }
}
```

当前语义：

- 这是 **历史 state ref** 查询，不是当前 head 查询
- BTC 头部即使已经前进，仍然应允许查询被保留窗口内的历史 state ref
- 当前第一版返回该高度的历史 `snapshot_info / local_state_commit_info / system_state_info`
- `context` 可选；传入 `expected_state` 后，服务会在该高度做 selector 校验
- 当前已支持 `snapshot_id / stable_block_hash / version / local_state_commit / system_state_id` 的 mismatch 错误
- 若高度低于统一历史保留窗口下界（当前实现为 `genesis_block_height`），会返回共享共识错误 `STATE_NOT_RETAINED`
- 若高度仍在保留窗口内，但该节点当前缺少构造历史 state ref 所需的辅助数据，会返回共享共识错误 `HISTORY_NOT_AVAILABLE`
- 后续 ETHW 验块应优先使用这条接口固定 `(height, state ref)`，再用相同上下文复查 pass/energy

---

## 5.2 矿工证（Pass）查询

### 8) `get_pass_snapshot`

按 inscription 查询某高度快照。

参数：

```json
{
  "inscription_id": "txidi0",
  "at_height": 900123,
  "context": {
    "requested_height": 900123,
    "expected_state": {
      "snapshot_id": "snapshot-...",
      "local_state_commit": "local-...",
      "system_state_id": "system-..."
    }
  }
}
```

语义：

- 使用 `history <= at_height` 解析动态状态（`state/owner/satpoint`）。
- 静态字段来自 `miner_passes`（`mint_owner/usdb_main/prev/invalid_*`）。
- 若 `at_height` 为空，则自动使用 `synced_block_height` 并返回 `resolved_height`。
- `context` 可选；若传入，服务会先校验该高度的历史 state ref 是否满足 `expected_state`。
- 若 `at_height` 和 `context.requested_height` 同时出现但不一致，返回 `InvalidParams`。
- 当前已支持 `snapshot_id / stable_block_hash / version / local_state_commit / system_state_id` 的 mismatch 错误。
- 若高度低于统一历史保留窗口下界（当前实现为 `genesis_block_height`），会返回共享共识错误 `STATE_NOT_RETAINED`。
- 若高度合法，但该节点当前缺少构造历史 state ref 所需的辅助数据，会返回共享共识错误 `HISTORY_NOT_AVAILABLE`。

### 9) `get_pass_history`

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

### 10) `get_active_passes_at_height`

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
      "owner": "<BtcScriptHash>"
    }
  ]
}
```

排序：

- 固定按 `(block_height DESC, event_id DESC)`。

### 11) `get_pass_stats_at_height`

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

### 12) `get_owner_active_pass_at_height`

查询某地址在高度 `h` 是否有活跃矿工证（按历史视图）。

参数：

```json
{
  "owner": "<BtcScriptHash>",
  "at_height": 900123
}
```

返回：

- `null`：无活跃 pass
- `PassSnapshot`：存在唯一活跃 pass
- 若出现多条，返回 `DUPLICATE_ACTIVE_OWNER`（硬错误）

### 13) `get_invalid_passes`

查询无效 mint 记录，便于外部排障。

参数：

```json
{
  "error_code": "INVALID_USDB_MAIN",
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

### 14) `get_pass_energy`

查询某 inscription 在目标高度的能量快照。

参数：

```json
{
  "inscription_id": "txidi0",
  "block_height": 900123,
  "context": {
    "requested_height": 900123,
    "expected_state": {
      "snapshot_id": "snapshot-...",
      "system_state_id": "system-..."
    }
  },
  "mode": "at_or_before"
}
```

`mode` 枚举：

- `exact`：仅接受该高度存在记录
- `at_or_before`：返回 `<= block_height` 的最近记录（推荐默认）

补充语义：

- `context` 可选；若传入，服务会先校验该高度的历史 state ref 是否满足 `expected_state`。
- 若 `block_height` 和 `context.requested_height` 同时出现但不一致，返回 `InvalidParams`。
- mismatch 校验成功后，才继续返回业务能量结果；`ENERGY_NOT_FOUND` 仍表示该 pass 在查询模式下没有对应能量记录。
- 若高度低于统一历史保留窗口下界（当前实现为 `genesis_block_height`），会返回共享共识错误 `STATE_NOT_RETAINED`。
- 若高度合法，但该节点当前缺少构造历史 state ref 所需的辅助数据，会返回共享共识错误 `HISTORY_NOT_AVAILABLE`。
- 返回的 `raw_energy`、`collab_contribution`、`effective_energy` 均为 canonical decimal string。`collab_contribution` / `effective_energy` 为运行时派生值，不写回 raw energy ledger。
- 返回的 `level`、`difficulty_factor_bps` 按 UIP-0005 从 `effective_energy` 运行时派生，不写入 energy DB；USDB indexer 不查询、不持久化 ETHW `base_difficulty` 或 `real_difficulty`。

### 15) `get_pass_energy_range`

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
- `items[].energy`：canonical decimal string。

### 16) `get_pass_energy_leaderboard`

查询某高度 pass 的能量排行榜（内部按 `u128 energy DESC` 排序，RPC 输出 canonical decimal string）。

该接口保留为前端/浏览器的 raw energy 展示榜单，不是 UIP-0006 `candidate_set_view`。需要 active-standard 审计集合的调用方必须使用 `get_candidate_set_view`。

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
      "owner": "<BtcScriptHash>",
      "record_block_height": 900123,
      "state": "active",
      "energy": "123456789"
    }
  ]
}
```

### 17) `get_pass_economic_profile`

查询单张 pass 在一个确定历史 context 下的 UIP-0006 经济画像。

参数：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "pass_id": "txidi0",
  "block_height": 900123,
  "context": {
    "requested_height": 900123,
    "expected_state": {
      "snapshot_id": "snapshot-...",
      "stable_height": 900123,
      "stable_block_hash": "000000...",
      "local_state_commit": "local-...",
      "system_state_id": "system-...",
      "balance_history_api_version": "1.0.0",
      "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
      "activation_registry_id": "...",
      "active_version_set_id": "01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691"
    }
  }
}
```

返回：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "external_state": {
    "btc_height": 900123,
    "snapshot_id": "snapshot-...",
    "stable_block_hash": "000000...",
    "local_state_commit": "local-...",
    "system_state_id": "system-...",
    "balance_history_api_version": "1.0.0",
    "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
    "activation_registry_id": "...",
    "active_version_set": {"inscription_schema_version":"uip-0001-miner-pass-inscription:v1","pass_state_machine_version":"uip-0002-pass-state-machine:v1","energy_formula_version":"uip-0003-pass-energy-formula:v1","effective_energy_formula_version":"uip-0004-collab-leader-effective-energy:v1","level_formula_version":"uip-0005-level-and-real-difficulty:v1","query_semantics_version":"uip-0006-economic-query-semantics:v1","state_view_version":"uip-0006-usdb-economic-state-view:v1","commit_protocol_version":"uip-0008-usdb-local-state-commit:v1","balance_history_semantics_version":"balance-snapshot-at-or-before:v1"},
    "active_version_set_id": "01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691"
  },
  "pass": {
    "pass_id": "txidi0",
    "owner_script_hash": "<BtcScriptHash>",
    "owner_btc_addr": null,
    "state": "active",
    "pass_kind": "standard",
    "raw_energy": "1000000",
    "collab_contribution": "500000",
    "effective_energy": "1500000",
    "level": 1,
    "difficulty_factor_bps": 9900,
    "collab_breakdown_count": 2
  }
}
```

补充语义：

- `view_version` 必填，不支持的值返回 `VIEW_VERSION_MISMATCH`。
- `block_height` 与 `context.requested_height` 同时存在时必须相等；未提供 `context` 时仍返回目标高度的完整 `external_state`。
- profile 使用 `at_or_before` raw energy 记录并投影到 `external_state.btc_height`，不要求目标高度恰好存在一条 energy row。
- active standard pass 返回 UIP-0004 聚合后的 contribution/effective energy；active collab 和所有 non-active pass 的 effective energy 为 `"0"`。
- invalid pass 不要求 energy DB row，服务从 pass history 识别后合成 `raw/contribution/effective = "0"`、`level = 0`、`difficulty_factor_bps = 10000`、`collab_breakdown_count = 0`。
- 目标 pass 在该历史 context 下不存在时返回 `PASS_NOT_FOUND`；non-invalid pass 存在但缺少 raw energy 时返回 `INTERNAL_INVARIANT_BROKEN`。
- 当前实现没有 script hash -> BTC address 历史反查索引，因此 `owner_btc_addr` 为 `null`。

### 18) `get_candidate_set_view`

查询某高度的 UIP-0006 candidate set audit view。

`candidate_pass` 严格表示目标 `external_state` 下的 `Active` standard pass。该接口返回全部 `candidate_pass`，排除 collab 和所有 non-active pass，并按 `effective_energy DESC, pass_id ASC` 稳定排序。成员资格不要求 energy 大于零；`effective_energy` 来自 UIP-0004 运行时派生，不写回 raw energy ledger。

排序第一项称为 `top_ranked_candidate`，不自动等于 UIP-0007 `selected_pass`，也不表示已经赢得 PoW 出块竞争。请求/响应字段名 `selection_rule` 只表示本 audit view 的 ordering contract。

参数：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "block_height": 900123,
  "context": {
    "requested_height": 900123,
    "expected_state": {
      "snapshot_id": "snapshot-...",
      "system_state_id": "system-..."
    }
  },
  "selection_rule": "uip-0006:effective-energy-desc-pass-id-asc:v1",
  "cursor": null,
  "limit": 100
}
```

`selection_rule` 可选；当前仅支持：

- `uip-0006:effective-energy-desc-pass-id-asc:v1`

首次请求传 `cursor = null`。后续请求将上页返回的 `next_cursor` 原样传回；可以省略 `block_height/context`，cursor 内绑定的完整 `external_state` 是 continuation 的权威历史 context。若显式重复这些 selector，必须与 cursor 一致。

旧 `page/page_size` 字段不受支持，且不会与 cursor 分页并存。

返回：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "external_state": {
    "btc_height": 900123,
    "snapshot_id": "snapshot-...",
    "stable_block_hash": "000000...",
    "local_state_commit": "local-...",
    "system_state_id": "system-...",
    "balance_history_api_version": "1.0.0",
    "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
    "activation_registry_id": "...",
    "active_version_set": {"inscription_schema_version":"uip-0001-miner-pass-inscription:v1","pass_state_machine_version":"uip-0002-pass-state-machine:v1","energy_formula_version":"uip-0003-pass-energy-formula:v1","effective_energy_formula_version":"uip-0004-collab-leader-effective-energy:v1","level_formula_version":"uip-0005-level-and-real-difficulty:v1","query_semantics_version":"uip-0006-economic-query-semantics:v1","state_view_version":"uip-0006-usdb-economic-state-view:v1","commit_protocol_version":"uip-0008-usdb-local-state-commit:v1","balance_history_semantics_version":"balance-snapshot-at-or-before:v1"},
    "active_version_set_id": "01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691"
  },
  "selection_rule": "uip-0006:effective-energy-desc-pass-id-asc:v1",
  "total": 6000,
  "limit": 100,
  "max_limit": 500,
  "next_cursor": "opaque-cursor-or-null",
  "items": [
    {
      "pass_id": "txidi0",
      "owner_script_hash": "<BtcScriptHash>",
      "state": "active",
      "pass_kind": "standard",
      "record_block_height": 900120,
      "raw_energy": "1000000",
      "collab_contribution": "500000",
      "effective_energy": "1500000",
      "level": 1,
      "difficulty_factor_bps": 9900
    }
  ]
}
```

补充语义：

- `context` 校验语义与 `get_pass_energy` 一致。
- `view_version` 必填；字段缺失是无效参数，不保留旧请求兼容入口。不支持的值返回 `VIEW_VERSION_MISMATCH`。
- 即使未传 `context`，服务也必须重建目标高度的完整历史 identity 并返回 `external_state`；缺失历史辅助数据返回 `HISTORY_NOT_AVAILABLE`。
- 服务在派生 candidate 数据前后重建并比较完整 state ref；若期间发生同高度 reorg，返回对应 state mismatch，不返回混合历史状态。
- `total` 是该 `external_state` 下 `candidate_pass` 总数，包括 `effective_energy = 0` 的 Active standard pass。
- `limit` 必须在 `1..=500`；非法 limit、cursor 篡改、跨资源复用或任一绑定字段变化均返回 `INVALID_PAGINATION`。
- cursor 绑定 `view_version`、完整 `external_state`、resource、`selection_rule`、`limit` 和最后一条确定性排序 key；调用方不得解析或构造 cursor。
- 排序使用内部 `u128 effective_energy`，RPC 只输出 canonical decimal string。
- `level` 和 `difficulty_factor_bps` 按 UIP-0005 从每个 `candidate_pass` 的 `effective_energy` 运行时派生，不改变排序口径。
- active collab pass 即使拥有很高 `raw_energy`，也不能成为 `candidate_pass`。
- 若 `candidate_pass` 缺少 raw energy 记录，服务 fail closed 并返回 `INTERNAL_INVARIANT_BROKEN`。

### 19) `get_collab_breakdown`

查询某 Leader pass 在目标高度的 collab contribution 审计明细。

参数：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "leader_pass_id": "txidi0",
  "block_height": 900123,
  "context": {
    "requested_height": 900123,
    "expected_state": {
      "snapshot_id": "snapshot-...",
      "system_state_id": "system-..."
    }
  },
  "sort": "collab_pass_id_asc",
  "cursor": null,
  "limit": 100
}
```

`sort` 可选，允许：

- `collab_pass_id_asc`：按 RPC 输出的 canonical collab pass id 文本逐字节升序，默认。
- `contribution_desc_pass_id_asc`：按 contribution 降序，以 canonical pass id 文本逐字节升序打破平局。

这里的 pass id 顺序以外部 inscription-id 文本为准，不使用内部 txid byte order。

首次请求传 `cursor = null`，后续请求原样传回 `next_cursor`。旧 `page/page_size` 字段会被拒绝，不保留双分页兼容层。

返回：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "external_state": {
    "btc_height": 900123,
    "snapshot_id": "snapshot-...",
    "stable_block_hash": "000000...",
    "local_state_commit": "local-...",
    "system_state_id": "system-...",
    "balance_history_api_version": "1.0.0",
    "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
    "activation_registry_id": "...",
    "active_version_set": {"inscription_schema_version":"uip-0001-miner-pass-inscription:v1","pass_state_machine_version":"uip-0002-pass-state-machine:v1","energy_formula_version":"uip-0003-pass-energy-formula:v1","effective_energy_formula_version":"uip-0004-collab-leader-effective-energy:v1","level_formula_version":"uip-0005-level-and-real-difficulty:v1","query_semantics_version":"uip-0006-economic-query-semantics:v1","state_view_version":"uip-0006-usdb-economic-state-view:v1","commit_protocol_version":"uip-0008-usdb-local-state-commit:v1","balance_history_semantics_version":"balance-snapshot-at-or-before:v1"},
    "active_version_set_id": "01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691"
  },
  "leader_pass_id": "txidi0",
  "leader_state": "active",
  "leader_pass_kind": "standard",
  "sort": "collab_pass_id_asc",
  "total": 2,
  "aggregate_collab_contribution": "500000",
  "limit": 100,
  "max_limit": 500,
  "next_cursor": null,
  "items": [
    {
      "collab_pass_id": "txidi1",
      "collab_owner_script_hash": "<BtcScriptHash>",
      "collab_owner_btc_addr": null,
      "record_block_height": 900120,
      "collab_raw_energy": "1000000",
      "collab_weight_bps": 5000,
      "collab_contribution": "500000",
      "leader_ref_kind": "leader_pass_id",
      "leader_ref_value": "txidi0"
    }
  ]
}
```

补充语义：

- `context` 校验语义与 `get_pass_energy` 一致。
- `view_version` 必填，并按 UIP-0006 的 view contract 校验。
- 响应的 `external_state` 是目标高度的完整历史 identity，不使用 current head 或当前常量覆盖历史 protocol/formula version。
- 服务在派生 breakdown 前后重建并比较完整 state ref；若期间发生同高度 reorg，返回对应 state mismatch。
- cursor 绑定完整 `external_state`、`leader_pass_id`、`sort`、`limit` 和最后一条确定性排序 key；非法 limit、篡改或绑定不一致返回 `INVALID_PAGINATION`。
- `aggregate_collab_contribution` 是该高度完整 breakdown 的全量 aggregate，不只限当前页。
- 下游可以遍历所有分页并重算 aggregate。
- 当前实现没有 script hash -> BTC address 反查索引，因此 `collab_owner_btc_addr` 为 `null`。

---

## 5.4 活跃地址余额快照

### 20) `get_active_balance_snapshot`

查询指定高度快照（精确高度）。

参数：

```json
{
  "block_height": 900123
}
```

返回：`ActiveBalanceSnapshot`。

错误：

- 若 `block_height > synced_block_height`，返回共享共识错误 `HEIGHT_NOT_SYNCED`
- 若高度合法，但该高度没有 exact active balance snapshot，返回共享共识错误 `NO_RECORD`

### 21) `get_latest_active_balance_snapshot`

查询最近一次已落库快照。

---

## 5.5 管理

### 22) `stop`

触发索引器优雅停止（建议默认仅 localhost 可访问）。

---

## 6. 错误码

### 6.1 共享共识错误（跨服务）

当前 `usdb-indexer` 已开始接入共享共识错误契约：

- `-32040 HEIGHT_NOT_SYNCED`
- `-32041 SNAPSHOT_NOT_READY`
- `-32042 SNAPSHOT_ID_MISMATCH`
- `-32043 BLOCK_HASH_MISMATCH`
- `-32044 VERSION_MISMATCH`
- `-32045 LOCAL_STATE_COMMIT_MISMATCH`
- `-32046 SYSTEM_STATE_ID_MISMATCH`
- `-32047 NO_RECORD`
- `-32048 STATE_NOT_RETAINED`
- `-32049 HISTORY_NOT_AVAILABLE`
- `-32050 VIEW_VERSION_MISMATCH`
- `-32052 FORMULA_VERSION_MISMATCH`
- `-32053 ACTIVATION_RECORD_NOT_FOUND`
- `-32054 ACTIVATION_RECORD_CONFLICT`
- `-32055 VERSION_NOT_SUPPORTED`
- `-32056 ACTIVE_VERSION_SET_MISMATCH`
- `-32057 COMMIT_PROTOCOL_VERSION_MISMATCH`

旧 `-32051 PROTOCOL_VERSION_MISMATCH` 已随全局 `usdb_index_protocol_version` 删除；开发期不保留兼容映射，也不复用该码位。

这些错误会携带结构化 `data`，包含：

- `service`
- `requested_height`
- `local_synced_height`
- `upstream_stable_height`
- `consensus_ready`
- `expected_state`
- `actual_state`
- `mismatch_field`

示例：

```json
{
  "code": -32041,
  "message": "SNAPSHOT_NOT_READY",
  "data": {
    "service": "usdb-indexer",
    "requested_height": null,
    "local_synced_height": 900123,
    "upstream_stable_height": 900123,
    "consensus_ready": false,
    "actual_state": {
      "snapshot_id": null,
      "local_state_commit": null,
      "system_state_id": null
    },
    "detail": "No adopted upstream snapshot anchor available"
  }
}
```

当前历史校验相关接口已经补齐这类共享错误：

- `STATE_NOT_RETAINED`
- `HISTORY_NOT_AVAILABLE`

它们的当前语义是：

- `STATE_NOT_RETAINED`
  - 请求高度本身合法
  - 但已低于当前统一历史保留窗口下界（现阶段即 `genesis_block_height`）
- `HISTORY_NOT_AVAILABLE`
  - 请求高度仍在保留窗口内
  - 但节点当前缺少重建该高度历史 state ref 所需的辅助数据

这类情况不能混成 `*_MISMATCH`，否则 ETHW 验块会把“服务没有这份历史数据”误判成“区块记录的状态错误”。

### 6.2 业务层错误

除标准 JSON-RPC 错误外，建议统一扩展：

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
  "code": -32011,
  "message": "PASS_NOT_FOUND",
  "data": {
    "inscription_id": "txidi0",
    "resolved_height": 900123
  }
}
```

---

## 7. 无歧义约束清单（实现必须遵守）

1. 高度查询全部采用 `<= h` 的包含边界语义。
2. 普通高度查询返回 `resolved_height`；UIP-0006 economic view 改由完整 `external_state` 承载目标高度和历史 identity。
3. 分页查询必须固定排序，且文档公开排序键。UIP-0006 cursor 还必须绑定完整 external state 和 continuation key。
4. 所有列表接口返回顺序必须稳定可重放。
5. `owner_active_pass` 发现重复活跃 owner 必须报错，不可“取第一条”。  
6. `invalid` pass 必须可查到 `invalid_code` 与 `invalid_reason`。  
7. 业务错误码必须稳定，不得随意复用文案替代错误码。  

---

## 8. 当前完成度与后续阶段

### 8.1 当前已完成

当前仓库已经完成或落到第一版的接口包括：

- `get_rpc_info`
- `get_network_type`
- `get_sync_status`
- `get_synced_block_height`
- `get_snapshot_info`
- `get_local_state_commit_info`
- `get_system_state_info`
- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_active_passes_at_height`
- `get_pass_stats_at_height`
- `get_pass_energy`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- `get_active_balance_snapshot`
- `get_latest_active_balance_snapshot`

其中与 ETHW 强一致历史校验直接相关的主链路已经具备：

- `get_state_ref_at_height`
- `get_pass_snapshot(context=...)`
- `get_pass_energy(context=...)`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- `STATE_NOT_RETAINED / HISTORY_NOT_AVAILABLE / *_MISMATCH`

### 8.2 后续阶段

后续更偏增强项而不是协议缺口：

- `get_pass_history`
- `get_owner_active_pass_at_height`
- `get_pass_energy_range`
- `get_pass_energy_leaderboard`
- `get_invalid_passes`
- `stop`

以及：

- 将 `ConsensusQueryContext` 继续扩展到更多外围查询面
- 若未来引入真实 prune，再把统一下界演进成真实 retention floor
- 推进更贴近 ETHW 最终 block body 的 validator 联调

---

## 9. 与当前实现的映射（便于开发）

- `miner_passes`：静态字段主表（含 `invalid_code/reason`）  
- `miner_pass_state_history`：历史事件与高度快照来源  
- `active_balance_snapshots`：活跃地址总余额快照  
- `pass_energy`（RocksDB）：能量记录  

建议默认以 `history` 作为状态口径，避免“当前状态污染历史重放”的歧义。
