# BTC Consensus RPC 错误契约设计

## 1. 目标

这份设计不是要把 `balance-history` 和 `usdb-indexer` 的所有 RPC 错误揉成同一套，而是要定义一层 **跨服务、可被 ETHW 或其他下游直接依赖的共识错误契约**。

当前两个服务各自已有本地错误码：

- `balance-history` 主要仍以 `InternalError / InvalidParams` 为主
- `usdb-indexer` 已有 `HEIGHT_NOT_SYNCED / PASS_NOT_FOUND / ENERGY_NOT_FOUND` 等业务错误

问题在于：

- 下游无法用统一方式判断“当前快照不可用”还是“业务对象不存在”
- 同一类快照/版本/锚点问题在不同服务上没有统一数值和结构化 `data`
- 错误字符串仍然过于依赖人工解析，不适合共识层自动处理

因此需要补一层共享契约。

## 2. 分层原则

错误面分成三层：

### 2.1 传输层错误

继续沿用 JSON-RPC 标准：

- `InvalidParams`
- `MethodNotFound`
- `InternalError`

这一层不做跨服务自定义。

### 2.2 共识契约错误

这是本设计要统一的层，只覆盖“快照是否可用于下游共识”的问题。

第一版共享错误集合：

- `HEIGHT_NOT_SYNCED`
- `SNAPSHOT_NOT_READY`
- `SNAPSHOT_ID_MISMATCH`
- `BLOCK_HASH_MISMATCH`
- `VERSION_MISMATCH`
- `LOCAL_STATE_COMMIT_MISMATCH`
- `SYSTEM_STATE_ID_MISMATCH`
- `NO_RECORD`
- `STATE_NOT_RETAINED`
- `HISTORY_NOT_AVAILABLE`

其中：

- `HISTORY_NOT_AVAILABLE` 已开始用于“高度合法，但该节点当前无法重建所需历史 state ref”的场景
- `STATE_NOT_RETAINED` 已用于“高度低于节点当前承诺保留的历史窗口下界”的场景

### 2.3 业务域错误

继续保留服务各自的业务错误，不强行统一，例如：

- `PASS_NOT_FOUND`
- `ENERGY_NOT_FOUND`
- `INVALID_PAGINATION`
- `INVALID_HEIGHT_RANGE`

这类错误不属于共识锚点或快照契约本身。

## 3. 共享错误码区间

当前统一放在 `-32040..-32049`：

| Name | Code | 含义 |
| --- | ---: | --- |
| `HEIGHT_NOT_SYNCED` | `-32040` | 请求高度超出当前 durable/stable 高度 |
| `SNAPSHOT_NOT_READY` | `-32041` | 服务活着，但当前快照还不能用于共识 |
| `SNAPSHOT_ID_MISMATCH` | `-32042` | 调用方预期的 snapshot id 与当前不一致 |
| `BLOCK_HASH_MISMATCH` | `-32043` | 调用方预期的 stable BTC block hash 与当前不一致 |
| `VERSION_MISMATCH` | `-32044` | 语义版本或协议版本与调用方预期不一致 |
| `LOCAL_STATE_COMMIT_MISMATCH` | `-32045` | 调用方预期的 local state commit 与当前不一致 |
| `SYSTEM_STATE_ID_MISMATCH` | `-32046` | 调用方预期的 system state id 与当前不一致 |
| `NO_RECORD` | `-32047` | 查询合法、范围合法，但该对象/键没有记录 |
| `STATE_NOT_RETAINED` | `-32048` | 查询高度已落到该节点明确的历史保留窗口之外 |
| `HISTORY_NOT_AVAILABLE` | `-32049` | 查询高度合法，但该节点当前无法重建所需历史状态 |

## 4. 共享请求上下文

仅有错误码还不够，必须同时标准化“调用方期待的快照上下文”。

第一版统一结构：

- `ConsensusQueryContext`
  - `requested_height`
  - `expected_state`

其中 `expected_state` 是可选 selector 集合：

- `snapshot_id`
- `stable_height`
- `stable_block_hash`
- `balance_history_api_version`
- `balance_history_semantics_version`
- `usdb_index_protocol_version`
- `local_state_commit`
- `system_state_id`

设计原则：

- 这是 **宽结构**，不是要求每个服务一次性支持全部字段
- 调用方只填自己真正要 pin 的字段
- 服务只比对自己有语义能力回答的字段

## 5. 共享错误数据结构

第一版统一 `data` 结构：

- `service`
- `requested_height`
- `local_synced_height`
- `upstream_stable_height`
- `consensus_ready`
- `expected_state`
- `actual_state`
- `detail`

这样做的目的：

- 下游不需要 parse 人类可读字符串
- 可以直接区分“没 ready”与“真实 mismatch”
- 可以把期望值和实际值一起记录下来，方便审计

## 6. 语义边界

### 6.0 `requested_height` 约束的是历史状态，不是当前 head

`ConsensusQueryContext.requested_height` 的语义必须明确为：

- 调用方要查询的是 **高度 `H` 对应的历史状态**
- 不是“当前服务 head 恰好也在 `H` 时的瞬时状态”

这一区分对 ETHW 验块尤其关键。

典型场景：

1. 矿工 A 在 BTC 侧 `height = H` 时查询到：
   - `snapshot_id`
   - `system_state_id`
   - 自己 pass 的 `energy/pass info`
2. 矿工 A 产出 ETHW 区块，并把 `(H, snapshot_id, system_state_id, pass info)` 写入区块。
3. 其他矿工在稍后收到该 ETHW 区块时，BTC 侧可能已经前进到 `H+1` 甚至 `H+2`。
4. 验证方此时必须基于 **高度 `H` 的历史状态** 重查对应 pass 信息，而不是用当前 head 状态直接比对。

因此：

- “当前 head 已经前进”本身 **不构成** `SNAPSHOT_ID_MISMATCH / SYSTEM_STATE_ID_MISMATCH`
- mismatch 只在“服务能够重建高度 `H` 的历史状态，但重建结果与区块里记录的 state ref 不一致”时成立

换句话说：

- `current_head != requested_height` 是正常现象
- `historical_state_at(requested_height) != expected_state_ref` 才是真正的 mismatch

### 6.1 `HEIGHT_NOT_SYNCED` 与 `NO_RECORD` 必须区分

这两个最容易混淆：

- `HEIGHT_NOT_SYNCED`
  - 请求高度超出了当前 stable/durable 范围
- `NO_RECORD`
  - 请求高度合法，但该地址/该 pass/该对象在此范围内没有记录

对于 `balance-history` 的 `at-or-before` 语义尤其重要，不能再把“没同步到”和“结果为零/无记录”混成一类。

### 6.2 `SNAPSHOT_NOT_READY` 与 mismatch 也必须区分

- `SNAPSHOT_NOT_READY`
  - 当前根本还不应该被下游消费
- `SNAPSHOT_ID_MISMATCH / BLOCK_HASH_MISMATCH / VERSION_MISMATCH`
  - 服务已经 ready，但和调用方 pin 的预期不一致

### 6.3 已区分“无法重建历史状态”与“历史状态不匹配”

当前实现里，这两类情况已经分开：

- `STATE_NOT_RETAINED`
  - 高度已经落到节点当前承诺保留的历史窗口之外
- `HISTORY_NOT_AVAILABLE`
  - 高度仍然合法且在保留窗口内，但节点当前缺少构造该历史 state ref 所需的辅助数据

这两类错误都不应再混成：

- `HEIGHT_NOT_SYNCED`
- 或 `*_MISMATCH`

当前第一版的保留窗口下界已经在 `usdb-indexer` 中落成统一规则：

- `retention_floor = genesis_block_height`

这还是一个简化实现，不代表最终一定长期使用 `genesis_block_height` 作为唯一 retention floor；但从 ETHW 验块语义看，错误分类已经独立成立。

### 6.4 ETHW 验块要求服务能回答“高度 H 的 state ref 是什么”

这部分在当前阶段已经具备第一版实现。

当前系统已有这些状态对象：

- `snapshot_id`
- `local_state_commit`
- `system_state_id`

同时也已经区分了两类接口：

- current-head introspection
  - `get_snapshot_info`
  - `get_local_state_commit_info`
  - `get_system_state_info`
- historical state-ref 查询
  - `balance-history.get_state_ref_at_height`
  - `usdb-indexer.get_state_ref_at_height`

也就是说，当前服务已经不再只能回答：

- “服务当前 head 对应的状态是什么”

而是已经能回答：

- “高度 `H` 对应的历史 `snapshot_id / local_state_commit / system_state_id` 是什么”

这正是 ETHW 验块真正需要的能力。

典型流程：

1. 矿工 A 在 BTC 高度 `H` 时读取：
   - `snapshot_id`
   - `system_state_id`
   - 自己 pass 的 `energy/pass info`
2. 矿工 A 产出 ETHW 区块，并把 `(H, snapshot_id, system_state_id, pass info)` 固定进区块。
3. 其他矿工稍后校验该区块时，BTC 头部可能已经前进到 `H+1` 甚至更高。
4. 校验方仍然需要查询 **高度 `H` 的历史 state ref**，再在这份历史状态下复查 pass 信息。

因此当前阶段的协议边界已经变成：

- 验证方不应直接读取当前 head 状态来校验旧块
- 应先读取或复查高度 `H` 的历史 `state ref`
- 再在这份固定历史上下文下查询 `pass snapshot / pass energy`

### 6.5 历史 state ref 查询的判定顺序

对未来的历史 `state ref` 查询 RPC，建议统一采用以下判定顺序：

1. `requested_height > 当前 durable/stable 范围`
   - 返回 `HEIGHT_NOT_SYNCED`
2. `requested_height` 合法，但该高度历史 state ref 已不可重建或未保留
   - 返回未来扩展错误，例如 `STATE_NOT_RETAINED` / `HISTORY_NOT_AVAILABLE`
3. 能重建高度 `H` 的历史 state ref，但与调用方 `expected_state` 不一致
   - 返回 `SNAPSHOT_ID_MISMATCH` / `LOCAL_STATE_COMMIT_MISMATCH` / `SYSTEM_STATE_ID_MISMATCH`
4. 能重建且一致
   - 返回成功结果

这样可以避免把“服务没有这份历史数据”误判成“状态不匹配”。

## 7. 当前完成度

截至当前仓库状态，这份设计的第一阶段已经基本完成，主要落地内容包括：

1. 公共协议层
   - `usdb-util` 已固定：
     - `ConsensusRpcErrorCode`
     - `ConsensusQueryContext`
     - `ConsensusStateReference`
     - `ConsensusRpcErrorData`
2. `balance-history`
   - 当前态共识错误契约已接入：
     - `get_balance`
     - `get_balances`
     - `get_snapshot_info`
   - 已实现：
     - `get_state_ref_at_height`
3. `usdb-indexer`
   - 当前态共识错误契约已接入：
     - `get_snapshot_info`
     - `get_local_state_commit_info`
     - `get_system_state_info`
     - `get_active_balance_snapshot`
   - 已实现：
     - `get_state_ref_at_height`
     - `get_pass_snapshot(context=...)`
     - `get_pass_energy(context=...)`

因此这份协议现在已经不只是“冻结类型和码位”，而是已经支撑 ETHW 风格的历史状态校验主路径。

## 8. 当前测试覆盖

当前仓库中，这块能力已经有单测和 regtest 两层覆盖。

### 8.1 单元测试

- `balance-history`
  - `get_state_ref_at_height` 成功路径
  - future height -> `HEIGHT_NOT_SYNCED`
  - 缺 block commit -> `HISTORY_NOT_AVAILABLE`
  - `SNAPSHOT_ID_MISMATCH / BLOCK_HASH_MISMATCH / VERSION_MISMATCH`
- `usdb-indexer`
  - `get_state_ref_at_height` 成功路径
  - `SNAPSHOT_ID_MISMATCH / LOCAL_STATE_COMMIT_MISMATCH / SYSTEM_STATE_ID_MISMATCH / VERSION_MISMATCH`
  - `get_pass_snapshot(context=...)`
  - `get_pass_energy(context=...)`
  - `STATE_NOT_RETAINED`
  - `HISTORY_NOT_AVAILABLE`

### 8.2 Regtest / integration

已落地的专项脚本包括：

- `live ord` 历史校验 reorg：
  - [regtest_live_ord_historical_validation_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_historical_validation_reorg.sh)
- `live ord` 历史校验 floor/restart：
  - [regtest_live_ord_historical_validation_floor_restart.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_historical_validation_floor_restart.sh)
- `live ord` 历史校验 history-not-available：
  - [regtest_live_ord_historical_validation_history_not_available.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_historical_validation_history_not_available.sh)
- validator 风格 historical-context e2e：
  - [regtest_live_ord_validator_historical_context_e2e.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_historical_context_e2e.sh)

这些场景已经覆盖：

- BTC head 前进后，旧块仍可按历史 context 校验
- same-height reorg 后，旧 context 返回稳定的 `*_MISMATCH`
- 历史窗口上升后，返回 `STATE_NOT_RETAINED`
- 历史辅助数据缺失时，返回 `HISTORY_NOT_AVAILABLE`

## 9. 剩余边界与后续阶段

虽然第一阶段已经基本完成，但当前仍有几项明确的后续边界：

1. `STATE_NOT_RETAINED` 仍是简化实现
   - 当前统一下界是 `genesis_block_height`
   - 未来如果引入真实 prune，需要演进成真实 retention floor 机制
2. 当前 validator-style e2e 仍是 BTC 侧模拟 payload
   - 还不是 ETHW 链内真实 block body / validator 实现的最终联调
3. `ConsensusQueryContext` 目前已接入最关键的 `pass snapshot / pass energy`
   - 但还没有扩展到所有外围查询面
4. 还没有真实 prune 后的长期回归矩阵
   - 当前只覆盖了“窗口上升语义”和“辅助数据缺失语义”

## 10. 当前结论

当前仓库已经具备：

- `readiness`
- `snapshot_id`
- `local_state_commit`
- `system_state_id`
- historical `state ref`
- `ConsensusQueryContext`
- 统一共识错误面

因此对“ETHW 需要强一致地回答高度 `H` 的状态是什么，并据此校验旧块”这个目标来说，
当前 BTC 侧协议第一阶段已经基本完成。

后续工作重点不再是“有没有这层协议能力”，而是：

- 把 retention/prune 机制从简化版推进到真实实现
- 把 validator-style payload 推进到更贴近 ETHW 最终块格式的联调
