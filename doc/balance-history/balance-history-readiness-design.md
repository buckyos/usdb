# Balance-History Readiness 设计说明

本文档定义 `balance-history` 第一版 readiness contract，目标是把“RPC 可访问”和“快照可用于共识”明确拆开，并为后续 `usdb-indexer / ETHW` 的严格 gating 提供统一语义。

## 背景

当前系统已经存在：

1. 进程级 liveness：服务端口可连接，请求能收到响应。
2. 同步进度状态：`get_sync_status` 返回 `phase/current/total/message`。
3. 稳定快照元信息：`get_snapshot_info` 返回 `stable_height / stable_block_hash / latest_block_commit`。

但这三者还没有形成一个统一、可测试、可直接消费的 readiness contract。特别是：

1. `get_network_type` 可调用只能说明 RPC 活着，不能说明快照已经适合下游共识消费。
2. reorg rollback / rollback resume / shutdown 等中间态需要显式把 readiness 拉低。
3. `phase/message` 是人类可读状态，不适合作为协议层 gating 依据。

## 目标

第一版 readiness contract 要回答三个不同问题：

1. `rpc_alive`
   - 服务是否已经监听 RPC 并能收发请求。
2. `query_ready`
   - 服务是否处于适合普通 DB 查询的状态。
3. `consensus_ready`
   - 当前 stable snapshot 是否已经完整且可被下游当作共识输入使用。

其中 `consensus_ready` 必须比 `rpc_alive` 更严格。

## 第一版接口

新增 RPC：

- `get_readiness`

返回结构：

- `service`
- `rpc_alive`
- `query_ready`
- `consensus_ready`
- `phase`
- `current`
- `total`
- `message`
- `stable_height`
- `stable_block_hash`
- `latest_block_commit`
- `blockers`

其中 `blockers` 为 machine-readable 的阻塞原因列表，第一版包括：

- `RpcNotListening`
- `Initializing`
- `Loading`
- `CatchingUp`
- `RollbackInProgress`
- `ShutdownRequested`
- `StableBlockHashMissing`
- `LatestBlockCommitMissing`

## 判定规则

### 1. rpc_alive

仅表示：

1. RPC server 已经 start 成功；
2. 还没有进入 close 完成后的状态。

### 2. query_ready

第一版规则：

1. `rpc_alive = true`
2. 不处于 `Initializing`
3. 不处于 `Loading`
4. 不处于 `RollbackInProgress`
5. 不处于 `ShutdownRequested`

这意味着：

1. 正在 catch-up 的 `Indexing` 阶段可以 `query_ready = true`
2. 但不代表它已经 `consensus_ready = true`

### 3. consensus_ready

第一版规则：

1. `query_ready = true`
2. 当前 `current >= total`
3. `stable_block_hash` 存在
4. `latest_block_commit` 存在

也就是说，只有当：

1. 服务不在初始化/加载/回滚/关停中；
2. 当前已经追平本轮 stable 目标高度；
3. 当前 stable snapshot 的关键元信息完整；

才允许下游将其用于共识消费。

## 动态更新要求

readiness 不是启动时一次性计算的静态值，而是 live state。

第一版要求即时更新以下事件：

1. RPC server start 成功：
   - `rpc_alive = true`
2. RPC server close 完成：
   - `rpc_alive = false`
3. 收到 shutdown 请求：
   - `shutdown_requested = true`
4. 检测到并执行 rollback：
   - `rollback_in_progress = true`
5. rollback/resume 成功完成：
   - `rollback_in_progress = false`

## 为什么第一版不直接替换 get_sync_status

`get_sync_status` 仍然保留，因为它适合：

1. 控制台进度展示
2. 运维观察同步进度
3. 人工排障

而 `get_readiness` 的目标是：

1. 提供 machine-readable gating contract
2. 服务于脚本、测试框架、下游共识消费者

两者职责不同。

## 第一版范围边界

第一版只覆盖 `balance-history`：

1. 不直接定义 `usdb-indexer` 的 readiness 细则
2. 不引入 ETHW 侧消费规则
3. 不在这一版加入复杂版本白名单逻辑

后续扩展方向：

1. `usdb-indexer` 增加同名 `get_readiness`
2. 统一 `query_ready / consensus_ready` 的跨服务判定字段
3. regtest 框架从 `get_network_type` 切换到 readiness gating
