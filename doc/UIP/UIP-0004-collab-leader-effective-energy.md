UIP: UIP-0004
Title: Collab Leader and Effective Energy
Status: Draft
Type: Standards Track
Layer: BTC Application / ETHW Validator Input
Created: 2026-04-25
Requires: UIP-0000, UIP-0001, UIP-0002, UIP-0003
Supersedes: doc/usdb-economic-model-design.md collab and effective energy sections after activation
Activation: BTC and ETHW network activation matrix

# 摘要

本文定义协作矿工证如何解析 Leader，以及如何从 `raw_energy` 派生 `effective_energy`。

核心规则：

- standard pass 可以独立进入 validator candidate set。
- collab pass 不能独立进入 validator candidate set。
- collab pass 自己仍按 UIP-0003 累计 `raw_energy`。
- collab pass 通过 `leader_pass_id` 或 `leader_btc_addr` 解析到唯一 Active standard Leader 后，以折算权重贡献给 Leader 的 `effective_energy`。
- `effective_energy` 是派生值，不可继承、不可写回 raw energy ledger。

# 动机

UIP-0001 已把协作绑定改为由协作者主动声明 Leader：

- `leader_pass_id`
- `leader_btc_addr`

其中 `leader_btc_addr` 可以自动跟随 Leader 地址 remint 出来的新 active pass。这个设计提升了协作者体验，但也引入一个关键风险：

```text
pass1.raw_energy + collab_contribution
    -> Leader remint pass2(prev=pass1)
    -> collab pass 又解析到 pass2
```

如果实现错误地把 collab contribution 写进 `pass1.raw_energy`，那么 `pass2` 会同时继承 collab contribution，并再次从同一批 collab pass 获得 contribution，形成 overlap。

UIP-0004 的目标是把所有协作能量都定义为 derived view，彻底避免这类双重计数。

# 非目标

本文不定义：

- raw energy 增长、惩罚、继承折损。
- pass 状态机。
- level 和 real difficulty 的具体公式。
- ETHW 侧 Leader eligibility、出块历史窗口、报价有效性和最终挖矿准入策略。
- reward split 和协作者收益分配。
- ETHW validator payload 的完整字段集合。

# 术语

| 术语 | 含义 |
| --- | --- |
| Leader | collab pass 引用的 active standard pass。 |
| `leader_ref` | `leader_pass_id` 或 `leader_btc_addr` 的抽象引用。 |
| `resolved_leader` | 在某一 BTC 高度解析出的唯一 active standard pass。 |
| `collab_source_energy` | collab pass 自身的 `raw_energy`。 |
| `collab_contribution` | collab pass 按权重折算后贡献给 Leader 的能量。 |
| `effective_energy` | validator candidate set、level 或 difficulty 可使用的派生能量。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 参数草案

| 参数 | 值 | 含义 |
| --- | ---: | --- |
| `COLLAB_WEIGHT_BPS` | `5000` | collab raw energy 按 50% 计入 Leader effective energy。 |
| `BPS_DENOMINATOR` | `10_000` | bps 分母。 |

# Leader 解析

collab pass 的 Leader 引用来自 UIP-0001 v1 schema。

在 BTC 高度 `h` 计算时，必须先取得高度 `h` 的 pass 状态快照和 raw energy 快照，再执行 Leader 解析。

## `leader_pass_id`

`leader_pass_id` 绑定固定 pass：

```text
candidate = pass_by_inscription_id(leader_pass_id, h)
```

只有当 `candidate` 同时满足以下条件时，解析成功：

- 存在。
- 是 standard pass。
- 状态为 `Active`。
- 未被 burn、consume 或判为 invalid。

否则：

```text
resolved_leader(collab, h) = none
```

`leader_pass_id` 不自动跟随 Leader remint。

## `leader_btc_addr`

`leader_btc_addr` 绑定 BTC 地址在高度 `h` 的 active standard pass：

```text
candidate = active_standard_pass_by_owner(leader_btc_addr, h)
```

只有当该地址在高度 `h` 能解析到唯一 active standard pass 时，解析成功。

由于 UIP-0002 规定同一 owner 同一高度最多一张 Active pass，因此标准实现中该解析应天然唯一。若实现检测到多张 active standard pass，必须视为索引状态不一致，并不得让 collab contribution 进入 candidate set。

`leader_btc_addr` 不需要目标地址在 collab pass mint 时已经出现。地址格式合法即可。若该地址在某一高度没有 active standard pass，则该高度不贡献有效能量。

# Collab Source Energy

collab pass 自己按 UIP-0003 计算 `raw_energy`：

```text
collab_source_energy(collab, h)
    = raw_energy(collab, h), if collab.state(h) = Active
    = 0,                     otherwise
```

Dormant collab pass 不再向任何 Leader 贡献能量。它保留自己的冻结 `raw_energy`，并可按 UIP-0002 / UIP-0003 作为 `prev` 被新 pass 继承。

所有 valid Dormant pass 在 remint 语义上保持一致。UIP-0004 不因旧 pass 是 standard 或 collab 而增加额外限制。

# Collab Contribution

单张 active collab pass 对其 Leader 的贡献：

```text
collab_contribution(collab, h)
    = floor(
        collab_source_energy(collab, h)
        * COLLAB_WEIGHT_BPS
        / BPS_DENOMINATOR
      )
```

如果 `resolved_leader(collab, h) = none`，则：

```text
collab_contribution(collab, h) = 0
```

每张 collab pass 在任一高度最多只能贡献给一个 Leader。

# Effective Energy

## Standard Pass

standard pass 的基础 effective energy：

```text
self_effective_energy(standard, h)
    = raw_energy(standard, h)
```

如果 standard pass 同时是一个或多个 collab pass 的 resolved Leader：

```text
effective_energy(leader, h)
    = raw_energy(leader, h)
      + Σ collab_contribution(collab_i, h)
```

其中 `collab_i` 必须满足：

```text
resolved_leader(collab_i, h) = leader
```

## Collab Pass 独立口径

collab pass 在独立挖矿候选口径下：

```text
effective_energy(collab, h) = 0
```

collab pass 不得直接进入 validator candidate set。它只能通过 Leader 的 `effective_energy` 间接影响候选排序和后续 level/difficulty。

## 非 Active Pass

非 Active pass 的 effective energy：

```text
effective_energy(pass, h) = 0, if pass.state(h) != Active
```

# Leader Eligibility Is ETHW Policy

UIP-0004 不把 ETHW 出块历史或报价窗口反向写入 USDB indexer。

USDB indexer 的职责是按 BTC 高度产出可重放的派生能量：

```text
raw_energy(pass, h)
collab_contribution(pass, h)
effective_energy(pass, h)
```

ETHW validator 或 mining policy 可以在查询 USDB indexer 后，再结合 ETHW 侧本地可验证数据判断该 Leader 的 `effective_energy` 是否可用于出块选择。例如：

```text
leader_eligible(leader, ethw_context)
    = has_recent_quoted_usdb_block(leader, ethw_context)
```

该判断属于 ETHW 侧规则，不得改变 USDB indexer 中任一 pass 的 `raw_energy`、`collab_contribution` 或 `effective_energy`。

因此，UIP-0004 的 collab contribution 公式只依赖 BTC 侧 Leader 解析结果：

```text
collab_contribution(collab, h) = 0,
    if resolved_leader(collab, h) = none
```

# `leader_btc_addr` Remint 示例

假设：

- `leader_addr` 当前 active standard pass 为 `pass1`。
- `collabA` 使用 `leader_btc_addr = leader_addr`。
- 高度 `h1` 时：

```text
effective_energy(pass1, h1)
    = raw_energy(pass1, h1)
      + floor(raw_energy(collabA, h1) * 5000 / 10000)
```

之后 `leader_addr` 在高度 `h2` mint `pass2(prev=[pass1])`。

根据 UIP-0003：

```text
raw_energy(pass2, h2)
    = inheritable_energy(pass1, h2)
```

这里的 `raw_energy(pass1, h2)` 不包含 `collabA` 的 contribution。

根据 UIP-0004：

```text
resolved_leader(collabA, h2) = pass2

effective_energy(pass2, h2)
    = raw_energy(pass2, h2)
      + floor(raw_energy(collabA, h2) * 5000 / 10000)
```

因此 `collabA` 的能量只在每个高度作为派生 contribution 计入一次，不会被 `pass1 -> pass2` 继承链重复吸收。

# Fixed Pass 绑定示例

如果 `collabB` 使用：

```text
leader_pass_id = pass1
```

则 `leader_addr` remint 到 `pass2` 后：

```text
resolved_leader(collabB, h2) = none
```

除非 `pass1` 在高度 `h2` 仍是 active standard pass。协作者如需跟随新 pass，必须重新 mint 或按协议允许的方式转换自己的 collab pass。

# Collab 退出与 Remint

UIP-0004 不定义单独的“退出协作”交易或直接类型转换协议。

collab pass 退出协作或转为 standard pass，必须统一通过 UIP-0002 的新 mint + `prev` 流程完成：

```text
old_collab --prev--> new_pass
```

其中 `new_pass` 的类型完全由新 mint 的 UIP-0001 schema 决定：

- 如果新 mint 包含 `eth_main`，则为 standard pass。
- 如果新 mint 包含 `leader_pass_id` 或 `leader_btc_addr`，则为 collab pass。

能量继承只使用 UIP-0003 的通用 `prev` 继承折损：

```text
initial_raw_energy(new_pass, h)
    = inheritable_energy(old_collab, h)
```

UIP-0004 不定义额外的 `COLLAB_EXIT_PENALTY_BPS`。

这意味着 standard pass 和 collab pass 在 remint 继承上保持一致。只要旧 pass 是 valid `Dormant` pass，且满足 UIP-0002 的 strict `prev` 校验，就可以被新 standard 或新 collab pass 继承。

成功 remint 后：

- `old_collab` 进入 `Consumed`。
- `old_collab.raw_energy = 0`。
- `old_collab` 不再向任何 Leader 贡献 `collab_contribution`。
- `new_pass` 只获得 `old_collab` 自身 raw energy 的 UIP-0003 继承值，不继承旧 Leader aggregation 或旧 collab contribution。

# 查询与 Payload 字段

RPC 或 validator payload 必须区分返回：

| 字段 | 来源 | 是否可继承 | 用途 |
| --- | --- | --- | --- |
| `raw_energy` | UIP-0003 | 是 | pass 自身资产能量、历史查询、`prev` 继承。 |
| `collab_contribution` | UIP-0004 | 否 | 该 pass 作为 Leader 收到的 collab 折算贡献；非 Leader 或无贡献时为 `0`。 |
| `effective_energy` | UIP-0004 | 否 | candidate set、level、difficulty 输入。 |

三者必须满足：

```text
effective_energy = raw_energy + collab_contribution,
    if pass is Active standard pass

effective_energy = 0,
    if pass is collab pass or pass.state != Active
```

按照 UIP-0003，energy 数值在 JSON、RPC 和 validator payload 中必须使用 canonical decimal string。

为了审计 Leader effective energy，查询接口应该提供 per-collab breakdown。最小明细字段建议为：

| 字段 | 含义 |
| --- | --- |
| `collab_pass_id` | 贡献来源 collab pass。 |
| `collab_raw_energy` | collab pass 在高度 `h` 的 raw energy。 |
| `collab_weight_bps` | 折算权重，首版为 `5000`。 |
| `collab_contribution` | 该 collab pass 对 Leader 的折算贡献。 |
| `leader_ref_kind` | `leader_pass_id` 或 `leader_btc_addr`。 |
| `leader_ref_value` | collab pass 声明的 Leader 引用值。 |

实现可以在轻量 snapshot 中只返回 aggregate `collab_contribution`，但 validator payload 或审计查询必须能携带或获取上述明细，以便独立重算。

# 安全性

## 禁止双重计数

实现必须满足：

```text
Σ effective_energy(candidate, h)
```

中，每张 active collab pass 的 `collab_source_energy` 最多出现一次，且只能以折算后的 `collab_contribution` 出现。

## 禁止 Derived Energy 继承

以下值都禁止进入 UIP-0003 的 `inheritable_energy`：

- `effective_energy`
- `collab_contribution`
- Leader aggregation result
- level/difficulty 折算收益

## Remint 退出无额外攻击面

collab pass 退出统一使用 remint + `prev`，不会产生额外双计数空间，原因是：

- 退出前，collab pass 的 `raw_energy` 属于自身，`collab_contribution` 只是 Leader 上的 derived view。
- 成功 remint 后，旧 collab pass 进入 `Consumed`，其 `raw_energy` 归零。
- 旧 collab pass 不再是 Active，因此不再向旧 Leader 贡献 `collab_contribution`。
- 新 pass 只继承旧 collab pass 自身 raw energy 的 UIP-0003 折损值。

因此，统一 remint 规则已经提供退出成本和防双计数边界，不需要额外 `COLLAB_EXIT_PENALTY_BPS`。

## 地址绑定的授权语义

选择 `leader_btc_addr` 的协作者显式接受该地址后续 active standard pass 的变化，包括：

- Leader remint。
- Leader `eth_main` 变化。
- Leader 自身 raw energy 变化。

如果协作者不希望自动跟随，应使用 `leader_pass_id`。

# 测试要求

实现 UIP-0004 时，至少需要覆盖：

- `leader_pass_id` 指向 active standard pass。
- `leader_pass_id` 指向 dormant/consumed/burned/invalid pass。
- `leader_btc_addr` 在无 active pass 时 contribution 为 0。
- `leader_btc_addr` 在 Leader remint 后自动解析到新 active pass。
- collab pass 自身不进入 candidate set。
- collab contribution 不写回 raw energy。
- 多个 collab pass 指向同一 Leader。
- 同一 collab pass 不会贡献给多个 Leader。
- fixed pass 绑定不会自动跟随 remint。
- collab pass remint 为 standard pass 时只继承 UIP-0003 折损后的 raw energy。
- collab pass remint 为新 collab pass 时只继承 UIP-0003 折损后的 raw energy。
- old collab 被 consumed 后不再向旧 Leader 贡献 `collab_contribution`。
- payload 同时携带 `raw_energy`、`collab_contribution`、`effective_energy`，且可由 breakdown 重算。

# 已确认规则

本轮审计已确认：

1. `COLLAB_WEIGHT_BPS = 5000` 作为首版正式参数。
2. ETHW Leader eligibility 不进入 USDB indexer 派生能量公式，由 ETHW validator / mining policy 自行判断。
3. 所有 valid Dormant pass 都按一致规则支持 remint，不区分旧 pass 是 standard 还是 collab。
4. collab 退出统一使用 remint + `prev`，不定义单独转换交易。
5. UIP-0004 不定义 `COLLAB_EXIT_PENALTY_BPS`；退出成本来自 UIP-0003 的通用继承折损。
6. payload / 查询必须区分 `raw_energy`、`collab_contribution`、`effective_energy`，并支持 collab contribution 明细审计。
