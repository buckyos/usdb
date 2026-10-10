# Miner BTC anchor gap 方案

状态：默认 gap=2 的本地策略方向已确认，项目组讨论见 [USDB #52](https://github.com/buckyos/usdb/issues/52)。
尚未实现；本文不改变当前出块策略或网络共识参数。

前置工作已完成本地验收：指定已提交高度查询、Go 延迟验证与自动重试，以及三节点独立
BH/indexer、多矿工恢复矩阵。测试实现见 Go `4c1c18817`，验收记录见 USDB `0bdbc84`。
这些测试证明等待与恢复路径可运行，尚未证明 gap=2 在公网的性能收益。

## 1. 建议与边界

增加可配置的本地矿工选高策略，默认 `gap=2`。矿工可以显式覆盖为其他合法非负值，
包括设为 `0` 恢复最新高度策略，但常规运行不推荐调整。gap 不强绑定网络参数；
其单位是 **BTC block**，不是 USDB block，也不是固定时间。

- BH 的 `stable_lag_blocks=10` 保持不变；gap 在 indexer 最新完整提交高度上计算，不再从
  Bitcoin tip 重复减去 10。正常追平时，所选高度约为本机 BTC tip 减 12；索引落后时会更旧。
- gap 是优先目标，不是必须满足的距离。父 anchor、复用上限、索引起点可以使实际 gap 小于 2。
- validator 仍只按区块携带的 selector 和父块校验，不检查生产者的 gap，不要求自己也满足 gap。
- 不新增 header 字段、不变更铭文 schema、公式、registry、genesis、chain/network ID，
  不要求重索引、网络重置或设置共识激活高度。已有合规 validator 可验证这种历史 selector。
- 保留延迟验证与自动重试。gap 不能替代它们，也不是 BTC finality 或全网同步程度的证明。

预期收益是：为其他节点同步同一 BTC 状态留出余量，减少正常跨节点延迟导致的暂缓验块。
这有望改善传播和出块利用率，不改变共识安全规则，也不保证提高吞吐或消除竞争分叉。

## 2. 选高规则

每次组块固定父块及以下输入：

| 输入 | 含义 |
| --- | --- |
| `L` | indexer 成功返回的最新完整、可查询的已提交高度；不能用 BH 目标或 Bitcoin tip 代替 |
| `G` | 本地配置 gap，建议默认 2 |
| `F` | 已核对的历史查询起点；当前实现对应 `genesis_block_height` / chain 的 `btcIndexOriginHeight` |
| `P, A` | 父 selector 的 BTC height 与 anchor age；首个 selector 没有父 anchor |
| `M` | **待挖 USDB 子块** activation checkpoint 指定的最大 anchor age |

在现有 anchor policy v1 下：

1. `L < F`，或者父 anchor 存在且 `P > L`：等待可验证状态，不向更早高度寻找替代。
2. 先选 `T = max(F, saturating_sub(L, G))`，防止起点附近减法下溢。
3. 有父 anchor 时，选 `H = max(T, P)`；没有父 anchor 时，选 `H = T`。
4. 若 `H == P` 且复用会使 `A+1 > M`（包括计数溢出）：
   - `L > P` 时选 `H = P+1`，让步于 gap，推进到最邻近的新 BTC 高度；
   - `L == P` 时等待新状态，不能继续复用，也不能构造超限块。
5. 精确查询 `H` 的完整历史身份与 candidate；仍调用现有
   `ExpectedBTCAnchorAgeBlocks` 验证完整 transition，不能只凭选高算术放行。
   同高必须保持父块的 `snapshot_id/system_state_id`；前进时 age 为 0。

第 4 步的 `P+1` 只在 `L>P` 时计算，且不超过 `L`。这一步只解决 age 上限，
不是根据 pass 是否有利逐个探测高度。未来出现新 anchor policy 时，必须显式分派，
不能直接套用 v1 的算术。`M=0` 或未知 policy 仍为配置/版本错误。

假定 `G=2`、`F<=98`：

| `L` | 父高度 `P` | 父 age | 选择结果 |
| --- | --- | --- | --- |
| 100 | 无或 97 | 未耗尽 | 98，正常 gap |
| 100 | 99 | `A<M` | 99，受父 anchor 限制，实际 gap=1 |
| 100 | 99 | `A=M` | 100，推进 anchor，实际 gap=0 |
| 100 | 100 | `A=M` | 等待新 BTC 状态 |
| 99 | 100 | 任意 | 等待本地补齐父 anchor，不能退到 97 |

`A=M-1` 时仍允许最后一次同高复用，子 age 恰为 `M`。首次引用 age=0，
所以不能把 `M` 误当作包含首次引用在内的总块数。

当前没有动态 pruning floor，不新增自动搜索“任意可查询高度”的逻辑。起点以上若历史
缺失、裁剪、身份不符或服务处于恢复屏障，保留明确错误和既有重试/运维分类，不能静默改用
current head。若未来支持裁剪，再明确 floor 的 RPC 契约及与验证历史保留责任的关系。

## 3. 历史状态必须贯穿组块

目前 Go `PayloadBuilder.BuildCurrentPayload` 用 `get_system_state_info` 的当前高度构造
context，再调用 `resolve_miner_candidate`。Rust 已有 `get_state_ref_at_height` 和带 context
的历史 candidate 查询；不需要为 gap 新造一套 indexer 状态或能量计算。

建议流程：

1. 读取当前已提交 state，取得 `L`，保留服务 scope、registry 兼容性及安全检查。
2. 按第 2 节仅由高度、父块和配置确定 `H`。
3. 通过 `get_state_ref_at_height(H, context)` 取得该高度的精确历史身份。
   context 绑定当前 USDB checkpoint 允许的 BTC registry；如果复用父高度，还必须绑定父
   snapshot/system ID。检查响应的各高度和身份一致，不能把 `L` 的 ID 搬到 `H`。
4. 使用这个身份调用 `resolve_miner_candidate(usdb_main, H, context)`；校验返回的
   external state、selection rule、pass 和收益地址，构造唯一 selector。
5. 所有 BTC 侧 pass 状态、能量、协作贡献、aggregate 都来自 `H`；USDB 难度、奖励等
   chain policy 仍按待挖 **USDB block number** 激活，再应用到该 historical profile。

BTC activation 按 `H` 解析，不能直接使用 `L` 的 active version set；USDB chain activation
不能因 gap 延后。registry 历史一致性检查仍须保留，不能利用旧高度绕过未知版本或冲突。
两次 RPC 间正常前进不要求 `L` 始终不变；identity mismatch、恢复屏障等仍使本次组块失败，
由已有机制重新读取状态并重试，不能拼接不同时刻的身份和 profile。

所选 `H` 没有本矿工的有效 candidate 时，等待后续状态或显式调整本地配置；不因收益、
能量、旧证仍有效等原因向前后搜索其它高度。同一个 `H` 内仍沿用当前确定的 candidate 排序。
gap=0 也必须遵守父 anchor 与安全检查。

## 4. 产品与网络层注意事项

### 时效性

余额变化、首次 mint、remint、继承和协作变更，对采用正常 gap=2 策略的出块者通常需要
再等待 indexer 提交两个 BTC 高度才进入所选 profile。包括失效变化：在新状态已出现、
旧历史高度仍被合法引用的窗口内，旧 pass 仍可能用于出块。这是已有历史 selector 语义，
不能表述成“最新失效立即禁止所有旧高度出块”。

gap 不丢弃这两个区间的能量记录，也不改能量公式；但更晚使用新的能量、协作和资产状态
可能改变期间的挖矿结果或奖励，不能承诺收益完全不变。不能将 10→12 的相对变化当作
“没有时效成本”，也不能把两个 BTC 高度承诺为固定等待分钟数。

若紧急升级要求某类存量 pass 立即停止后续权益，应另行明确跨 BTC/USDB activation 的
共识规则；不能依赖建议 gap 或最新状态的 UI 判断来达成全网撤销。

### 不同 gap 共存

0、1、2 等配置可以共存，不因 gap 不同拒绝 peer 或区块。但只要一个合法生产者先把规范链
anchor 推到更高位置，后继矿工就不能退回 `L-2`；实际余量可能暂时归零。
默认值一致有利于效果稳定，不能要求 validator 通过本地 tip 强制维持余量。

当其他节点的索引差距超过 2，或某节点 RPC 故障时，仍需暂缓验证并自动恢复。这个策略
不承诺所有矿工同时选中同一高度，也不解决故意从历史高度缓慢推进的问题；已有 age 上限
只限制精确 anchor 的连续复用次数。

### 恢复与工作刷新

- 深重组 halt、失败发布、数据库恢复等保护不因历史 gap 放宽；不自动寻找“安全旧高度”。
- anchor 不倒退约束比较的是同一分支的父子块。USDB 正常换到另一条合法分支时，应基于
  新父块重算，不能拿被丢弃分支的最大 anchor 当作全局不可回退水位。
- 当前 `HasSystemStateChanged` 比较 current ID 与上次成功组块 ID。加入 gap 后必须分开
  保存“成功组块时观察的最新 head”和“实际选择的历史 state”，否则 current 永远不同于
  selected，可能每轮轮询都取消重建工作。
- 失败不得提前确认刷新基线；parent 改变、age 到达边界、配置变化和新 head 到来都应重算。
  只看到新 head 但还没成功完成历史查询时，不能吞掉后续重试。

## 5. 配置、展示与升级

建议实现名：geth `--miner.usdb-anchor-gap`，`miner.USDBConfig.AnchorGap`；node-kit
`USDB_MINER_ANCHOR_GAP`。均为拟定名称，当前尚不可执行。Go 默认配置与 node-kit 默认值
一致为 2，显式 0 必须保留；不要用“值为零就取默认”的写法。
首版只提供静态配置，不增加在线自适应调参；拒绝负数、非整数和超出高度类型范围的值。

设置放在本机运行配置，不放进共识 `network.json`、activation registry 或数据兼容性身份。
升级只需更新相应软件与运行配置并重启受影响服务，不隔离旧库、不重置矿工资格授权。
节点工具须能区分配置值、容器实际参数和未知状态，避免升级后以为 gap=2 而仍运行旧参数。

状态至少区分：

- 最新完整高度 `L`、当前工作选择的 `H`、configured/actual gap。
- 选高原因：正常 gap、历史起点、父 anchor、age 上限导致推进。
- 所选 pass、anchor age/max age；等待时指出目标高度、可用高度和具体失败原因。

目前 `usdb_mining.py` 按最新 readiness context 查询 candidate。工具接入时须区分
“最新状态资格”和“实际所选高度资格”，否则首次 mint 会显示可挖但 builder 等待，或
remint 后 UI 显示新证而当前工作仍使用旧证。实际工作以 Go builder 的结构化诊断为准；
诊断不存在时显示未知，不复制一套会漂移的选高算法。优先扩展现有本机 mining 诊断路径，
无需新增持久化监控服务；矿工授权、reorg guard 及 SourceDAO 前提继续独立保留。

## 6. 实施顺序与验收

### A. Go 选高与完整组块

在 `internal/usdb/client.go` 接入现有历史 state-ref RPC，拆出纯选高函数，修改
`builder.go` 和 worker 的刷新记录；沿用 anchor transition 与 profile resolver。
配置接入 `miner.USDBConfig`、CLI/TOML 默认及显式零值解析。无需修改 validator 接受规则。

测试覆盖：0/1/2、起点、无父 selector、父高度更高、本地尚未追到父高度、age 的 M-1/M
边界、counter/height 溢出、checkpoint 更换上限、同高度身份变化与未知 policy。
完整 builder 测试必须使用 `L` 与 `H` 不同的 profile/ID/版本，证明奖励与难度使用所选
历史数据；补 candidate 不存在不搜索其它高度、查询期间恢复、失败重试及刷新不空转。
activation 前后、USDB 分支切换、历史 registry revision 兼容与冲突都要覆盖。

### B. node-kit 配置和运维语义

接入 compose/启动参数、配置查看与运行态核验、升级保留显式 gap、矿工诊断与 status 展示。
补上首次 mint/remint 的等待提示；确认普通 CatchingUp 不被再次变成 mining 全局阻断。
验证无需重新授权或数据重建，且 gap=0 与新默认 2 均能明确落到实际 geth 参数。

UIP-0007 的 `Miner Payload Generation` 当前仍写 current state 和配置 pass_id；实施时
同步为“本地策略选择可验证历史 state，按 usdb_main 原子选择 candidate”，区分默认生成
策略与 validator 的规范约束。配合更新 RPC 使用说明和 handbook 即可；若不改区块有效性
规则，无需为此单独新增共识 UIP 编号。

### C. 独立多节点实测

扩展已有 3 节点/2 矿工矩阵，继续使用独立 Core/BH/indexer 和真实查询：

| 场景 | 必须观察的结果 |
| --- | --- |
| 相同网络推进、gap=0 与 gap=2 对照 | 控制验证者落后 1/2 个高度，前者产生可归因的状态等待，后者的指定高度可立即查询 |
| gap=2，验证者落后 3 个高度 | 仍进入等待；上游恢复后无人工重连或 chain 重启自动导入 |
| 多矿工混合 0/1/2 | 高 anchor 先入链后其余矿工跟随，绝不回退或因 gap 不同拒绝块 |
| age 耗尽但 L>P | 自动让步 gap、推进 H；没有新高度时停止组块，恢复后自动继续 |
| 首次 mint/remint/继承/协作/失效边界 | 按所选 H 切换 pass 与经济视图，UI 不把最新资格冒充实际工作状态 |
| BTC 与 USDB activation 边界 | 历史 BTC 规则和子块 chain policy 分别正确，不混用公式、registry 或身份 |
| BH/indexer 故障、恢复、竞争分支 | 仍遵守已有安全屏障，恢复后规范历史、两名矿工收益与状态一致 |

对照分别从隔离的同等初始条件启动，并显式记录实际 anchor；不能让 gap=0 先推进了父
anchor，再把被迫跟随的 gap=2 样本当作正常 gap 对照。记录状态等待次数/时长、导入时间、
实际 gap、竞争分支和恢复证据，不将真实网络噪声或 fakepow 算力差异归因于 gap。

Fast 放纯策略与历史组块回归；Nightly 放有界延迟对照及 age 让步；Weekly 执行混合 gap、
多轮故障与经济状态边界。已有 fakepow 矩阵用于功能和恢复验收，默认 2 的公网效果仍需
发布后观察，指标不达预期时可显式配置 0，不需要重置网络。

## 参考

- [总体计划与前置验收](committed-state-mining-and-validation-plan.md)
- [UIP-0006 历史经济视图](../UIP/UIP-0006-usdb-economic-state-view.md)
- [UIP-0007 selector 与 anchor](../UIP/UIP-0007-usdb-consensus-profile-selector.md)
- [UIP-0008 激活矩阵](../UIP/UIP-0008-protocol-versioning-and-activation-matrix.md)
- [indexer RPC 契约](usdb-indexer-rpc-v1.md)
