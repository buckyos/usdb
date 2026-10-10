# USDB-Indexer Readiness 设计说明

本文定义 `usdb-indexer` 第一版 readiness contract，目标是把“RPC 可访问”和“可用于下游共识消费”拆开，避免测试脚本或下游服务再把 `get_network_type` 之类的简单探活误当成 ready。

## 1. 设计目标

`usdb-indexer` 的 ready 不是启动阶段的一次性锁存，而是一个运行时动态状态：

- 初始化时可能尚未 ready
- 正常追块时会在 `query_ready` 和 `consensus_ready` 之间逐步收敛
- reorg、rollback、pending recovery、shutdown 中间态都必须即时拉低 readiness

第一版的目标是回答三类问题：

1. 进程和 RPC 是否活着
2. 本地查询是否允许继续服务
3. 当前系统状态是否已经完整到可以给 ETHW / 下游共识侧消费

## 2. 三层语义

`get_readiness` 返回三层布尔状态：

- `rpc_alive`
  - 只表示 RPC listener 已经起来
  - 这是纯 liveness，不代表数据已经完整
- `query_ready`
  - 表示本地 durable 状态已经可用于普通查询
  - 允许上游暂时未 ready，但不允许本地处于 reorg recovery / shutdown 中间态
- `consensus_ready`
  - 表示当前状态已满足全局追平、历史覆盖和安全条件
  - 启动编排和同步验收等待此状态；指定已提交高度的共识查询单独判断，见 5.4

补充字段 `committed_query_ready` 表示当前已提交 head 可供 context 查询使用，允许正常追块和
下一块的向前写入。该字段不代替具体请求的高度、历史保留范围和 identity 检查。

## 3. 依赖的状态来源

第一版 `usdb-indexer` readiness 由以下几类状态共同决定：

1. 运行态标志
   - `rpc_alive`
   - `upstream_reorg_recovery_pending`
   - `shutdown_requested`
2. 上游状态缓存
   - `balance-history.get_readiness`
   - `balance-history.get_snapshot_info`
3. 本地 durable 状态
   - `synced_block_height`
   - `adopted upstream snapshot anchor`
   - `local_state_commit_info`
   - `system_state_info`
4. durably persisted recovery marker
   - `upstream_reorg_recovery_pending_height`
5. 历史 anchor 的连续覆盖位置
   - `snapshot_history_start_height` 与 `snapshot_history_next_height` 持久化在 SQLite `state`
   - `next_height` 是从配置索引起点开始的第一个缺口，不能用历史表的最大高度代替
   - 启动时从实际历史行重新核对位置；新块和回填事务负责增量推进，reorg 事务负责回退

这里特别强调一点：`upstream_reorg_recovery_pending_height` 必须参与 readiness 计算，这样即使进程重启，服务也不会因为内存态丢失而错误地重新报告 ready。

## 4. 阻塞条件

第一版 blocker 定义如下：

- `RpcNotListening`
- `ShutdownRequested`
- `SyncedHeightMissing`
- `CatchingUp`
- `HistoryBackfillPending`
- `BlockProcessingPending`
- `UpstreamReadinessUnknown`
- `UpstreamConsensusNotReady`
- `UpstreamSnapshotMissing`
- `UpstreamSnapshotHeightMismatch`
- `ReorgRecoveryPending`
- `LocalStateCommitMissing`
- `SystemStateMissing`

这些 blocker 都是结构化枚举，脚本和下游不需要依赖自由文本 message 推断语义。

P6.4 新增 `block_processing_pending_height` 运行时标记：开始处理时设置，保存该块和 synced height 的事务提交后清除。
数据查询或其他块处理失败时保持 `BlockProcessingPending`；恢复核对后已无需处理该块也可清除。
该标记阻止瞬时或失败状态被宣称为全局共识就绪；正常向前执行时可以查询已提交前缀，见 5.4。
重启仍以持久化高度、
能量/reorg恢复和历史锚点完整性为准。Core 单块输入能力预检和后台验证进度不是本服务的 consensus readiness，
见[P6.4操作](../balance-history/balance-history-assumeutxo-p64-operations.md)。

## 5. 判定规则

### 5.1 `query_ready`

第一版使用较宽松规则：

- `rpc_alive = true`
- `shutdown_requested = false`
- `upstream_reorg_recovery_pending = false`
- `synced_block_height` 已存在

也就是说，`query_ready` 允许节点在“本地状态完整，但上游尚未 consensus ready”时继续服务本地查询。

### 5.2 `consensus_ready`

第一版使用严格规则：

- `query_ready = true`
- 上游 `balance-history.consensus_ready = true`
- 本地没有 `CatchingUp`
- adopted upstream snapshot anchor 存在
- `local_state_commit_info` 可生成
- `system_state_info` 可生成
- 没有任何 blocker
- `snapshot_history_pending_from = null`，即所需历史 anchor 已连续覆盖本地已提交高度

全局就绪判定会等待以下情况结束，其中正常追块不代表旧的已提交状态不安全：

- RPC 已可访问，但本地还没 durable 到任何高度
- 上游已经有新 stable snapshot，但本地还在追块
- rollback 已经开始，但 reorg recovery 还没完成
- 进程正在 drain/shutdown
- 本地与上游高度相同、head anchor 已存在，但中间历史 anchor 尚未补齐

### 5.3 每块提交与旧库恢复

正常索引复用生成 pass block commit 时取得的 `balance-history.get_block_commit(H)`，将同一高度的
BTC hash、上游 block commit、stable lag 和协议版本写入历史 anchor。该行、连续覆盖位置和
`synced_block_height` 与业务 SQLite 状态一起在现有每块 savepoint 内提交；能量库继续使用既有跨库恢复协议。
正常追块完成后无需再逐块请求历史 state ref 来补 anchor。批次末尾的 adopted head anchor 仍单独发布。

对外读取的同步高度和历史覆盖位置使用独立只读 SQLite 连接，只观察已提交状态。writer 的 savepoint
尚未提交时，对外不能提前暴露新高度。readiness 的这两项状态来自同一次只读事务，查询开销不随历史长度增长。

旧库可能仍有“业务高度在前、历史 anchor 在后”的缺口。启动时在 RPC 监听之前重算连续覆盖位置，
索引循环在继续追块或发布新 head 之前先修复缺口。回填每批最多 64 行，在同一事务内写行并推进覆盖位置；
失败或退出只重做未提交批次。返回的历史高度、hash identity 和已有本地 pass commit 的上游锚定必须一致，
不一致时停止本轮并保留未就绪状态。

`get_readiness` 新增：

| 字段 | 含义 |
| --- | --- |
| `snapshot_history_ready_height` | 从索引起点连续具备 anchor 的最后已提交高度；首行尚缺时为 null |
| `snapshot_history_pending_from` | 已提交业务范围内的第一个缺失高度；无缺口时为 null |

回填时全局 readiness 的 blockers 包含 `HistoryBackfillPending`。指定高度的共识查询仍须检查
该高度自身的历史 anchor 和完整 identity；其他高度的覆盖缺口不单独阻断该查询。
即使双方高度完全相同，也只有连续补齐后全局 `consensus_ready` 才为 true。
详细验收见 [历史 anchor 原子提交验收](./usdb-indexer-snapshot-anchor-acceptance.md)。

### 5.4 指定已提交高度的可验证性

`get_state_ref_at_height`、带 context 的 pass snapshot/energy，以及经济视图查询允许本地和
BH 正常 `CatchingUp`。这不修改全局 `consensus_ready`：请求必须位于本地已提交范围，并具备
该高度的历史 anchor、local/system commit 和匹配的 expected state。

BH 必须仍为 `rpc_alive && query_ready`，已提交稳定高度覆盖请求高度，拥有 hash/commit，
blockers 只能是正常 `CatchingUp`。本地仍拒绝 shutdown、reorg recovery（含持久化标记）、
未知上游、缺失或不一致的当前状态身份。`BlockProcessingPending` 仅在正常向前执行、pending
高于已提交 head、能量已 finalize 到 head 且没有失败恢复时可以共存；失败留下的 pending 仍拒绝。
请求派生完成后再检查安全条件并复核历史 identity；未来高度、历史缺失或裁剪沿用结构化错误。

RPC/能量派生使用独立 SQLite 只读连接及严格的能量历史高度限制。head 使用每块提交的历史
anchor。整个状态查询持有共享读取许可，重组/恢复持有独占许可；普通向前写入不占用该许可。
恢复开始或排队时新查询快速拒绝，已进行的查询结束/拒绝后才修改历史。仍不承诺发布切换瞬间
零重试，也不替代 Go 侧延迟验证重试。

实现与后续验收计划见[已提交状态出块与延迟验证](./committed-state-mining-and-validation-plan.md)。

## 6. 与现有 RPC 的关系

这套设计不替代已有接口，而是把它们组织起来：

- `get_sync_status`
  - 继续提供进度视图
- `get_snapshot_info`
  - 表示本地已提交高度对应的 upstream snapshot，从逐块提交的历史 anchor 读取
- `get_local_state_commit_info`
  - 表示本地核心 durable 状态
- `get_system_state_info`
  - 表示给下游消费的顶层系统状态
- `get_readiness`
  - 负责把这些状态组合成“现在能不能用”

启动编排继续等待 `get_readiness.consensus_ready = true`。已运行的 miner/validator 使用指定高度
及完整 context 的查询结果判断能否使用该状态，不能仅因全局正在追块就拒绝。
节点挖矿工具仍核对候选前后 height、epoch 和 identity，真实不安全状态保持 WAITING。

## 7. 测试策略

第一版测试覆盖两层：

1. 单元测试
   - 未启动 RPC 时默认 not ready
   - 本地和上游都完整时 `consensus_ready=true`
   - `CatchingUp` 时 `consensus_ready=false`
   - `ReorgRecoveryPending` 时 `query_ready=false`
   - 上游 `consensus_ready=false` 时本地也必须 `consensus_ready=false`
2. regtest
   - `regtest_reorg_smoke.sh`
   - `regtest_same_height_reorg_smoke.sh`
   - `regtest_e2e_smoke.sh`
   - `regtest_live_ord_e2e.sh`
   - `regtest_world_sim.sh`
   - 这些入口现在都已经在关键断言前等待 `regtest_wait_usdb_consensus_ready` 或等价的 readiness helper

## 8. 后续可扩展项

当前仍未完成的内容：

- 在 regtest 中显式断言 `consensus_ready=false` 的中间窗口
- 把 `blockers` 进一步细分成 `query_blockers` / `consensus_blockers`
- 把 readiness 的负向断言扩展到更多 restart / world-sim / historical validation 场景

当前阶段主链路已经拉直：服务端、单元测试、smoke、live ord、world-sim、reorg/restart/recovery 语义都已统一到结构化 readiness 上。
