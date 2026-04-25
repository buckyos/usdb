UIP: UIP-0003
Title: Pass Raw Energy Formula and Inheritance
Status: Draft
Type: Standards Track
Layer: BTC Application / Consensus Input
Created: 2026-04-25
Requires: UIP-0000, UIP-0001, UIP-0002
Supersedes: doc/usdb-economic-model-design.md raw energy sections after activation
Activation: BTC network activation matrix

# 摘要

本文定义 USDB 矿工证的 `raw_energy` 公式、余额变化惩罚、继承折损和终态 energy 语义。

本文的核心边界是：

- `raw_energy(pass, h)` 是某张 pass 自身拥有的基础能量。
- `raw_energy` 可以被历史查询、`prev` 继承和后续 remint 使用。
- `effective_energy`、collab contribution、level、real difficulty 不属于本文范围。
- 任意 collab pass 向 Leader 贡献的折算能量都不得写回 `raw_energy`。

这个边界用于避免 `leader_btc_addr` 自动跟随 Leader remint 时发生能量重叠或双重继承。

# 动机

当前实现已经有可运行的 raw energy 记录与懒结算逻辑，但目标经济模型还需要协议化以下内容：

1. 增长公式必须写成确定性整数规则。
2. 余额减少时的 penalty 应从固定窗口近似收敛为与真实持有年龄相关的公式。
3. `prev` 继承应引入明确折损率，避免无损永续滚动。
4. `Consumed` / `Burned` 的 energy 必须归零，并成为可重放的终态。
5. raw energy 与 UIP-0004 的 effective energy 必须严格分离。

# 非目标

本文不定义：

- standard pass / collab pass 的铭文字段。
- pass 状态机和 `prev` 严格校验。
- Leader 解析、collab 权重和 `effective_energy`。
- level、real difficulty、validator candidate set。
- reward split、价格、发行和辅助算力池。

# 术语

| 术语 | 含义 |
| --- | --- |
| `raw_energy` | pass 自身拥有的基础能量，可冻结、继承和历史查询。 |
| `settled_raw_energy` | 在某一事件高度完成懒结算后的 raw energy。 |
| `projected_raw_energy` | 对 Active pass 从最近 checkpoint 投影到查询高度的 raw energy。 |
| `inheritable_energy` | 某张 pass 可通过 `prev` 传给新 pass 的 raw energy。 |
| `effective_energy` | 挖矿候选和难度计算使用的有效能量，由 UIP-0004/0005 定义，不可继承。 |
| `owner_balance_sats` | pass 当前 owner 在 BTC 侧的可计入余额，单位 sat。 |
| `active_block_height` | 当前 raw energy checkpoint 对应的余额年龄起点。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 全局不变量

## Raw Energy 是唯一可继承能量

新 pass 的初始能量只能来自：

```text
initial_raw_energy(new_pass, h)
    = Σ inheritable_energy(prev_i, h)
```

`effective_energy`、collab contribution、Leader 收到的外部协作能量、level 折算收益都禁止进入 `initial_raw_energy`。

## 每张 Pass 独立累计 Raw Energy

每张 Active pass 的 `raw_energy` 只由以下输入决定：

- 该 pass 自己的历史 checkpoint。
- 该 pass 当前 owner 的 `owner_balance_sats`。
- 从 checkpoint 到目标高度的 BTC block delta。
- 余额变化引发的 penalty 和 `active_block_height` 调整。
- `prev` 继承带来的初始 raw energy。

Leader 关系不得改变 collab pass 自己的 `raw_energy` 计算。

## 终态 Raw Energy

`Consumed`、`Burned`、`Invalid` 在经济口径下的 `raw_energy` 必须为 `0`。

`Dormant` pass 保留冻结时的 `raw_energy`，但不再继续增长。

# 参数草案

| 参数 | 建议值 | 含义 |
| --- | ---: | --- |
| `MIN_BALANCE_SATS` | `100_000` | 余额达到 0.001 BTC 后才开始增长。 |
| `RAW_GROWTH_PER_SAT_BLOCK` | `10_000` | 每 sat 每 BTC block 的 raw energy 增长倍率。 |
| `PENALTY_LAMBDA_NUM` | `3` | penalty 倍率分子。 |
| `PENALTY_LAMBDA_DEN` | `2` | penalty 倍率分母，即 `lambda = 1.5`。 |
| `INHERIT_DISCOUNT_BPS` | `500` | `prev` 继承折损，500 bps = 5%。 |
| `BPS_DENOMINATOR` | `10_000` | bps 分母。 |

所有公式必须使用整数或定点数实现。禁止在共识路径使用浮点数。

# Raw Energy 增长

## 可计入余额

余额阈值规则：

```text
eligible_balance_sats(balance)
    = 0,       if balance < MIN_BALANCE_SATS
    = balance, otherwise
```

本文草案采用“达到阈值后按 sat 线性增长”的规则，以保持与当前实现的增长口径一致。

## 增长公式

Active pass 从 `from_height` 投影到 `to_height` 时：

```text
block_delta = max(0, to_height - from_height)

growth_delta
    = eligible_balance_sats(owner_balance_sats)
      * RAW_GROWTH_PER_SAT_BLOCK
      * block_delta

projected_raw_energy
    = settled_raw_energy + growth_delta
```

当 `to_height <= from_height` 时，`growth_delta = 0`。

## 增长示例

假设 owner 余额不变：

| owner balance | 1 block | 144 blocks | 1008 blocks |
| ---: | ---: | ---: | ---: |
| `99_999` sats | `0` | `0` | `0` |
| `100_000` sats | `1_000_000_000` | `144_000_000_000` | `1_008_000_000_000` |
| `1 BTC` | `1_000_000_000_000` | `144_000_000_000_000` | `1_008_000_000_000_000` |

# 余额增加

当 Active pass 的 owner 余额在高度 `h` 从 `balance_before` 增加到 `balance_after` 时，必须先把旧余额从上一个 checkpoint 懒结算到高度 `h`：

```text
settled_raw_energy_h
    = projected_raw_energy(pass, h, balance_before)
```

余额增加后的推荐 age 规则：

```text
age_before = h - active_block_height_before

remaining_age_after_increase
    = floor(age_before * balance_before / balance_after)

active_block_height_after
    = h - remaining_age_after_increase
```

如果 `balance_before = 0`，则：

```text
active_block_height_after = h
```

该规则表达：新增 BTC 不能免费继承旧余额的全部持有年龄。

当前实现对正向增资不调整 `active_block_height`。因此，本节是 v2 目标规则，进入 Review 前需要审计是否作为激活变更落地，还是保留当前实现口径。

# 余额减少与 Penalty

当 Active pass 的 owner 余额在高度 `h` 从 `balance_before` 减少到 `balance_after` 时：

```text
lost_sats = balance_before - balance_after
age_before = h - active_block_height_before
```

必须先使用减少前余额结算到高度 `h`：

```text
settled_raw_energy_h
    = projected_raw_energy(pass, h, balance_before)
```

目标 penalty 公式：

```text
penalty
    = floor(
        lost_sats
        * age_before
        * RAW_GROWTH_PER_SAT_BLOCK
        * PENALTY_LAMBDA_NUM
        / PENALTY_LAMBDA_DEN
      )

raw_energy_after_penalty
    = max(0, settled_raw_energy_h - penalty)
```

使用建议参数时：

```text
penalty = floor(lost_sats * age_before * 15_000)
```

## 余额年龄折旧

余额减少后，剩余余额只保留按比例折算后的年龄：

```text
remaining_age_after_loss
    = floor(age_before * balance_after / balance_before)

active_block_height_after
    = h - remaining_age_after_loss
```

如果 `balance_after = 0` 或 `balance_after < MIN_BALANCE_SATS`，则：

```text
active_block_height_after = h
```

如果 `balance_before = 0`，该事件不得产生 penalty，且：

```text
active_block_height_after = h
```

## 当前实现兼容说明

当前实现采用固定窗口近似：

```text
penalty_current = lost_sats * 43_200_000
```

该公式可理解为把目标公式中的 `age_before` 固定为 `2880` blocks：

```text
10_000 * 1.5 * 2880 = 43_200_000
```

因此，UIP-0003 的 penalty v2 是协议变更，不应在未配置激活高度时替换当前历史重放结果。

# 继承折损

当新 pass 使用 `prev` 继承旧 pass 时，每个 `prev_i` 的可继承 raw energy 为：

```text
inheritable_energy(prev_i, h)
    = floor(
        settled_raw_energy(prev_i, h)
        * (BPS_DENOMINATOR - INHERIT_DISCOUNT_BPS)
        / BPS_DENOMINATOR
      )
```

使用建议参数时：

```text
inheritable_energy(prev_i, h)
    = floor(settled_raw_energy(prev_i, h) * 9500 / 10000)
```

新 pass 初始 raw energy：

```text
initial_raw_energy(new_pass, h)
    = Σ inheritable_energy(prev_i, h)
```

多 `prev` 必须逐项先折损再求和，禁止先求和再统一折损。这样可以避免 rounding 在不同实现间出现差异。

继承成功后，每个 `prev_i` 必须在同一 event height 写入 `Consumed` energy record：

```text
raw_energy(prev_i, h_after_event) = 0
```

# 状态查询语义

| pass state | `raw_energy(pass, h)` |
| --- | --- |
| `Active` | 从最近 checkpoint 投影到 `h`。 |
| `Dormant` | 返回冻结时的 raw energy。 |
| `Consumed` | `0`。 |
| `Burned` | `0`。 |
| `Invalid` | `0`。 |

历史查询必须以 BTC block 为最小公开粒度。若同一高度存在多条内部事件，公开查询默认返回该 block 所有 canonical events 执行后的最终 energy。

# 数值边界

所有实现必须使用足够宽的中间整数执行乘法。

草案建议：

- 中间计算至少使用 `u128`。
- 对外存储字段若仍为 `u64`，溢出时饱和到 `u64::MAX`。
- 是否把 `u64::MAX` saturation 固化为长期协议语义，仍需在 Review 阶段确认。

# 与 UIP-0004 的边界

UIP-0003 只产出：

```text
raw_energy(pass, h)
inheritable_energy(pass, h)
```

UIP-0004 可以读取 `raw_energy`，但禁止把以下值写回本文定义的状态：

- `collab_contribution`
- `effective_energy`
- Leader 从 collab pass 得到的折算能量
- level 或 difficulty 折算结果

# 安全性

## 防止 Leader Remint 双重继承

如果 Leader 地址从 `pass1` remint 到 `pass2`：

```text
pass2.initial_raw_energy = inheritable_energy(pass1)
```

这里的 `pass1` 只包含 `pass1` 自身的 raw energy，不包含任何 collab pass 折算贡献。

使用 `leader_btc_addr` 绑定的 collab pass 会在 UIP-0004 中重新解析到 `pass2`，并继续以 derived contribution 形式进入 `pass2.effective_energy`。该 contribution 不得被 `pass2` 继承为 raw energy。

## 防止同一余额重复增长

UIP-0002 已规定同一 BTC owner 在同一高度最多只能拥有一张 Active pass。UIP-0003 依赖该不变量防止同一 owner balance 被多张 pass 同时计入 raw energy。

# 测试要求

实现 UIP-0003 时，至少需要覆盖：

- 低于 `MIN_BALANCE_SATS` 不增长。
- 达到阈值后按 sat 线性增长。
- 正向增资后的 age 折算。
- 部分减仓 penalty 和 age 折算。
- 全部减仓后 `active_block_height = h`。
- 单 `prev` 继承折损和 rounding。
- 多 `prev` 逐项折损后求和。
- `Consumed` / `Burned` 查询 energy 为 `0`。
- 溢出时 saturation 行为。

# 审计点

进入 Review 前需要确认：

1. 增长口径是否继续采用当前实现的 sat 级线性增长，还是改为离散 `0.001 BTC` 单位增长。
2. 正向增资是否必须按比例折算 `active_block_height`。
3. `PENALTY_LAMBDA = 1.5` 和 `INHERIT_DISCOUNT_BPS = 500` 是否作为首个正式参数。
4. `u64::MAX` saturation 是否长期成为协议语义。
5. penalty v2 的激活高度是否由 UIP-0007 统一定义。
