# USDB BTC 深层重组恢复机制讨论备忘录

Document Type: Discussion Memo
Status: Discussion Draft, Non-Normative
Updated: 2026-07-29
Related UIPs: UIP-0006, UIP-0007, UIP-0008, UIP-0009, UIP-0011, UIP-0012
Discussion: https://github.com/buckyos/usdb/issues/32

## 1. 文档定位

本文讨论 BTC 重组越过 USDB 当前 stable frontier 后，已经引用受影响 BTC
状态的 USDB chain 应如何检测、停机、恢复和重新加入网络。

本文不是正式 UIP，不定义已激活共识规则，也不授权实现自动链回滚。当前代码、
配置和运维流程不得仅依据本文改变 public network 行为。

先使用讨论备忘录而不是立即分配 UIP 编号，原因是以下决策尚未冻结：

1. USDB 是否永久接受曾经 canonical、后来 orphan 的 BTC 状态。
2. 如果不接受，USDB chain 应回退到哪个区块。
3. 谁负责确认事件、发布恢复工件，以及节点如何验证该工件。
4. 恢复决定如何进入共识验证、历史重放和 fresh joiner 流程。
5. public network 可接受的最大恢复范围和停机目标。

上述决策一旦稳定，影响区块有效性和历史重放的部分必须提升为一个新的
Standards Track UIP，并通过 UIP-0008 的版本和激活机制生效。当前不预留具体 UIP
编号。

## 2. 与现有 reorg 文档的边界

已有 [BTC Reorg 风险现状与改造计划](btc-reorg风险现状与改造计划.md)
主要处理 BTC 侧服务内部的 canonical chain 对齐：

1. `balance-history` 检测 BTC reorg，回滚 UTXO、余额历史和 snapshot。
2. `usdb-indexer` 跟随上游 stable anchor，回滚 pass、energy 和派生状态。
3. 两个服务在 replacement branch 上重新索引。

本文处理的是更上一层的问题：

1. 某个 BTC snapshot 已经进入 USDB block 的 profile selector。
2. BTC 后续发生深层 reorg，该 snapshot 对应的 BTC block 变为 orphan。
3. BTC 侧服务正确回滚后，不再能够按 canonical history 重放旧 selector。
4. 已运行节点可能已经接受旧 USDB block，而 fresh joiner 会拒绝它。

因此，BTC 索引器完成 rollback 并不等于 USDB chain 已经完成恢复。

## 3. 当前协议与实现基线

### 3.1 已有保护

当前 draft 基线包含以下保护：

1. BTC activation registry 固定 `stable_lag_blocks = 5`。
2. `balance-history` 只向下游暴露 stable frontier 及更早的状态。
3. `snapshot_id` 承诺对应的 `stable_block_hash`。
4. UIP-0007 anchor policy 禁止子区块 BTC height 回退。
5. 同一 BTC height 被连续复用时，snapshot 和 system state identity 必须不变。
6. 同一 anchor 的复用次数受到 `btc_anchor_max_age_blocks` 限制。
7. profile、external state 或版本不可重放时，validator fail closed。

这些规则降低普通短 reorg 和长期陈旧 selector 重放的风险，但不提供 BTC
finality。

### 3.2 未解决的分叉风险

当 BTC reorg 越过 stable frontier 时，当前可能出现：

1. BTC 侧服务回滚到新 canonical branch，并删除或替换旧派生状态。
2. 已运行 USDB 节点仍保留此前已经导入的 USDB blocks。
3. 已运行节点不会自动重新验证全部历史 selector，也不会自动 rewind。
4. fresh joiner 在重放旧 USDB block 时无法取得原 selector，因而 fail closed。
5. 新旧节点可能对同一 USDB chain 是否有效产生不同结论。

这是 public network 必须解决的确定性问题。提高 stable lag 只能降低触发概率，
不能消除该状态。

### 3.3 当前参数的含义

`stable_lag_blocks = 5` 是当前开发阶段的风险缓冲参数，不是以下任一承诺：

1. BTC block 在 5 个确认后具有绝对 finality。
2. USDB 节点可以使用本机 BTC tip 判断一个历史 USDB block 是否有效。
3. 深度超过 5 的 reorg 可以由节点各自自动处理。
4. UIP-0007 bounded-reuse guard 可以替代 reorg recovery。

public testnet/mainnet 上线前仍需根据实际运行目标复核 stable lag。

## 4. 术语

- **stable frontier**：BTC 当前可见 tip 减去 activation registry 固定
  `stable_lag_blocks` 后，允许进入 USDB economic state 的最高 BTC 高度。
- **frontier-crossing BTC reorg**：某个已经进入 USDB selector 的 stable BTC block
  后续不再属于 BTC canonical chain。
- **orphan selector**：引用上述 orphan BTC state identity 的
  `ProfileSelectorPayload`。
- **affected USDB block**：直接携带 orphan selector 的 USDB block。
- **earliest affected block**：目标 USDB canonical chain 上第一个 affected USDB
  block。
- **safe USDB head**：earliest affected block 的 parent。若策略要求回退，它是候选
  rewind target。
- **recovery artifact**：针对一次确定事件，冻结网络、受影响分支、safe head 和恢复
  规则的规范化工件。
- **orphan evidence archive**：保留旧 BTC/USDB 分支数据用于审计和取证的只读档案。
  它不自动赋予旧分支继续参与共识的资格。
- **recovery epoch**：一次已冻结并应用到特定 USDB network 的恢复决定。

## 5. 必须保持的设计约束

无论最终选择哪种恢复策略，都应保持以下约束：

1. `VerifyHeader` 不得读取验证节点本机的实时 BTC tip 来判断新鲜度。
2. 不得让不同节点根据各自 bitcoind 观察结果自动选择不同的 USDB rewind target。
3. 检测到疑似 frontier-crossing reorg 后，应 fail closed 并停止组块，不能边调查边
   继续累积依赖状态。
4. 在 evidence、earliest affected block 和 safe head 尚未冻结前，不得执行不可逆的
   数据删除。
5. recovery 必须对 restart、重复执行和 fresh joiner 保持确定性。
6. recovery 后的 validator 必须拒绝重新导入已废弃的 USDB 分支，不能只执行一次本地
   `setHead`。
7. BTC 侧状态、USDB EVM state、system storage、issued supply 和索引服务必须恢复到
   同一个逻辑边界。
8. 旧分支数据应先归档再回滚，以支持审计、故障分析和争议处理。
9. 任一必需服务、工件或历史数据不可用时，节点应保持 halted，不得猜测恢复结果。

## 6. 两种基本策略

### 6.1 策略 A：archive and continue

该策略把 USDB 已接受的 BTC snapshot 视为 acceptance-time final：

1. BTC reorg 后仍永久保存 orphan snapshot 及其完整派生状态。
2. validator 可按旧 identity 重放 profile。
3. 已导入 USDB blocks 保持有效，不回退 USDB chain。

优点：

1. USDB chain 不因 BTC 深层 reorg 回退。
2. 已确认交易、奖励和应用状态不被撤销。
3. 节点恢复流程相对简单。

代价和风险：

1. USDB 会永久保留已经不属于 BTC canonical chain 的 pass、energy、余额和经济影响。
2. archive 必须足以让 fresh joiner 独立重放，不能只保留最终 JSON 响应。
3. archive 的完整性、可用性和信任来源会成为新的共识依赖。
4. BTC canonicality 与 USDB economic state 的权威关系会被重新定义。
5. 如果 archive 丢失，旧 USDB history 将无法验证。

选择该策略意味着正式协议必须明确：BTC 状态只在进入 USDB 时接受一次，后续 BTC
canonicality 变化不再使其失效。这不是单纯的存储优化，而是跨链 finality 语义。

### 6.2 策略 B：safe halt and deterministic rewind

该策略持续把 BTC canonical history 作为 USDB economic state 的权威来源：

1. 检测到 frontier-crossing reorg 后停止组块和继续导入。
2. 确定 earliest affected USDB block。
3. 将其 parent 冻结为 safe USDB head。
4. 发布并验证统一 recovery artifact。
5. 所有节点回退到 safe head。
6. BTC 侧服务在 replacement branch 上重放。
7. USDB 从 safe head 继续同步和出块。

优点：

1. pass、energy、difficulty、reward、K 和 emission 始终来自 BTC canonical history。
2. fresh joiner 与已运行节点可以恢复到同一结果。
3. 不需要把 orphan BTC 派生状态永久作为共识输入。

代价和风险：

1. safe head 之后的 USDB blocks、交易和奖励会被撤销。
2. 需要全网协调停机、发布、升级和恢复。
3. 恢复决定本身具有治理属性。
4. 需要防止旧客户端继续扩展或重新导入废弃分支。
5. 跨服务 rewind、restart 和 joiner 流程复杂，必须充分自动化。

### 6.3 对比

| 维度 | Archive and continue | Deterministic rewind |
| --- | --- | --- |
| BTC canonicality | 接受时有效即可 | 持续作为权威来源 |
| USDB 历史 | 不回退 | 回退到 safe head |
| Fresh joiner | 依赖完整 orphan archive | 依赖 recovery epoch |
| 存储成本 | 长期保存可重放 orphan state | 保存审计证据，不用于继续共识 |
| 运维成本 | archive 高可用和长期维护 | 事故时全网协调恢复 |
| 用户影响 | 保留 USDB 确认结果 | 撤销 affected range |
| 主要风险 | orphan BTC 状态永久影响经济 | 恢复工件或回滚边界不一致 |

## 7. 当前讨论建议

当前更建议采用以下组合，作为后续讨论和原型验证方向：

```text
stable lag
    + bounded selector reuse
    + frontier-crossing detection
    + automatic safe halt
    + network-wide deterministic rewind
    + orphan evidence archive
```

其中：

1. stable lag 负责降低普通短 reorg 进入 USDB 的概率。
2. bounded reuse 防止矿工无限复用同一陈旧 selector。
3. 自动化只负责检测和安全停机。
4. rewind target 必须来自全网一致的 recovery decision，不能由节点本地自动决定。
5. orphan archive 只用于证据和审计，不用于让 orphan economic state 继续生效。

主要理由是当前 USDB 的 pass、energy、difficulty、K、reward 和 emission 都把 BTC
canonical state 作为外部权威输入。archive-and-continue 会把该语义隐式改成
acceptance-time finality，影响范围远大于一个存储实现选择。

这只是工作建议，不是最终结论。正式 UIP review 仍应允许选择策略 A，或定义带有明确
边界的混合策略。

## 8. 建议的恢复状态机

```text
Normal
  -> Suspected
  -> Halted
  -> RecoveryPrepared
  -> Rewinding
  -> Replaying
  -> Resumed
```

任一步骤失败都进入 `Halted` 或 `RecoveryFailed`，不得跳过校验后继续运行。

### 8.1 Normal

1. BTC 侧按 stable lag 提供 canonical economic state。
2. miner 和 validator 正常解析 selector。
3. 后台监控持续检查已经承诺的 BTC anchor 是否仍可按 canonical history 重放。

### 8.2 Suspected

触发条件可以包括：

1. 已承诺 `stable_block_hash` 不再位于 BTC canonical chain。
2. 历史 selector 从可重放变为 canonical mismatch。
3. BTC 侧服务报告 stable frontier 发生跨越式替换。

该状态允许收集证据，但不应继续生成依赖新 BTC profile 的 USDB block。

### 8.3 Halted

1. 停止 USDB mining。
2. 停止导入会扩展受影响 head 的新区块。
3. 保留 RPC 只读诊断面。
4. 冻结 BTC 与 USDB 证据副本。

自动 halt 是保底措施，不等于自动决定哪条链有效。

### 8.4 RecoveryPrepared

1. 验证 replacement BTC branch。
2. 枚举 USDB canonical chain 上的 selector。
3. 找到 earliest affected block。
4. 计算并交叉验证 safe head。
5. 生成候选 recovery artifact。
6. 完成治理、发布和客户端接受流程。

### 8.5 Rewinding

1. 验证本地 USDB chain 与 artifact 中的 affected branch 一致。
2. 归档被移除的区块、交易、receipts 和相关服务数据。
3. 回退 geth canonical head 和 EVM state 到 safe head。
4. 标记废弃分支，使节点不能再次导入它。
5. 将 BTC 侧服务回滚并对齐到 replacement branch。

### 8.6 Replaying

1. 先恢复 `balance-history`。
2. 再恢复 `usdb-indexer`。
3. 验证 economic state view 可稳定重放。
4. 从 safe head 重新同步 USDB chain。
5. 交叉校验 system storage、issued supply、reward 和相关索引。

### 8.7 Resumed

只有在既有节点和 fresh joiner 得到相同 head/state root 后，才重新开放 mining。

## 9. Safe head 计算草案

工作算法如下：

1. 选择待恢复的旧 USDB canonical branch。
2. 从 BTC reorg 可能影响的最早高度开始，按 USDB block number 顺序重放 selector。
3. 对每个 selector 校验其 `snapshot_id` 承诺的 BTC block 是否仍属于 replacement
   canonical branch，并重算对应 economic profile。
4. 第一个不能按新 canonical BTC history 验证的 USDB block 是 earliest affected
   block。
5. safe head 是该 block 的 parent。
6. earliest affected block 及其所有 descendants 都属于 removed range，即使后续 block
   使用了重新变为可用的 selector。

最后一条非常重要。后续区块的 state root 已经继承 affected block 的 reward、fee、
system storage 和交易状态，不能只删除直接携带 orphan selector 的区块。

该算法仍需冻结以下细节：

1. selector historical lookup 的规范输入和错误分类。
2. activation boundary 上没有 selector 的区块如何处理。
3. 多次 BTC reorg 叠加时使用哪个 replacement branch。
4. safe head 过深或早于允许恢复窗口时是否停止自动化并进入特殊治理流程。

## 10. Recovery artifact 草案

recovery artifact 至少应承诺：

1. artifact schema version。
2. USDB network ID、chain ID 和 genesis hash。
3. recovery epoch / incident ID。
4. 适用的 client release 和 activation registry binding。
5. 旧 BTC branch 与 replacement BTC branch 的 fork evidence。
6. replacement BTC anchor 的 height、block hash 和累计工作量证据。
7. earliest affected USDB block number/hash。
8. safe USDB head number/hash/state root。
9. 被废弃 USDB branch 的首个 block hash。
10. BTC `balance-history` 和 `usdb-indexer` 恢复边界。
11. artifact canonical hash。
12. 发布授权或签名信息。

工件必须使用 canonical encoding，并拒绝：

1. duplicate JSON key。
2. 未知 network/genesis。
3. safe head hash 与本地 canonical chain 不匹配。
4. earliest affected block 不是 safe head 的直接子块。
5. artifact version 或 registry binding 不受支持。
6. 缺失的 branch rejection 信息。

当前不在本文冻结 signer 模型。第一阶段不要求持续运行 M-of-N checkpoint
signer/publisher 网络。事故恢复工件可以复用未来冻结的 release manifest 发布流程，但
其责任人、签名门限和紧急发布规则仍需单独 review。

## 11. 组件职责草案

### 11.1 balance-history

1. 检测并证明 local stable anchor 与 BTC canonical branch 的偏离。
2. 提供 fork point、old/new block hash 和 rollback status。
3. 回滚并重放 replacement branch。
4. 在恢复完成前不向下游宣称 ready。

### 11.2 usdb-indexer

1. 跟随 `balance-history` 回滚 pass、energy、snapshot 和 adopted anchor。
2. 暴露 selector replay/mismatch 的稳定错误分类。
3. 为 recovery planner 批量验证 historical selector。
4. 恢复后重算 profile、candidate 和 breakdown，并提供一致性摘要。

### 11.3 go-ethereum

1. 发现疑似 orphan selector 时停止 mining/import。
2. 验证 recovery artifact 的 network、chain 和 branch binding。
3. 确定性回退到 artifact 指定 safe head。
4. 持久化 recovery epoch。
5. 拒绝重新导入 artifact 标记的废弃分支。
6. restart 和 fresh joiner 按相同 recovery epochs 重放。

仅调用本地 `SetHead` 不足以完成协议恢复，因为节点之后仍可能按累计工作量重新导入旧
分支。branch rejection 必须成为升级客户端可验证的输入。

### 11.4 运维与发布流程

1. 收集至少两个独立 BTC 数据源和本地索引证据。
2. 冻结 incident 时间线和 affected range。
3. 生成候选 artifact 并在隔离环境重放。
4. 发布带校验和的客户端与 recovery artifact。
5. 协调节点 halt、升级、rewind 和 restart。
6. 验证既有节点与 fresh joiner。
7. 达到恢复验收条件后再开放 mining。

## 12. 状态与交易语义

采用 deterministic rewind 时：

1. removed blocks 的 coinbase reward、fee split 和 issued supply 通过 EVM state rewind
   一起撤销，不应编写额外反向转账修补。
2. UIP-0011 至 UIP-0014 system storage 应随 state root 回退。
3. removed transactions 可以重新进入 txpool，但只做 best-effort，不保证原顺序和再次
   成功。
4. replacement chain 必须重新执行全部交易和系统结算，不能复用旧 receipts。
5. explorer、console、indexer 和外部消费者必须能够识别 removed branch。
6. 对外 API 应暴露 recovery epoch、halt 状态和 canonical head，避免静默改写历史。

## 13. 崩溃一致性与幂等要求

恢复流程必须：

1. 每一步写入 durable phase marker。
2. restart 后从已确认阶段继续，而不是重新猜测。
3. 重复应用同一 artifact 得到相同结果。
4. 拒绝对同一 recovery epoch 使用不同 artifact。
5. 在任一组件失败时保持 mining disabled。
6. 在删除旧状态前完成可验证备份。
7. 支持 dry-run，仅计算 affected range 和 safe head。

## 14. 安全与治理考虑

### 14.1 禁止基于本地 tip 自动回滚

不同节点看到 BTC reorg 的时间、peer 集合和 RPC provider 可能不同。若每个节点自行
回滚，USDB block validity 会依赖本地时间和网络视图，必然产生分叉风险。

### 14.2 自动 halt 与治理决定分离

自动 halt 只减少继续扩展错误状态的损失。选择 replacement BTC branch、safe head 和
废弃 USDB branch 是全网一致性决定，必须有可审计工件。

### 14.3 不把价格 oracle 与 BTC checkpoint 信任混合

BTC canonical branch 恢复与 fixed/dynamic price 更新属于不同信任域。后续即使使用
签名工件，也必须进行 domain separation，不能让同一个未经约束的消息同时改变 BTC
anchor 和价格。

### 14.4 旧客户端风险

未识别 recovery epoch 的旧客户端可能继续跟随废弃分支。public recovery 流程必须说明：

1. 最低支持 client version。
2. 旧版本是否强制停止。
3. peer handshake 或 capability 是否需要暴露 recovery epoch。
4. 如何防止旧分支凭更高累计工作量重新成为本地 canonical chain。

### 14.5 Future SPV

BTC header chain / SPV proof 可以降低 canonical branch 判断对外部服务的信任，但不能
自动消除跨链 finality 取舍。即使 future policy 能在 USDB 内验证 BTC cumulative work，
仍需定义已经接受的 USDB history 遇到更重 BTC branch 时是否回退。

SPV 应使用新的 anchor policy version 独立激活，不应静默改变 v1 恢复语义。

## 15. 测试矩阵

正式实现至少覆盖：

1. reorg 完全位于 stable lag 窗口内，USDB 不受影响。
2. reorg 恰好越过 stable frontier 一个 BTC block。
3. 多 BTC block 深层 reorg。
4. 多个连续 USDB blocks 复用同一受影响 selector。
5. affected selector 分布在多个 BTC heights。
6. earliest affected block 与 safe head 精确计算。
7. safe head 之前的 state root、issued supply 和 system storage 不变。
8. removed range 的 reward、fee、transactions 和 receipts 被撤销。
9. restart 发生在 halt、artifact apply、rewind 和 replay 各阶段。
10. 同一 artifact 重复执行保持幂等。
11. 错误 network、genesis、safe head、registry binding 和签名被拒绝。
12. 同一 epoch 的冲突 artifact 被拒绝。
13. 节点不能重新导入已废弃 branch。
14. BTC 服务、usdb-indexer 和 geth 任一不可用时保持 halted。
15. 恢复后既有节点与 fresh joiner 得到相同 block hash/state root。
16. 从 genesis 使用全部 recovery epochs 重放得到相同结果。
17. world simulator 对 recovery 前后 profile/candidate/breakdown 交叉校验。
18. explorer、console 和 control-plane 正确标记 removed branch。
19. 大数据量下 dry-run、rewind 和 replay 的时间、内存、磁盘评估。
20. future SPV fake policy 激活不改变既有 v1 historical replay。

## 16. 待讨论问题

1. 最终采用 archive-and-continue、deterministic rewind，还是显式混合策略。
2. frontier-crossing 事件需要什么 BTC fork evidence 才能从 `Suspected` 进入
   `Halted/RecoveryPrepared`。
3. 自动 halt 是否需要多个独立 BTC provider 一致，还是单个可信本地 full node 即可。
4. recovery artifact 由谁批准、签名和发布。
5. safe head 最大允许回退多少 USDB blocks 或多长时间。
6. 超过最大恢复范围后，是长期 halt、人工 hard fork，还是切换 archive policy。
7. 被移除交易进入 txpool 的保留时间和去重规则。
8. recovery epoch 放入 chain config、release manifest、独立 registry，还是三者组合。
9. fresh joiner 如何取得所有历史 recovery artifacts，并验证其完整顺序。
10. old client 与 recovery-aware client 的网络隔离规则。
11. 何时允许重新开放 mining，以及需要多少独立节点完成 joiner 验证。
12. public testnet/mainnet 的 stable lag 最终参数和事故响应时限。

## 17. 升格为 UIP 的条件

满足以下条件后，应将本备忘录的规范部分整理为新的 Standards Track UIP：

1. 冻结 BTC orphan state 是否继续具有 USDB 共识效力。
2. 冻结 deterministic safe head 算法。
3. 冻结 detection evidence 和 halt 条件。
4. 冻结 recovery artifact schema、canonical encoding 和授权模型。
5. 冻结 client branch rejection 与 recovery epoch 规则。
6. 明确涉及的 version fields、activation matrix 和 backward compatibility。
7. 完成 geth、BTC services、restart 和 fresh joiner 原型。
8. 完成跨进程 deep-reorg recovery E2E。
9. 在 public testnet 至少演练一次完整停机、回退、重放和恢复出块。
10. 将最终决议同步回 UIP-0007、UIP-0008、UIP-0009 和相关经济 UIP。

在此之前，本备忘录只作为 review 和原型设计输入。
