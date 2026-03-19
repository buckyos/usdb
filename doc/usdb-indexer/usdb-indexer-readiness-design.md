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
  - 表示当前状态已经满足严格共识消费条件
  - 这是脚本和下游系统应该等待的状态

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

这里特别强调一点：`upstream_reorg_recovery_pending_height` 必须参与 readiness 计算，这样即使进程重启，服务也不会因为内存态丢失而错误地重新报告 ready。

## 4. 阻塞条件

第一版 blocker 定义如下：

- `RpcNotListening`
- `ShutdownRequested`
- `SyncedHeightMissing`
- `CatchingUp`
- `UpstreamReadinessUnknown`
- `UpstreamConsensusNotReady`
- `UpstreamSnapshotMissing`
- `UpstreamSnapshotHeightMismatch`
- `ReorgRecoveryPending`
- `LocalStateCommitMissing`
- `SystemStateMissing`

这些 blocker 都是结构化枚举，脚本和下游不需要依赖自由文本 message 推断语义。

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

这样能把下面这些危险窗口显式挡住：

- RPC 已可访问，但本地还没 durable 到任何高度
- 上游已经有新 stable snapshot，但本地还在追块
- rollback 已经开始，但 reorg recovery 还没完成
- 进程正在 drain/shutdown

## 6. 与现有 RPC 的关系

这套设计不替代已有接口，而是把它们组织起来：

- `get_sync_status`
  - 继续提供进度视图
- `get_snapshot_info`
  - 继续表示 adopted upstream snapshot
- `get_local_state_commit_info`
  - 表示本地核心 durable 状态
- `get_system_state_info`
  - 表示给下游消费的顶层系统状态
- `get_readiness`
  - 负责把这些状态组合成“现在能不能用”

因此下游应遵循：

1. 先等 `get_readiness.consensus_ready = true`
2. 再读取 `snapshot_info / local_state_commit_info / system_state_info`

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
   - 这些脚本现在会在关键断言前等待 `regtest_wait_usdb_consensus_ready`

## 8. 后续可扩展项

第一版还没有做的内容：

- 对 `world-sim` / `e2e_smoke` 全部入口统一切换到 `get_readiness`
- 在 regtest 中显式断言 `consensus_ready=false` 的中间窗口
- 把 `blockers` 进一步细分成 `query_blockers` / `consensus_blockers`

当前阶段先把主链路拉直：服务端、单元测试、reorg smoke 和 restart/recovery 语义先统一到结构化 readiness 上。
