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
- collab pass 通过 `leader_pass_id` 或 `leader_btc_addr` 解析到唯一有效 Leader 后，以折算权重贡献给 Leader 的 `effective_energy`。
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

| 参数 | 建议值 | 含义 |
| --- | ---: | --- |
| `COLLAB_WEIGHT_BPS` | `5000` | collab raw energy 按 50% 计入 Leader effective energy。 |
| `BPS_DENOMINATOR` | `10_000` | bps 分母。 |
| `COLLAB_EXIT_PENALTY_BPS` | `1000` | collab 转 standard 时的建议额外折损，1000 bps = 10%。 |

`COLLAB_EXIT_PENALTY_BPS` 是否进入 UIP-0004 首版 Final 仍需审计。若状态转换和 raw energy 修改落在 UIP-0003/0002 的实现路径中，本文只定义触发条件与推荐参数。

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

Dormant collab pass 不再向任何 Leader 贡献能量。它保留自己的冻结 `raw_energy`，但只有重新进入协议允许的 active 形态后才可能继续贡献。具体转换路径由 UIP-0002 和后续实现规则约束。

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

# Leader Eligibility Window

目标经济模型中，Leader 有效性可能还需要 ETHW 侧行为约束，例如“最近一周存在带报价的 USDB 出块”。

本文将 Leader 解析和 Leader eligibility 分开：

```text
resolved_leader(collab, h) -> pass or none
leader_eligible(leader, h) -> true or false
```

首版草案的计算形式为：

```text
collab_contribution(collab, h) = 0,
    if resolved_leader(collab, h) = none
    or leader_eligible(resolved_leader, h) = false
```

`leader_eligible` 的 ETHW 输入、窗口长度、跨链最终性和 validator payload 字段仍需在 UIP-0005/UIP-0006 中进一步定义。若这些输入在 UIP-0004 Final 前尚未固定，UIP-0004 可以先将 `leader_eligible` 默认设为 `true`，只标准化 BTC 侧解析与能量边界。

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

# Collab 退出与转 Standard

collab pass 转 standard pass 会改变其能量的经济用途：从“只能贡献给 Leader”变为“可独立进入 candidate set”。

草案建议：

```text
exit_energy
    = floor(
        raw_energy(collab, h)
        * (BPS_DENOMINATOR - COLLAB_EXIT_PENALTY_BPS)
        / BPS_DENOMINATOR
      )
```

使用建议参数时：

```text
exit_energy = floor(raw_energy(collab, h) * 9000 / 10000)
```

该规则需要和 UIP-0002 的状态转换以及 UIP-0003 的 raw energy 写入规则共同审计。若首版不支持直接转换，则 collab 退出可以先要求 remint，并通过 `prev` 继承折损表达。

# 查询与 Payload 字段

RPC 或 validator payload 应区分返回：

| 字段 | 来源 | 是否可继承 | 用途 |
| --- | --- | --- | --- |
| `raw_energy` | UIP-0003 | 是 | pass 自身资产能量、历史查询、`prev` 继承。 |
| `collab_contribution` | UIP-0004 | 否 | 审计 Leader effective energy 组成。 |
| `effective_energy` | UIP-0004 | 否 | candidate set、level、difficulty 输入。 |

如果 payload 只携带 `effective_energy`，必须能通过同一高度的 `raw_energy`、collab 绑定和 Leader 解析规则重算验证。

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
- collab 退出或 remint 不产生无损独立 effective energy。

# 审计点

进入 Review 前需要确认：

1. `COLLAB_WEIGHT_BPS = 5000` 是否作为首个正式参数。
2. `leader_eligible` 是否在 UIP-0004 首版中默认 `true`，还是必须绑定 ETHW 行为窗口。
3. collab pass dormant 后是否允许通过 remint 重新成为 collab 或 standard。
4. `COLLAB_EXIT_PENALTY_BPS = 1000` 是否进入首版协议。
5. validator payload 是否必须显式携带 `collab_contribution` 明细，还是只携带可重算的 `effective_energy`。
