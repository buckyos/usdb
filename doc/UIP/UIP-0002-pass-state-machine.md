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
- `prev` 严格校验与原子失败规则。

本文不定义 energy 具体公式、继承折损参数或 `effective_energy` 公式。

# 动机

当前实现已经具备可运行的 pass 状态记录和历史记录，但仍有几类规则需要标准化：

- `prev` 引用不存在、owner 不一致、状态不符合要求时，当前实现会 `warn + skip`，然后继续 mint。
- burn 当前主要关闭 pass 状态，但 energy 终态还需要同步封口。
- 同一 block 内 transfer 与 mint 的处理顺序必须成为协议规则，否则历史重放可能分叉。
- UIP-0001 引入 standard pass / collab pass 后，状态机必须明确两类 pass 是否共享 active owner 限制。

UIP-0002 的目标是先固定事件与状态语义，为 UIP-0003 energy 公式和 UIP-0004 collab/effective energy 提供稳定输入。

# 非目标

本文不定义：

- energy 增长公式。
- `prev` 继承折损率和 rounding。
- collab energy 权重。
- level、difficulty、reward split。
- validator payload 的完整字段集合。
- 前端展示状态命名。

# 术语

| 术语 | 含义 |
| --- | --- |
| pass | 符合 UIP-0001 v1 schema 的矿工证铭文。 |
| standard pass | 包含 `eth_main` 的 pass，可独立参与挖矿候选集合。 |
| collab pass | 包含 `leader_pass_id` 或 `leader_btc_addr` 的 pass，不可独立参与挖矿候选集合。 |
| owner | 当前持有 pass 所在 UTXO 的 BTC 地址语义，规范化后以 script hash 或等价确定性 ID 表达。 |
| state | pass 在指定 BTC 高度下的协议状态。 |
| active owner set | 在某一高度所有处于 `Active` 的 pass 按 owner 形成的集合。 |
| event height | 触发状态转换的 BTC block height。 |

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

# 状态转换表

| From | Event | To | 说明 |
| --- | --- | --- | --- |
| none | valid mint | `Active` | 新 pass 成功铸造。 |
| none | invalid mint | `Invalid` | mint schema 或状态前置条件失败。 |
| `Active` | valid mint by same owner | `Dormant` | 旧 active pass 被新 pass supersede。 |
| `Active` | transfer to same owner | `Active` | 仅更新 satpoint。 |
| `Active` | transfer to different owner | `Dormant` | 先冻结 energy，再更新 owner/satpoint。 |
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
2. 如果同一 owner 当前已有 active pass，先在同一 event height 将旧 pass 虚拟视为 `Dormant`，用于后续 `prev` 校验。
3. 如果所有校验通过，才提交状态变更。
4. 若任一校验失败，新 mint 记录为 `Invalid`，不得改变旧 active pass，不得消费任何 `prev`。

提交顺序建议为：

1. 将旧 active pass 写为 `Dormant`，如果存在。
2. 写入新 pass 为 `Active`。
3. 将所有被引用的 `prev` pass 写为 `Consumed`。
4. 写入对应 energy 状态记录。

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

UIP-0002 采用 BTC owner 一致性，而不是 ETH 地址、Leader 地址或 lineage 一致性。

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

- 若 pass 为 `Active`，必须先在 event height 结算 raw energy。
- 若 pass 为 `Active`，必须转为 `Dormant`。
- 必须更新 owner 与 satpoint。
- 若 pass 为 `Dormant`，保持 `Dormant` 并更新 owner/satpoint。
- 若 pass 为 `Consumed` 或 `Burned`，协议不要求继续追踪 owner/satpoint；实现可以保留非共识审计记录，但不得恢复任何经济能力。
- 若 pass 为 `Invalid`，不得进入 active owner set。

new owner 若希望继续使用该 pass 的 raw energy，必须通过新 mint + `prev` 继承流程显式激活。

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

预期需要修改：

- `src/btc/usdb-indexer/src/index/pass.rs`
- `src/btc/usdb-indexer/src/index/indexer/block_events.rs`
- `src/btc/usdb-indexer/src/storage/pass.rs`
- `src/btc/usdb-indexer/src/index/energy.rs`
- pass snapshot / history RPC。

实现建议：

- 在 `on_mint_pass` 中先做完整 pre-validation，再提交任何状态变更。
- 将 `prev` 处理从 warn/skip 改为 strict invalid。
- 将 burn 同步写入 energy 终态。
- 保留 ordered block event planner，并把其排序规则作为测试目标。
- 为 state history 增加同高度多事件 replay 测试。

# 测试要求

最小测试集合：

- valid standard mint creates `Active` pass。
- valid collab mint creates `Active` collab pass but cannot enter independent candidate set。
- invalid schema mint records `Invalid` and does not affect old active pass。
- same owner multi mint: old active becomes `Dormant`, new pass becomes `Active`。
- same owner remint with `prev = [old_active]`: old active becomes `Consumed`, new pass becomes `Active`。
- invalid `prev` owner mismatch makes entire mint `Invalid`。
- missing `prev` makes entire mint `Invalid`。
- duplicate `prev` makes entire mint `Invalid`。
- already consumed `prev` makes entire mint `Invalid`。
- burned `prev` makes entire mint `Invalid`。
- transfer to same owner updates satpoint only。
- transfer to different owner turns active pass into `Dormant`。
- transfer then remint in same block succeeds only when event ordering puts transfer first。
- burn active pass writes pass state and energy state as `Burned`。
- burn dormant pass returns zero energy after burn height。
- burn consumed pass keeps current economic state as `Consumed`。
- `leader_pass_id` collab mint requires active standard Leader。
- `leader_btc_addr` collab mint accepts valid address and resolves Leader by height。

# 安全考虑

## 防 `prev` 双花

`prev` 必须 strict invalid，不能 partial success。否则同一份 dormant energy 可能被多个新 pass 重复继承。

## 防余额能量重复

单 owner 单 active pass 必须覆盖 standard 和 collab 两类 pass。否则同一个 BTC owner 的余额会被多张 active pass 重复累计。

## 防历史 replay 分叉

同一 block 的事件排序必须固定。尤其是 transfer + mint + prev 的组合，如果不同节点排序不同，会导致 owner 校验和 pass 状态不同。

## Burn 终态

`Active` / `Dormant` 的 burn 必须同步关闭 pass state 和 energy state。否则 validator 或历史查询可能继续使用 burn 前能量。

# 未决问题

- `leader_pass_id` 引用的 Leader 是否必须在同一 BTC 高度之前已经存在，还是允许同一 block 内按 canonical event ordering 解析。
- `leader_btc_addr` 在 mint 高度没有 active standard pass 时，是 invalid，还是允许后续高度动态生效。本文倾向后者。
- `Consumed` 之后的非共识审计记录是否需要单独 RPC 暴露。
- 同一 height 下是否需要非共识审计 API 暴露 event index；协议状态查询暂不需要。

# 下一步

1. Review 本草案中的 mint 原子提交顺序。
2. 确认 `prev.owner_at_event_height == new_mint.mint_owner` 作为所有权一致性定义。
3. 确认 `leader_pass_id` mint-time strict validation 与 `leader_btc_addr` dynamic validation 的差异。
4. 基于 UIP-0002 修改 `MinerPassManager::on_mint_pass` 的 pre-validation 路径。
5. 在 UIP-0003 定义 energy 终态记录格式和继承折损，并在 UIP-0004 定义 collab derived energy。
