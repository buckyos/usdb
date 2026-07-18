UIP: UIP-0002
Title: Miner Pass State Machine
Status: Draft
Type: Standards Track
Layer: BTC Application / Consensus Input
Created: 2026-04-25
Requires: UIP-0000, UIP-0001
Supersedes: doc/矿工证铭文协议.md state draft after activation
Activation: BTC network activation matrix

# 摘要

本文定义 USDB 矿工证的标准状态机。

本文覆盖：

- `Active / Dormant / Consumed / Burned / Invalid` 的状态语义。
- standard pass 与 collab pass 在状态机中的共同规则和差异。
- mint、invalid mint、transfer、same-owner transfer、burn、remint(prev) 的状态转换。
- 同一 BTC block 内多事件的 canonical ordering。
- 同一 BTC block 内 balance 变动与 pass 事件的 block-level settlement boundary。
- `prev` 严格校验与原子失败规则。

本文不定义 energy 具体公式、继承折损参数或 `effective_energy` 公式。

# 动机

早期实现已经具备可运行的 pass 状态记录和历史记录，但曾有几类规则尚未标准化：

- `prev` 引用不存在、owner 不一致、状态不符合要求时，早期实现会 `warn + skip`，然后继续 mint。
- burn 必须同时关闭 pass 状态和 energy 终态，避免 burn 后继续暴露可用能量。
- 同一 block 内 transfer 与 mint 的处理顺序必须成为协议规则，否则历史重放可能分叉。
- 同一 block 内 owner balance 变化与 pass 事件的相对顺序必须成为协议规则，否则 transfer/remint 可能绕过 UIP-0003 penalty。
- UIP-0001 引入 standard pass / collab pass 后，状态机必须明确两类 pass 是否共享 active owner 限制。

UIP-0002 的目标是先固定事件与状态语义，为 UIP-0003 energy 公式和 UIP-0004 collab/effective energy 提供稳定输入。

# 当前实现状态

参考实现已完成 UIP-0002 core 对齐：

- `prev` 先完整校验再原子提交；missing、owner mismatch、duplicate、非 Dormant、Consumed/Burned 引用均使新 mint 进入 `Invalid`，不会部分消费。
- valid remint 在同一 event height 将每张 prev 写为 `Consumed`、energy 写为 `Consumed / 0`，再创建新 Active pass。
- Active / Dormant burn 同步写入 `Burned / 0` energy 终态；Consumed burn 保持 `Consumed` 经济终态。
- pass event 使用 canonical block ordering，Active 离开前完成 block-level balance settlement，block-end 只结算最终仍 Active 的 pass。
- `leader_pass_id` mint-time 校验 Active standard Leader；`leader_btc_addr` 按当前 BTC network 接收并在目标高度动态解析。
- Consumed / Burned pass 后续 transfer 不再更新 owner/satpoint，明确作为非共识审计 tradeoff。

当前剩余工作是集中 live/regtest 状态矩阵复核，以及由 UIP-0008 固定公开网络 activation matrix；不需要兼容早期 warn/skip 行为或旧数据库。

# 非目标

本文不定义：

- energy 增长公式。
- `prev` 继承折损率和 rounding。
- collab energy 权重。
- level、difficulty、reward split。
- validator payload 的完整字段集合。
- 前端展示状态命名。
- BTC block 内 transaction-level 的余额结算顺序；本文以 BTC block 为最小经济结算粒度。

# 术语

| 术语 | 含义 |
| --- | --- |
| pass | 符合 UIP-0001 v1 schema 的矿工证铭文。 |
| standard pass | 包含 `usdb_main` 的 pass，可独立参与挖矿候选集合。 |
| collab pass | 包含 `leader_pass_id` 或 `leader_btc_addr` 的 pass，不可独立参与挖矿候选集合。 |
| owner | 当前持有 pass 所在 UTXO 的 BTC 地址语义，规范化后以 script hash 或等价确定性 ID 表达。 |
| state | pass 在指定 BTC 高度下的协议状态。 |
| active owner set | 在某一高度所有处于 `Active` 的 pass 按 owner 形成的集合。 |
| event height | 触发状态转换的 BTC block height。 |
| block-level balance snapshot | 某 owner 在一个 BTC block 完整执行后的聚合余额视图。 |
| balance settlement boundary | pass 状态事件消费 block-level balance snapshot 的确定性边界。 |

# 状态集合

矿工证状态必须是以下之一：

| 状态 | 含义 | 是否可增长 raw energy | 是否可作为 `prev` | 是否可独立挖矿 |
| --- | --- | --- | --- | --- |
| `Active` | 当前活跃 pass。 | 是 | 否，除非在同一次 valid mint 中被先虚拟转为 `Dormant`。 | 仅 standard pass 可以。 |
| `Dormant` | 已冻结 pass。 | 否 | 是。 | 否。 |
| `Consumed` | 已被一次 valid `prev` 继承消费。 | 否，energy 必须为 `0`。 | 否。 | 否。 |
| `Burned` | 已销毁或不可再参与经济行为。 | 否，energy 必须为 `0`。 | 否。 | 否。 |
| `Invalid` | mint 不满足协议。 | 否。 | 否。 | 否。 |

`Consumed`、`Burned`、`Invalid` 都是经济终态。它们不得重新变为 `Active` 或 `Dormant`。

# 全局不变量

## 单 owner 单 Active pass

任一 BTC owner 在同一历史高度最多只能拥有一张 `Active` pass。

该限制同时适用于 standard pass 和 collab pass。原因是 raw energy 与 owner 的 BTC 余额相关，如果同一 owner 能同时拥有多张 active pass，会导致余额能量被重复计入。

## Active collab pass 不可独立挖矿

collab pass 可以处于 `Active`，并可按 UIP-0003 继续累计 raw energy。

但 collab pass 在独立挖矿口径下的 `effective_energy` 必须视为 `0`。它只能通过 UIP-0004 定义的 Leader 解析与权重规则影响 Leader 的 `effective_energy`。

## 终态能量

`Consumed` 和 `Burned` 的 energy 必须为 `0`。

如果 pass 状态写入 `Consumed` 或 `Burned`，energy 状态机必须在同一 event height 写入等价终态记录。具体 energy 记录格式由 UIP-0003 定义。

# Canonical Event Ordering

同一 BTC block 内，索引器必须按确定性顺序处理 pass 相关事件。

事件排序规则：

1. 按 transaction 在 block 中的位置升序。
2. 同一 transaction 中，transfer/burn 事件先于 mint 事件。
3. 同一 transaction 的多个 transfer/burn 事件按 input index 升序。
4. 同一 transaction 的多个 mint 事件按 inscription index 升序。
5. 如果以上字段仍相同，按 inscription id 字符串升序。

历史查询在高度 `h` 查询某 pass 当前态时，必须观察高度 `h` 的全部 ordered events 执行完成后的最终状态。

实现可以保留同一高度的多条 history event，但必须保证 replay 顺序稳定。

UIP-0002 的公开协议查询粒度是 BTC block。history query 默认返回某高度完整 block 执行后的最终状态，不要求暴露 event index 或同高度中间态。实现可以为审计提供 event index，但该字段不得影响共识结果。

# Canonical Balance Settlement Boundary

UIP-0002 规定 BTC block 是 USDB pass 经济状态的最小结算粒度。协议不定义 BTC block 内 transaction-level 的余额结算顺序。

对任意 owner `O` 和 BTC block height `H`，必须先定义 block-level balance snapshot：

```text
balance_before(O, H) = balance_at(O, H - 1)
balance_after(O, H)  = balance_at(O, H)
delta(O, H)          = balance_after(O, H) - balance_before(O, H)
```

如果 `H = 0`，`balance_before(O, H)` 视为 `0`。

同一 BTC block 内，无论 owner 的多个 UTXO 在 transaction 级别如何流转，UIP-0003 的余额变化结算都只能消费上述聚合后的 `balance_before / balance_after / delta`。实现不得用不同的 transaction-level 余额顺序影响 raw energy、penalty、`prev` 继承或 state transition。

## Active Pass 离开 Active 前的结算

在 block `H` 内，如果某个 `Active` pass 会因 ordered pass event 转为 `Dormant` 或被 `prev` 继承消费，则必须先使用该 pass 当前 owner 在 `H` 的 block-level balance snapshot 完成 UIP-0003 settlement。

该 settlement 必须发生在以下状态效果之前：

- different-owner transfer 导致的 `Active -> Dormant`。
- same-owner mint 导致旧 active pass 被 supersede 为 `Dormant`。
- 当前 active pass 在同一 valid mint 中被作为 `prev` 消费。

如果该 owner 在 `H` 的 `delta(O, H) < 0`，则 UIP-0003 balance decrease penalty 必须先计入 old active pass 的 settled raw energy，然后该 pass 才能冻结或被继承。

同一 pass 在同一高度最多只能有一个 canonical balance settlement。若实现已经在高度 `H` 写入相同 `balance_after / delta` 的 settlement record，后续 ordered event 或 block-end settlement 必须把它视为幂等结果，不得重复扣 penalty 或重复增长。

## New Mint 的初始余额

在 block `H` 内创建的新 pass 使用 `balance_after(mint_owner, H)` 作为初始 `owner_balance_sats`，并设置：

```text
active_block_height = H
```

新 pass 不承担 block `H` 的聚合 owner balance decrease penalty；`H` 内发生的余额变化只通过 `balance_after(mint_owner, H)` 体现在初始余额中。它的初始 raw energy 只来自 valid `prev` 继承结果；后续 raw energy 增长从高度 `H` 之后开始。

## Block-End Active Balance Settlement

ordered pass events 全部执行完成后，索引器必须对 block `H` 结束时仍处于 `Active` 的 pass 执行 block-end active balance settlement。

该结算使用同一份 block-level balance snapshot：

```text
owner_balance_sats = balance_after(owner, H)
owner_delta        = delta(owner, H)
```

对于已经在本 block 内因离开 Active 前置结算而写入 exact-height settlement 的 pass，block-end settlement 必须幂等跳过或验证一致性。

# 状态转换表

| From | Event | To | 说明 |
| --- | --- | --- | --- |
| none | valid mint | `Active` | 新 pass 成功铸造。 |
| none | invalid mint | `Invalid` | mint schema 或状态前置条件失败。 |
| `Active` | valid mint by same owner | `Dormant` | 旧 active pass 被新 pass supersede。 |
| `Active` | transfer to same owner | `Active` | 仅更新 satpoint。 |
| `Active` | transfer to different owner | `Dormant` | 先完成 old owner 的 energy settlement，再冻结并更新 owner/satpoint。 |
| `Active` | burn | `Burned` | energy 同步归零。 |
| `Dormant` | transfer | `Dormant` | 更新 owner/satpoint，不恢复增长。 |
| `Dormant` | valid `prev` consumption | `Consumed` | energy 被新 pass 继承后归零。 |
| `Dormant` | burn | `Burned` | energy 同步归零。 |
| `Consumed` | burn | `Consumed` | 可追加非共识审计记录，但当前经济状态保持 `Consumed`。 |
| `Consumed` | transfer | `Consumed` | 不要求继续追踪 owner/satpoint，不产生经济效果。 |
| `Burned` | any | `Burned` | 终态。 |
| `Invalid` | any | `Invalid` | 非 pass 经济对象。 |

禁止的转换必须导致相关事件无经济效果；如果该事件是 mint 的前置条件失败，则新 mint 必须进入 `Invalid`。

# Valid Mint

valid mint 必须满足：

- inscription content 满足 UIP-0001 v1 schema。
- mint owner 可以从 reveal 结果确定。
- 如果 mint 为 collab pass，Leader 绑定字段满足本文的前置条件。
- `prev` 列表满足本文的严格校验。

valid mint 的提交必须是原子的：

1. 校验全部前置条件。
2. 如果同一 owner 当前已有 active pass，先按本文的 balance settlement boundary 在同一 event height 结算旧 active pass，再将旧 pass 虚拟视为 `Dormant`，用于后续 `prev` 校验。
3. 如果所有校验通过，才提交状态变更。
4. 若任一校验失败，新 mint 记录为 `Invalid`，不得改变旧 active pass，不得消费任何 `prev`。

提交顺序建议为：

1. 结算旧 active pass 的 energy，如果存在。
2. 将旧 active pass 写为 `Dormant`，如果存在。
3. 写入新 pass 为 `Active`。
4. 将所有被引用的 `prev` pass 写为 `Consumed`。
5. 写入对应 energy 状态记录。

同一 event height 下的最终状态以完整提交后的结果为准。

# Invalid Mint

如果 mint 不满足 UIP-0001 schema 或 UIP-0002 状态前置条件，索引器必须记录 `Invalid` mint。

invalid mint 必须满足：

- 不进入 active owner set。
- 不产生 raw energy。
- 不消费 `prev`。
- 不使同 owner 旧 active pass 进入 `Dormant`。
- 不影响 Leader 解析。

invalid mint 的 error code 应该稳定可检索。具体 error code 可以在实现文档或后续 parser UIP 中细化。

# `prev` 严格校验

`prev` 缺失等价于空数组。

如果 `prev` 非空，则必须先完整校验，再执行任何状态写入。

每个 `prev_i` 必须满足：

- 是合法 inscription id。
- 在当前 replay 上下文中存在。
- 是 valid pass，不是 `Invalid`。
- 在本次 mint 的虚拟前置状态中为 `Dormant`。
- 当前 owner 等于新 mint 的 mint owner。
- 未处于 `Consumed` 或 `Burned`。
- 未在同一个 `prev` 数组中重复出现。

如果任一 `prev_i` 不满足条件，本次 mint 必须进入 `Invalid`，且不得部分继承。

## 当前 active pass 作为 `prev`

同一 owner 可以在新 mint 的 `prev` 中引用自己当前的 active pass。

该场景按以下规则处理：

```text
old_active --virtual_dormant_at_h--> eligible_prev --consume_at_h--> Consumed
new_mint -----------------------------------------------> Active
```

如果同一 mint 中还引用了其他无效 `prev`，则整次 mint invalid，`old_active` 必须保持原状态，不得被部分 dormant 或 consumed。

## 所有权一致性

UIP-0002 采用 BTC owner 一致性，而不是 USDB/EVM 地址、Leader 地址或 lineage 一致性。

即：

```text
prev.owner_at_event_height == new_mint.mint_owner
```

如果旧 pass 先在同一 block 的更早 ordered event 中 transfer 给新 owner，则后续 mint 可以引用它作为 `prev`。

# Transfer

transfer 指 pass 所在 inscription UTXO 转移到新的 BTC owner。

## same-owner transfer

如果 transfer 后 owner 与 transfer 前 owner 相同：

- 必须更新 satpoint。
- 禁止改变 state。
- 禁止重置 energy 增长窗口。
- 禁止扣除或继承 energy。

## different-owner transfer

如果 transfer 后 owner 与 transfer 前 owner 不同：

- different-owner transfer 本身不产生 transfer-specific penalty。
- 若 pass 为 `Active`，必须先在 event height 按 UIP-0003 结算 old owner 的 raw energy。
- 若 old owner 在同一 BTC height 存在可计入余额减少，该余额减少引发的 UIP-0003 penalty 必须按本文的 balance settlement boundary 先进入 settlement 结果，然后才能冻结为 `Dormant`。
- 若 pass 为 `Active`，必须转为 `Dormant`。
- 必须更新 owner 与 satpoint。
- 若 pass 为 `Dormant`，保持 `Dormant` 并更新 owner/satpoint，不得扣除 energy 或恢复增长。
- 若 pass 为 `Consumed` 或 `Burned`，协议不要求继续追踪 owner/satpoint；当前 BTC indexer 实现不保留后续 physical transfer 审计记录，也不得恢复任何经济能力。
- 若 pass 为 `Invalid`，不得进入 active owner set。

different-owner transfer 后，冻结的 `raw_energy` 是该 pass 的可转让历史收益凭证。old owner 后续 BTC 余额变化不得影响该 `Dormant` pass；new owner 的 BTC 余额也不得让该 `Dormant` pass 继续增长。

new owner 若希望继续使用该 pass 的 raw energy，必须通过新 mint + `prev` 继承流程显式激活，并接受 UIP-0003 定义的通用继承折损。协议不得定义额外 transfer penalty 或绕过继承折损的直接激活路径。

# Burn

burn 指 pass inscription 被销毁或无法再定位到可用 owner。

burn 规则：

- `Active` pass burn 后必须转为 `Burned`。
- `Dormant` pass burn 后必须转为 `Burned`。
- `Consumed` pass burn 后当前经济状态保持 `Consumed`；实现可以追加非共识审计记录。
- `Invalid` mint burn 后仍视为非经济对象。
- 当 burn 导致 pass 经济状态转为 `Burned` 时，必须在 pass 状态和 energy 状态中同时写入终态。
- burn 后任意高度查询不得继续投影或返回 burn 前的可用 energy。

`Consumed` 之后的物理 satpoint 流转不属于 UIP-0002 的共识要求。随着时间增长，`Consumed` pass 数量会持续增加，如果强制继续追踪其 UTXO 流转，会显著扩大索引成本，且不会改变任何经济状态。

# Standard Pass 与 Collab Pass

standard pass 与 collab pass 共享同一状态集合和单 owner 单 active 限制。

差异如下：

| 项 | standard pass | collab pass |
| --- | --- | --- |
| `Active` raw energy | 可以增长 | 可以增长 |
| 独立 candidate set | 可以进入 | 禁止进入 |
| `effective_energy` | 基于自身和 collab 加成 | 独立口径为 `0` |
| Leader 解析 | 不适用 | 由 UIP-0001 字段和 UIP-0004 规则解析 |
| 转为另一类型 | 可通过新 mint + `prev` | 可通过新 mint + `prev` |

类型转换只通过新 mint + `prev` 完成。直接修改已有 pass 的类型是禁止的。

collab pass 转 standard pass 或 standard pass 转 collab pass 时，状态机只负责保证 `prev` 原子消费。能量继承统一使用 UIP-0003 的 `prev` 继承折损；UIP-0004 只定义转换后的 derived effective energy 影响，不定义额外 collab exit penalty。

# Collab Leader 前置条件

collab pass 的 Leader 绑定字段由 UIP-0001 定义。

## `leader_pass_id`

如果 collab mint 使用 `leader_pass_id`：

- 引用的 Leader pass 必须存在。
- Leader pass 必须是 standard pass。
- Leader pass 在 event height 的 ordered context 中必须为 `Active`。
- Leader pass 不得是本次 mint 创建的新 pass。

如果不满足上述条件，本次 collab mint 必须进入 `Invalid`。

## `leader_btc_addr`

如果 collab mint 使用 `leader_btc_addr`：

- `leader_btc_addr` 必须是当前 BTC network 上的合法地址。
- mint 时不强制要求该地址已经存在 active standard pass。
- 在任意历史高度，只有当该地址能解析到唯一 active standard pass 时，collab pass 才能向其贡献有效能量。

`leader_btc_addr` 在 UIP-0002 中不需要额外延迟一个 BTC block。解析口径是目标高度完整 block 执行后的 canonical pass snapshot。

ETHW 侧是否需要额外 finality lag、epoch 延迟或 validator payload 固定窗口，不属于 UIP-0002，应由 validator / effective energy 相关 UIP 定义。

# Activation Matrix

UIP-0002 影响 BTC 侧 pass 状态、`prev` 消费和历史 replay。ETHW 侧只消费索引结果。

| Chain | Network Type | Network ID | Activation Anchor | Activation Value | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| BTC | regtest | btc-regtest | btc_height | TBD | Planned | 本地测试先启用 strict state machine。 |
| BTC | testnet | btc-testnet4 | btc_height | TBD | Planned | 公开测试网验证历史 replay。 |
| BTC | mainnet | btc-mainnet | btc_height | TBD | Planned | BTC 主网 pass 状态机激活高度。 |
| ETHW | devnet | ethw-devnet-<name> | governance | TBD | Planned | ETHW 侧切换到消费 UIP-0002 pass snapshot。 |
| ETHW | mainnet | 主网-mainnet | governance | TBD | Planned | 主网接受 UIP-0002 pass 语义的治理激活点。 |

未列出的网络不得默认激活 UIP-0002。

# 实现影响

参考实现已对齐：

- `src/btc/usdb-indexer/src/index/pass.rs`
- `src/btc/usdb-indexer/src/index/indexer/block_events.rs`
- `src/btc/usdb-indexer/src/balance/monitor.rs`
- `src/btc/usdb-indexer/src/storage/pass.rs`
- `src/btc/usdb-indexer/src/index/energy.rs`
- pass snapshot / history RPC。

当前实现不变量：

- 在 `on_mint_pass` 中先做完整 pre-validation，再提交任何状态变更。
- 将 `prev` 处理从 warn/skip 改为 strict invalid。
- 将 burn 同步写入 energy 终态。
- 保留 ordered block event planner，并把其排序规则作为测试目标。
- 在 Active pass 离开 Active 前执行 block-level balance settlement，并保证同高度 settlement 幂等。
- 在 block-end active balance settlement 中只处理最终仍为 `Active` 的 pass。
- 为 state history 增加同高度多事件 replay 测试。

# 测试要求

最小测试集合：

- valid standard mint creates `Active` pass。
- valid collab mint creates `Active` collab pass but cannot enter independent candidate set。
- invalid schema mint records `Invalid` and does not affect old active pass。
- same owner multi mint: old active becomes `Dormant`, new pass becomes `Active`。
- same owner remint with `prev = [old_active]`: old active becomes `Consumed`, new pass becomes `Active`。
- invalid `prev` owner mismatch makes entire mint `Invalid`。
- missing referenced `prev` makes entire mint `Invalid`。
- duplicate `prev` makes entire mint `Invalid`。
- already consumed `prev` makes entire mint `Invalid`。
- burned `prev` makes entire mint `Invalid`。
- transfer to same owner updates satpoint only。
- transfer to different owner turns active pass into `Dormant` without transfer-specific penalty。
- transfer to different owner after same-height old owner balance decrease applies UIP-0003 balance penalty before freezing。
- same-owner remint after same-height old owner balance decrease applies UIP-0003 balance penalty before inheritance。
- active pass used as same-block `prev` is settled before `Consumed` and inheritance.
- new mint in a block with owner balance decrease uses `balance_after(H)` as initial balance but does not pay pre-mint penalty。
- block-end active balance settlement is idempotent for passes already settled at height `H`。
- dormant transfer keeps frozen raw energy unchanged。
- transfer then remint in same block succeeds only when event ordering puts transfer first。
- burn active pass writes pass state and energy state as `Burned`。
- burn dormant pass returns zero energy after burn height。
- burn consumed pass keeps current economic state as `Consumed`。
- `leader_pass_id` collab mint requires active standard Leader。
- `leader_btc_addr` collab mint accepts valid address and resolves Leader by height。

参考实现的状态机、energy timeline、indexer behavior 和 service tests 已覆盖上述 core 场景；集中 live/regtest 阶段继续复核真实 ord event ordering、burn 和完整 prev 失败矩阵。

# 安全考虑

## 防 `prev` 双花

`prev` 必须 strict invalid，不能 partial success。否则同一份 dormant energy 可能被多个新 pass 重复继承。

## 防余额能量重复

单 owner 单 active pass 必须覆盖 standard 和 collab 两类 pass。否则同一个 BTC owner 的余额会被多张 active pass 重复累计。

## 防历史 replay 分叉

同一 block 的事件排序必须固定。尤其是 transfer + mint + prev 的组合，如果不同节点排序不同，会导致 owner 校验和 pass 状态不同。

同一 block 内 balance settlement boundary 也必须固定。所有节点必须以同一份 block-level balance snapshot 计算 `balance_before / balance_after / delta`，并禁止使用 transaction-level 余额顺序改变 pass energy 结果。

## 防转让绕过余额减少 penalty

different-owner transfer 允许把冻结后的 `raw_energy` 作为历史收益凭证转让，但不得让 `Active -> Dormant` 转让绕过 old owner 在同一高度已经发生的余额减少 penalty。

实现必须保证 Active pass 在转为 `Dormant` 前，已经按 UIP-0003 使用 canonical balance snapshot 完成 old owner 在 event height 的余额 settlement。若该 settlement 包含余额减少 penalty，冻结后的 `raw_energy` 必须是扣除 penalty 后的值。

转让完成后，old owner 后续余额变化与该 `Dormant` pass 解耦；new owner 只能通过 `prev` remint 继承折损后的 raw energy。

## Burn 终态

`Active` / `Dormant` 的 burn 必须同步关闭 pass state 和 energy state。否则 validator 或历史查询可能继续使用 burn 前能量。

# 未决问题

- 同一 height 下是否需要非共识审计 API 暴露 event index；协议状态查询暂不需要。

# 下一步

1. 在集中 live/regtest 中复核 burn、完整 prev invalid 矩阵和 same-block ordering。
2. 在 UIP-0008 activation matrix 中确认正式激活高度和稳定 `network_id`。
3. 后续按审计需求决定是否增加不参与协议状态的 event-index API。
