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
- `STATE_NOT_RETAINED` 仍是保留码位，等待 retention floor 元数据落地后再真正启用

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

### 6.3 还需要区分“无法重建历史状态”与“历史状态不匹配”

对 ETHW 验块来说，将来还需要再区分一类情况：

- 服务当前已经不再保留或无法重建 `requested_height = H` 的历史 state ref

这类错误既不是：

- `HEIGHT_NOT_SYNCED`
- 也不是 `*_MISMATCH`

更接近：

- `STATE_NOT_RETAINED`
- 或 `HISTORY_NOT_AVAILABLE`

本设计第一阶段先不把它编码进共享错误码集合，但后续开发历史 `state ref` 查询 RPC 时，应把这类错误单独拉出来，而不是混成 mismatch。

### 6.4 ETHW 验块要求服务能回答“高度 H 的 state ref 是什么”

当前第一阶段已经有这些状态对象：

- `snapshot_id`
- `local_state_commit`
- `system_state_id`

但当前接口仍然主要回答 **current-head view**：

- `get_snapshot_info`
- `get_local_state_commit_info`
- `get_system_state_info`

它们能够回答：

- “服务当前 head 对应的状态是什么”

却还不能直接回答：

- “高度 `H` 对应的历史 `snapshot_id / local_state_commit / system_state_id` 是什么”

而 ETHW 验块真正需要的是后者。

典型流程：

1. 矿工 A 在 BTC 高度 `H` 时读取：
   - `snapshot_id`
   - `system_state_id`
   - 自己 pass 的 `energy/pass info`
2. 矿工 A 产出 ETHW 区块，并把 `(H, snapshot_id, system_state_id, pass info)` 固定进区块。
3. 其他矿工稍后校验该区块时，BTC 头部可能已经前进到 `H+1` 甚至更高。
4. 校验方仍然需要查询 **高度 `H` 的历史 state ref**，再在这份历史状态下复查 pass 信息。

因此后续必须补一层“历史 state ref 查询”能力，而不是让验证方直接读取当前 head 状态。

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

## 7. 第一阶段实施范围

本阶段只做两件事：

1. 在 `usdb-util` 中固定共享类型：
   - `ConsensusRpcErrorCode`
   - `ConsensusQueryContext`
   - `ConsensusStateReference`
   - `ConsensusRpcErrorData`
2. 不立即改写所有 RPC 行为，只让公共协议先冻结下来

这样可以先把：

- 错误码区间
- 错误名
- `data` 结构
- 请求上下文结构

固定下来，再分阶段把 `balance-history` 和 `usdb-indexer` 的具体方法切过去。

## 8. 下一阶段建议

### Phase 2

先落在最小共识面：

- `balance-history`
  - `get_balance`
  - `get_balances`
  - `get_snapshot_info`
- `usdb-indexer`
  - `get_snapshot_info`
  - `get_local_state_commit_info`
  - `get_system_state_info`
  - `get_active_balance_snapshot`

同时冻结历史 `state ref` 查询设计，至少要支持：

- 在 `requested_height = H` 上重建并返回 `snapshot_id / local_state_commit / system_state_id`
- 让 ETHW 验块使用“区块声明的历史 state ref”进行校验，而不是读取当前 head 状态

当前代码已经开始落第一版：

- `balance-history.get_state_ref_at_height`
- `usdb-indexer.get_state_ref_at_height`

当前仓库已经落到第二步：

- 已支持“查询高度 `H` 的历史 state ref”这个基础能力
- 已支持历史 `state ref` 查询上基于 `expected_state` 的 `*_MISMATCH` 校验
- 已开始把“高度合法但历史辅助数据缺口”的路径收敛成 `HISTORY_NOT_AVAILABLE`
- `STATE_NOT_RETAINED` 仍待 retention floor 元数据落地后启用

### Phase 3

在 `balance-history / usdb-indexer` 中逐步实现：

- retention floor 元数据
- `STATE_NOT_RETAINED` 的真实返回路径
- 将 `ConsensusQueryContext` 继续扩展到 ETHW 真正依赖的 pass / energy 查询
- 增加针对历史 state ref 校验的 regtest / integration coverage

并补共享 helper：

- 从 `ConsensusRpcErrorCode` 构造 JSON-RPC error
- 从 `ConsensusQueryContext` 做 selector 校验
- 统一负向测试和 regtest

## 9. 当前状态

当前仓库已完成：

- `readiness`
- `snapshot_id`
- `local_state_commit`
- `system_state_id`

因此现在补统一错误契约，已经有比较明确的状态对象可绑定，不需要再回到“只靠高度和自由文本 message”那种弱契约。
