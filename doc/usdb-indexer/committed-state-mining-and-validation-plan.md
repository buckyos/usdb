# 已提交 BTC 状态的出块与延迟验证改进

状态：第一阶段已实现并通过本地回归，待评审与发布验证。后续阶段尚未实现，不代表已经通过多节点实测或发布。

## 问题与目标

依赖链为 USDB chain → usdb-indexer → balance-history → Bitcoin Core。BH 的
`stable_lag_blocks = 10` 用于减少浅层 BTC 重组影响，并不保证所有节点同时完成同一高度的索引。

本次设计针对两个独立问题（第一项已完成本地实现）：

1. BH/indexer 的 `consensus_ready` 要求追平当前目标。BH 正在处理 H+1 时，即使 H 的
   状态完整，带 context 的历史共识查询也被阻断，CLI 同样把 Mining 显示为 WAITING。
2. validator 缺少区块引用的 BTC 状态时，Go 将 RPC 错误上传至普通验证失败路径。
   gossip fetcher 可能断开诚实 peer；downloader 可能包装成 invalid-chain 错误，导入路径
   可能记录 BAD BLOCK。尚未形成完整的“暂不可验证 → 等待 → 自动重试”闭环。

目标是将同步进度、指定高度的可验证性、真实状态故障分别处理。可验证的历史状态不因
最新同步目标前进而失效；数据不足不等于区块无效。

## 已确定的边界

- 默认 miner 使用 indexer 最新完整提交且可验证的状态，不按自身 pass 是否有效向更早高度搜索。
- 查询必须绑定指定高度及对应的 snapshot/system-state/registry identity，禁止失败后回退 current head。
- 父子 BTC anchor 不得回退；同高度必须保持精确 identity，复用次数不得超限。
  超限时本地 builder 即拒绝组块，不等其他节点拒绝。
- 复用上限仅约束同一 anchor 连续被多少个 USDB block 引用，不证明与真实 BTC tip 的距离。
- rollback/reorg recovery、未完成或失败的块发布、状态缺失或 mismatch 仍须拒绝相关共识查询。
- 深重组现有持久化 halt 机制继续有效；本计划不实现 orphan archive、自动 USDB rewind
  或向更早高度自动搜索安全状态。

## 第一阶段：指定已提交高度可验证

范围：indexer RPC、节点工具的矿工资格检查，以及对应测试与契约说明。

- 保留 `get_readiness.consensus_ready` 的全局追平语义，继续供启动编排和同步展示使用。
- 指定高度的共识查询独立检查安全条件。允许本地和 BH 的正常 `CatchingUp`；不再要求
  当前进度等于目标进度。历史覆盖有其他缺口时，仍须验证请求高度自身的完整锚点。
- 请求高度高于已提交高度返回 `HEIGHT_NOT_SYNCED`；目标历史缺失、裁剪、identity mismatch
  保留原来的结构化错误，不做高度替换。
- 保留 `BlockProcessingPending` 等提交保护。这一阶段不承诺在跨存储提交窗口中零等待，
  更不通过删除 pending 保护来读取 writer 的未提交状态。未来若放宽，需要完整的一致性读证据。
- 经济查询在派生后再次检查安全状态和完整 historical identity，拒绝跨恢复窗口的结果。
- CLI 使用 indexer 的已提交高度及精确 context 查询结果判断候选资格，不再要求 BH/indexer
  全局追平；深重组保护、Bitcoin 启动检查、epoch 和 candidate identity 校验保持有效。
- 不改铭文 JSON、能量公式、anchor policy、registry、genesis 或数据库格式，不要求重建。

验收：

- [x] BH 正常落后目标、indexer 正常落后 BH 时，已提交高度的 state/profile/candidate/energy 可查询。
- [x] 全局 readiness 仍报告追赶；矿工资格检查不因此直接拒绝。
- [x] 未来高度、目标锚点缺失、历史裁剪、错误 identity 仍被拒绝。
- [x] 未提交/失败块、shutdown、上游未知/回滚、持久化 reorg recovery 仍阻断。
- [x] 查询开始后出现恢复或提交保护时，完成阶段的复核拒绝发布结果。

本地验证记录：

- `tests/indexer_committed_height_queries.rs`：使用临时 SQLite/能量库、真实 RPC 方法和启用的
  readiness gate，覆盖正常追块、精确历史身份、未提交 writer、失败发布和恢复屏障。
  completion 复核用例在初始 state ref 与复核之间注入屏障，不依赖线程时序。
- `tests/test_node_mining.py`：正常追块时 preflight/enable/observe 仍可完成；未知 blocker、
  缺失 readiness 字段和候选返回后的提交屏障仍阻断授权，失败不修改配置或重建容器。
- indexer 完整测试：408 通过，12 项既有用例保持忽略；包含多区间协议、重组及进程退出恢复矩阵。
- mining 系列 77 项、节点工具 127 项、usdb-util 89 项通过；workspace check/Clippy、格式与
  release fragment 校验通过。上述均为本地测试，尚未执行本批 CI、真实节点升级或独立多节点验收。

## 第二阶段：Go 延迟验证与重试

新增明确的“暂时无法验证”分类，覆盖 header、gossip、downloader、区块执行/导入路径。
本地缺数据或可恢复的 RPC 不可用不得直接处罚 peer、标记 bad block 或视为确定无效链。
先完成可用的结构校验，再以有界队列或可重新获取的任务等待依赖；状态推进后重新完整验证。

必须包含去重、总量/字节/peer 限额、超远高度约束、重试退避、停机取消和父子依赖顺序。
数据永久裁剪、配置不兼容和深重组不能被简单归入无限重试。真实无效区块仍拒绝。
不能复用 timestamp future-block 错误来掩盖不同的依赖和资源边界。

## 第三阶段：独立多节点延迟矩阵

至少使用独立的 indexer/BH 状态，覆盖 gossip 先到、父块先同步、连续子块、RPC 中断、
BH/indexer 分别落后、恢复后无人工 reconnect/restart 自动导入，以及真正无效块的拒绝。
记录等待时长、peer 断连、重试次数、分叉/孤块和恢复高度；不以共用一个上游的测试替代。

## 第四阶段：可选矿工发布延迟 gap

gap 只是一项本地出块策略，不是 validator 的新共识条件。典型情况下优先选择最新完整
高度 L 的 L-gap；所有 profile、能量、难度、奖励和 activation 必须按所选历史 context 查询。

- 验证者只验证 payload 指定高度，不比较自己的 gap 或当前 BTC tip。
- 不得低于父 anchor；必要时复用父 anchor，identity 必须相同且不超龄。
- 达到复用上限且本地已有更高完整状态时，建议允许让步于本地 gap，推进 anchor。
- 必须处理起始高度、历史保留范围、规则激活边界及 remint 生效延迟。
- gap 只能降低等待概率，不能替代第二阶段；默认 0/1/2 根据第三阶段数据决定。

## 参考

- [UIP-0006 历史经济视图](../UIP/UIP-0006-usdb-economic-state-view.md)
- [UIP-0007 anchor 与验证](../UIP/UIP-0007-usdb-consensus-profile-selector.md)
- [readiness 契约](usdb-indexer-readiness-design.md)
- [RPC 契约](usdb-indexer-rpc-v1.md)

现有 `run_usdb_profile_e2e.sh` outage 测试显式 reconnect 和重启 mining，不能作为
第二、三阶段无干预恢复的验收证据。第一阶段通过也不代表上述 P2P 缺口已经关闭。
