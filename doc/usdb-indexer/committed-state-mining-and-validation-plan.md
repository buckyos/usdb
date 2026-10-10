# 已提交 BTC 状态的出块与延迟验证改进

状态：第一阶段及正常区块执行期间的已提交前缀查询已提交（`f760421`、`981be5a`）。
第二阶段 Go 延迟验证与重试已提交（Go `40c3200c5`，文档 `7cc13b8`）。
第三阶段独立多节点矩阵已实现并通过本地单轮、连续三轮验收，待评审；可选矿工 gap 尚未实现。

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
- 第一批保留 `BlockProcessingPending` 等提交保护，不读取 writer 的未提交状态。
  对正常执行期间的进一步放宽见下面的补充批次；失败或恢复仍阻断。
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

## 第一阶段补充：正常执行 H+1 时查询已提交 H

第一批的 `BlockProcessingPending` 覆盖整个单块处理流程，不能保证处理新块时仍能使用旧高度。
补充实现将正常向前索引与失败、回滚恢复分开处理：

1. RPC 和派生能量查询使用独立 SQLite 只读连接，不能读到 writer 的未提交 pass、owner、
   collab、余额快照或高度。索引执行仍使用原写连接，保留同块内读取前序事件的语义。
2. 正常块只向已提交高度之后追加能量记录；查询严格限制在指定高度，能量 finalized height
   必须覆盖 SQLite 已提交高度。能量先 finalize，SQLite 最后发布；不会提前使用 H+1。
3. 每个状态查询持有读取许可，直到派生与校验结束。正常向前索引可并行；破坏历史的 reorg
   和失败恢复必须等待现有查询释放许可，新查询快速返回 `SNAPSHOT_NOT_READY`。
   这样也避免跨分支的中间数据污染经济视图缓存。长查询可能延后恢复实际执行，恢复期间不接受新查询。
4. 正常执行标志以作用域守卫管理；返回错误、取消或 unwind 都清除。pending 高度必须高于
   已提交高度，且没有 publication recovery，才允许读取旧前缀。失败留下的 pending 不放行。
5. head identity 读取随每块事务提交的精确历史 anchor，不等待旧的 batch adopted anchor 更新。
   `get_readiness.committed_query_ready` 表示当前已提交 head 是否可查询，仍须做请求高度和 identity 校验。
   全局 `consensus_ready` 和 `BlockProcessingPending` 保持同步/处理状态语义。
6. 挖矿工具只有在服务端明确返回 `committed_query_ready=true` 时才允许 pending 状态，不能仅根据
   blocker 名称推断。旧服务不提供此字段时仍保留 pending 阻断行为。

边界：不承诺每次 RPC 零重试。SQLite 已发布但运行态 pending 尚未清除、head 正好变化、
失败或恢复等窗口仍可能拒绝；不需要网络重置、重索引或更改能量/铭文规则。

验收使用 `tests/indexer_concurrent_committed_queries.rs` 的真实区块执行流程，以通道暂停在
8 个 publication failpoint；比较暂停前后的完整 state、pass、energy、profile、candidate、
collab、aggregate 结果，覆盖跨块发布、失败后的阻断/重试、重组等待查询结束和新查询快速拒绝。
这些暂停点仅编译进测试，不是运行时注入开关。

补充批次本地验证：indexer 411 项通过、12 项既有用例保持忽略；mining 系列 78 项、
节点工具 127 项、监控投影 16 项通过。新增 projection 测试确认该 readiness 字段保留
服务端布尔值，缺失或类型不合法时保持未知，不从全局同步状态推断。
workspace check/Clippy、格式、usdb-util 健康检查及 release fragment 校验通过；
尚未执行本批 CI、真实节点升级和独立多节点延迟验收。

## 第二阶段：Go 延迟验证与重试

新增明确的“暂时无法验证”分类，覆盖 header、gossip、downloader、区块执行/导入路径。
本地缺数据或可恢复的 RPC 不可用不得直接处罚 peer、标记 bad block 或视为确定无效链。
先完成可用的结构校验，再以有界队列或可重新获取的任务等待依赖；状态推进后重新完整验证。

必须包含去重、总量/字节/peer 限额、超远高度约束、重试退避、停机取消和父子依赖顺序。
数据永久裁剪、配置不兼容和深重组不能被简单归入无限重试。真实无效区块仍拒绝。
不能复用 timestamp future-block 错误来掩盖不同的依赖和资源边界。

### 第二阶段实现

- Go 使用独立的 external-state 错误分类，保留原始 RPC 错误及请求 pass/BTC 高度。
  `HEIGHT_NOT_SYNCED`、`SNAPSHOT_NOT_READY` 和已识别的临时网络故障进入等待；
  identity mismatch、无效 pass、难度、状态根错误仍是验证失败。
- 历史裁剪、不支持的查询版本、配置/凭据错误、不可解析的响应等本地永久故障不处罚 peer，
  不进入区块重试队列。同步调度明确暂停，修复依赖后需重启 chain 服务。
  不自动清库、重建或回退 BTC anchor。
- `FinalizeWithError` 是可选的 engine 接口，Ethash 和 Beacon 包装层均保留错误；
  StateProcessor 在失败时返回原始原因并丢弃工作状态，避免 RPC 失败被奖励缺失产生的
  state-root mismatch 掩盖。`InsertChain` 不持有链锁等待，由广播/同步调用方重试。
- gas 等可独立完成的 header 校验先于 profile RPC。每次重试重新进入正常 header、
  profile、奖励及状态根验证，不能凭先前成功的 RPC 结果跳过最终导入校验。
- fetcher 的等待与普通待导入区块共用去重和配额：全局 256 块、编码体积 128 MiB，
  每 peer 64 块/32 MiB，最多同时运行 4 个导入任务；保留 head 前 7/后 32 块的距离约束。
  初始 1 秒退避，依次 2、4、8 秒并封顶；每秒调度一次，实际启动时间还受正在执行的查询影响。
  队列任务从首次入队起最多保留 30 分钟，重复广播不延长寿命。父块未导入时保留后继顺序，
  等待过期后释放配额，可由后续同步或新广播重新获取。
- downloader 复用现有有界结果缓存，仅持有当前批次进行重试；每批最多等待 2 分钟，
  退避同样封顶 8 秒。超时返回临时错误，同步协调器至少等待 10 秒再发起新会话，
  peer 保持连接；已经提交的前缀保留。light/snap header 和 full block 导入都接入同一策略。
- 停机取消等待计时器，fetcher 的完成通知不会阻塞已经退出的事件循环。
  已经进入的 RPC 按其既有 query timeout 结束，不承诺立即中断底层请求。
- 输出首次等待、恢复、到期和永久阻断日志，包含阶段/高度/hash、尝试次数、耗时或原始原因。
  `SNAPSHOT_NOT_READY` 的服务端含义较宽，因此仍保留有界等待及现有深重组 halt 机制；
  不能将反复出现的此错误解释为“必然会自动恢复”。

本批不更改协议、genesis、registry 或数据库格式。已更新 Go CI 的 USDB 依赖到 `981be5a`，
但本地提交/验证不代表已推送、通过远端 CI 或完成真实节点升级。

### 第二阶段本地验收范围

- 真实临时 chain DB + 模拟 RPC：header 查询失败、header 通过后奖励查询失败，
  Ethash/Beacon 两条路径均不写坏块、不提交失败奖励，依赖恢复后原区块可重新导入。
  可直接证明无效的 gas header 在 RPC 前被拒绝，真正的奖励状态根不匹配仍被拒绝。
- fetcher：乱序父子、重复广播、header/执行阶段临时失败、恢复后连续导入、
  恢复后发现真实无效、light header 重试、配额/距离/寿命/并发上限及退出通知。
- downloader：full/light/snap 实际同步路径、部分前缀已提交后重试、取消/退出/等待超时，
  本地永久故障与真实无效链的 peer 处理差异。
- Fast CI 的必跑清单记录广播/同步关键用例，防止筛选条件遗漏；独立 BH/indexer 的
  网络级时延与故障恢复仍属于第三阶段，不以这些模拟 RPC 测试替代。


本地验证记录：Go 1.18.5 下完整 fetcher/downloader 测试通过；相关 USDB 共识、
导入、RPC 和 miner 回归通过；新增 core/fetcher/downloader/sync 调度用例通过 race 检查。
Fast CI JSON 报告确认 14 个关键同步用例实际运行并通过；覆盖校验/依赖锁工具 15 项测试通过。
Go 1.26 兼容工具链下新增用例和 geth 编译检查通过；release fragment、依赖锁、shell 语法、
格式及既有 Fast CI 涉及包的 vet 检查通过。额外扫描 `consensus` 根包仍报告
`merger.go` 两处既有 unreachable code，本批没有修改该文件或放宽现有 CI 检查。
未运行远端 CI、发布或真实节点升级。

## 第三阶段：独立多节点延迟矩阵

至少使用独立的 indexer/BH 状态，覆盖 gossip 先到、父块先同步、连续子块、RPC 中断、
BH/indexer 分别落后、恢复后无人工 reconnect/restart 自动导入，以及真正无效块的拒绝。
记录等待时长、peer 断连、重试次数、分叉/孤块和恢复高度；不以共用一个上游的测试替代。

### 第三阶段实现

Go 仓库新增 `tests/multi_miner_acceptance.py`，通过现有独立上游 runner 的
`MATRIX_SCENARIO=multi-miner` 启动。每个节点从独立空目录同步 Core/BH/indexer；
两个矿工使用不同 BTC owner、真实 pass 和 USDB 收益地址，第三个节点作为晚加入的 validator。
透明代理只延迟真实 Core `getblock` 请求，不伪造高度、profile 或 readiness 响应。

- BH 落后两个块时，旧高度的完整历史 profile 必须与健康节点一致；矿工仍可使用它出块。
- indexer 单独落后时，BH 必须已追平，历史 RPC 保持可用；新高度的验证进入等待。
- gossip 连续块、晚加入节点的 downloader 分别观察真实失败及重试。
  只在初次连接时设置 peer，恢复阶段不重连、不重启 chain，也不手工启动 mining 推动同步。
- indexer 进程中断产生实际 RPC 连接故障；使用同一数据库恢复该服务后，chain 自动导入。
- 两个矿工同时出块；另外显式构造不同 BTC anchor、同一 USDB 高度 hash 不同的竞争分支，
  恢复后要求更高工作量分支获胜，记录旧分支被替换的块。
- 测试 genesis 缩小 anchor age 上限到 24，验证耗尽后停止组块，新 BTC 状态可用后自动继续。
- 正常依赖故障不得留下 BAD BLOCK；最后通过真实 `admin_importChain` 注入非法 gas header，
  要求拒绝并记录它，再验证后续合法块正常导入。

恢复门限为 90 秒；必须有相同 selector 先失败、后成功的 RPC 审计证据。
同时核对 PID、Linux process start time、启动次数、peer 断连记录及目标高度的规范 hash。
`miner_stop` 后可能仍有在途的已封块，因此允许 head 前进到目标的合法后继，不能仅凭高度
追平或不同时间采样的 head hash 判断恢复或分叉。
最终逐块核对区块 roots、历史系统存储、两名矿工的收益，以及独立上游的 pass/energy/history。

Nightly `multi-miner-delay` 执行一轮，Weekly `multi-miner-soak` 在同一批节点与数据库上连续
执行三轮。Fast CI 覆盖证据判定、透明代理和 CI 入口；队列资源界限、light/snap、取消及
非法 gossip 仍由第二阶段 Go 回归覆盖。BTC 深重组沿用既有独立上游故障矩阵。

封块使用 `fakepow` 加 1200ms 延迟，其余共识校验、P2P、执行和奖励路径均为真实实现。
多矿工验收发现现有 delayed fake sealer 同步阻塞任务循环，导致无法及时取消旧封块任务；
已将该测试模式改为异步延迟并响应取消，新增 Fast 必跑及 race 回归，普通 PoW 路径不变。
这不是实际算力或公网延迟基准，不据此决定生产 gap，也不代表远端 CI 或真实节点升级已通过。

2026-10-10 本地验收：单轮 14 项、同一批节点连续三轮 34 项全部通过，最终分别收敛到
USDB 高度 68、136；每个恢复场景约 1.3–20.1 秒，无 peer 断连、chain 重启或人工重连。
三轮竞争分支分别有 2 个旧块被规范链替换，最终历史执行、两名矿工收益和独立上游状态一致。
两个 runner 均正常退出并完成进程清理。Python 辅助回归 79 项、Fast 必跑共识 9 项、
miner 定向回归通过；fake sealing 在 Go 1.18.5、Go 1.26 及 race 模式下通过。
ShellCheck、workflow YAML、Go 格式/vet 和 release fragment 校验通过。
本批只完成本地验收与 CI 入口接入，未运行远端 CI、发布或升级真实节点。

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
第三阶段无干预恢复的验收证据。第二阶段的本地 Go 回归也不能替代独立多节点验收。
