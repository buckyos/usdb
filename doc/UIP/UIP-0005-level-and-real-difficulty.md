UIP: UIP-0005
Title: Level and Real Difficulty
Status: Draft
Type: Standards Track
Layer: BTC Application / ETHW Validator Input
Created: 2026-04-25
Requires: UIP-0000, UIP-0003, UIP-0004
Supersedes: doc/usdb-economic-model-design.md level and difficulty sections after activation
Activation: BTC and ETHW network activation matrix; development networks activate from height 0 after implementation

# 摘要

本文定义如何从 UIP-0004 的 `effective_energy` 派生矿工证 `level`，以及如何从 `level` 派生难度折算系数 `difficulty_factor_bps`。

本文同时给出 ETHW 侧如何使用 `difficulty_factor_bps` 和动态 `base_difficulty` 计算 `real_difficulty` 的整数规则。`base_difficulty` 和 `real_difficulty` 不属于 USDB indexer 持久状态。

核心规则：

- `level` 只由 `effective_energy` 和 UIP-0005 参数决定。
- `level` 使用整数阈值表计算，禁止运行时使用浮点数、`log` 或平台相关数学库。
- `difficulty_factor_bps` 只由 `level` 和 UIP-0005 参数决定。
- `real_difficulty` 由 ETHW validator / mining policy 使用当前 `base_difficulty` 计算。
- `level`、`difficulty_factor_bps` 和 `real_difficulty` 都是派生值，不可继承、不可写回 raw energy ledger，也不得写入 USDB raw energy 状态。
- collab pass 自身不直接参与 validator candidate set，因此其 `effective_energy = 0`，`level = 0`。

# 动机

当前经济模型大纲给出了目标公式：

```text
level(effective_energy)
    = floor(log_q(1 + (q - 1) * effective_energy / E0))

real_difficulty = difficulty * (1 - level * 0.01)
```

但该公式仍有两个协议化问题：

1. `log` 和小数运算不适合直接作为跨语言、跨平台的共识关键实现。
2. `real_difficulty` 必须定义整数 rounding 和下界，否则可能被折算到 0 或负值。

UIP-0005 的目标是把等级和难度折算变成可重放、可测试、可审计的整数规则。

# 非目标

本文不定义：

- raw energy 增长、惩罚和继承。
- collab pass 的 Leader 解析和 `effective_energy` 聚合。
- ETHW 侧 Leader eligibility、出块报价窗口或 candidate policy。
- base difficulty 的来源、ETHW 出块算法或 PoW target 编码。
- USDB indexer 查询、持久化或反向依赖 ETHW `base_difficulty`。
- reward split、CoinBase 释放和价格规则。

# 术语

| 术语 | 含义 |
| --- | --- |
| `effective_energy` | UIP-0004 派生出的有效能量，是 level 的唯一能量输入。 |
| `level_threshold[L]` | 达到等级 `L` 所需的最小 `effective_energy`。 |
| `level` | 由 `effective_energy` 映射出的非负整数等级。 |
| `base_difficulty` | ETHW validator / mining policy 输入的基础挖矿难度，不是 USDB indexer 输入。 |
| `difficulty_factor_bps` | `level` 对难度产生的折算系数，单位 bps。 |
| `real_difficulty` | ETHW 侧应用 `difficulty_factor_bps` 后的实际难度。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 当前实现状态

当前代码尚未实现本 UIP 定义的 `level` 和 `difficulty_factor_bps`。

在本 UIP 激活前，已有 leaderboard、RPC 或 validator 样例若只使用 raw `energy`，都不应被视为最终协议行为。实现进入本 UIP 后：

- USDB indexer 查询接口应该基于 `effective_energy` 动态计算 `level` 和 `difficulty_factor_bps`。
- USDB indexer 不需要持久化 `level` 或 `difficulty_factor_bps`。
- USDB indexer 不计算、不持久化、不查询 ETHW `base_difficulty` 或 `real_difficulty`。
- ETHW validator / mining policy 使用 `difficulty_factor_bps` 和自己的当前 `base_difficulty` 计算 `real_difficulty`。

# 输入语义

在 BTC 高度 `h` 计算某张 pass 的等级时，必须先按 UIP-0004 计算：

```text
effective_energy(pass, h)
```

然后再计算：

```text
level(pass, h) = level_from_effective_energy(effective_energy(pass, h))
```

对于非 Active pass 或 collab pass，UIP-0004 已定义：

```text
effective_energy(pass, h) = 0
```

因此：

```text
level(pass, h) = 0
```

# 参数草案

| 参数 | 值 | 含义 |
| --- | ---: | --- |
| `LEVEL_E0` | `1_000_000` | 等级曲线的基础能量参数；与 UIP-0003 unit-block energy 量纲匹配。 |
| `LEVEL_Q_NUM` | `118` | 等级曲线公比 `q = 1.18` 的分子。 |
| `LEVEL_Q_DEN` | `100` | 等级曲线公比 `q = 1.18` 的分母。 |
| `MAX_LEVEL` | `50` | 首版最高等级。 |
| `LEVEL_DISCOUNT_BPS` | `100` | 每级降低 1% 难度。 |
| `MAX_DIFFICULTY_DISCOUNT_BPS` | `5000` | 最大降低 50% 难度。 |
| `MIN_DIFFICULTY_FACTOR_BPS` | `5000` | 难度折算系数下界。 |
| `BPS_DENOMINATOR` | `10_000` | bps 分母。 |

`MAX_LEVEL = 50` 与 `MIN_DIFFICULTY_FACTOR_BPS = 5000` 表示矿工证最多只能把基础难度降低到 50%，不得继续下穿。

# 已确认规则

本轮审计已确认：

1. `MAX_LEVEL = 50` 作为首版正式参数。
2. 最大 difficulty discount 固定为 50%，即 `MIN_DIFFICULTY_FACTOR_BPS = 5000`。
3. `level` 和 `difficulty_factor_bps` 是查询时派生值，不需要在 usdb-indexer 中持久化。
4. `base_difficulty` 由 ETHW 提供，USDB indexer 不依赖该值。
5. `real_difficulty` 在 ETHW validator / mining policy 中计算，不写回 USDB indexer。
6. 公开 ETHW 测试网和正式网应使用同一套 level 参数；local / regtest 可以为测试临时 override，但不得影响公开网络 activation matrix。

## `LEVEL_E0` 量纲确认

`LEVEL_E0 = 1_000_000` 来自 issue #23 中的等级模型讨论。该讨论使用的能量量纲是：

```text
UNIT_SATS = 100_000
ENERGY_PER_UNIT_BLOCK = 1
```

这意味着最低 1 个 `balance_unit` 的 active standard pass，每个 BTC block 增长：

```text
1 raw_energy
```

UIP-0003 已采用同一 unit-block 量纲，因此 `LEVEL_E0 = 1_000_000` 不需要再按旧 raw scale 放大。

开发期旧公式曾等价于：

```text
ENERGY_PER_UNIT_BLOCK = 1_000_000_000
```

若保留该旧 scale 而不同步放大 `LEVEL_E0`，会导致最低 1 个 `balance_unit` 在极短时间内达到高等级。UIP-0003 已移除该旧 scale，因此本文保留 issue #23 的 `LEVEL_E0 = 1_000_000`。

# Level 阈值语义

原始连续公式：

```text
level = floor(log_q(1 + (q - 1) * effective_energy / E0))
```

等价于以下阈值规则：

```text
level_threshold(0) = 0

level_threshold(L)
    = ceil(LEVEL_E0 * Σ(i = 0..L-1) (LEVEL_Q_NUM / LEVEL_Q_DEN)^i),
      for 1 <= L <= MAX_LEVEL
```

计算等级时：

```text
level(effective_energy)
    = max L where 0 <= L <= MAX_LEVEL
      and effective_energy >= level_threshold(L)
```

如果：

```text
effective_energy >= level_threshold(MAX_LEVEL)
```

则：

```text
level = MAX_LEVEL
```

实现必须使用整数阈值表或任意精度有理数预计算阈值表。运行时禁止使用浮点数、`log`、`pow` 或平台相关数学库参与共识关键计算。

# 候选阈值表

以下表格使用当前大纲参数生成：

```text
LEVEL_E0 = 1_000_000
LEVEL_Q_NUM = 118
LEVEL_Q_DEN = 100
MAX_LEVEL = 50
```

| Level | `level_threshold` | `difficulty_factor_bps` |
| ---: | ---: | ---: |
| `0` | `0` | `10000` |
| `1` | `1000000` | `9900` |
| `2` | `2180000` | `9800` |
| `3` | `3572400` | `9700` |
| `4` | `5215432` | `9600` |
| `5` | `7154210` | `9500` |
| `6` | `9441968` | `9400` |
| `7` | `12141522` | `9300` |
| `8` | `15326996` | `9200` |
| `9` | `19085855` | `9100` |
| `10` | `23521309` | `9000` |
| `11` | `28755145` | `8900` |
| `12` | `34931071` | `8800` |
| `13` | `42218663` | `8700` |
| `14` | `50818023` | `8600` |
| `15` | `60965267` | `8500` |
| `16` | `72939014` | `8400` |
| `17` | `87068037` | `8300` |
| `18` | `103740283` | `8200` |
| `19` | `123413534` | `8100` |
| `20` | `146627971` | `8000` |
| `21` | `174021005` | `7900` |
| `22` | `206344786` | `7800` |
| `23` | `244486847` | `7700` |
| `24` | `289494480` | `7600` |
| `25` | `342603486` | `7500` |
| `26` | `405272113` | `7400` |
| `27` | `479221094` | `7300` |
| `28` | `566480891` | `7200` |
| `29` | `669447451` | `7100` |
| `30` | `790947992` | `7000` |
| `31` | `934318630` | `6900` |
| `32` | `1103495984` | `6800` |
| `33` | `1303125261` | `6700` |
| `34` | `1538687807` | `6600` |
| `35` | `1816651613` | `6500` |
| `36` | `2144648903` | `6400` |
| `37` | `2531685705` | `6300` |
| `38` | `2988389132` | `6200` |
| `39` | `3527299176` | `6100` |
| `40` | `4163213027` | `6000` |
| `41` | `4913591372` | `5900` |
| `42` | `5799037819` | `5800` |
| `43` | `6843864626` | `5700` |
| `44` | `8076760259` | `5600` |
| `45` | `9531577106` | `5500` |
| `46` | `11248260984` | `5400` |
| `47` | `13273947962` | `5300` |
| `48` | `15664258595` | `5200` |
| `49` | `18484825142` | `5100` |
| `50` | `21813093667` | `5000` |

## 样本等级

在 `ENERGY_PER_UNIT_BLOCK = 1`、无负向余额变化、无继承能量时：

```text
effective_energy = balance_units * age_blocks
```

参考样本：

| BTC 余额 | balance_units | 持有周期 | age blocks | effective_energy | level |
| ---: | ---: | --- | ---: | ---: | ---: |
| `1 BTC` | `1_000` | 1个月 | `4_320` | `4_320_000` | `3` |
| `1 BTC` | `1_000` | 6个月 | `25_920` | `25_920_000` | `10` |
| `1 BTC` | `1_000` | 1年 | `52_560` | `52_560_000` | `14` |
| `1 BTC` | `1_000` | 4年 | `210_000` | `210_000_000` | `22` |
| `10 BTC` | `10_000` | 1年 | `52_560` | `525_600_000` | `27` |
| `100 BTC` | `100_000` | 1年 | `52_560` | `5_256_000_000` | `41` |

# Difficulty Factor 与 Real Difficulty

给定 `level` 后，先计算：

```text
difficulty_discount_bps
    = min(level * LEVEL_DISCOUNT_BPS, MAX_DIFFICULTY_DISCOUNT_BPS)

difficulty_factor_bps
    = BPS_DENOMINATOR - difficulty_discount_bps
```

由于 `MAX_DIFFICULTY_DISCOUNT_BPS = 5000`，所以：

```text
difficulty_factor_bps >= MIN_DIFFICULTY_FACTOR_BPS
```

`difficulty_factor_bps` 是 USDB indexer 可以返回的 BTC-side derived value。它只依赖 `effective_energy`、`level` 和 UIP-0005 参数。

ETHW 侧拿到 `difficulty_factor_bps` 后，结合当前 ETHW `base_difficulty` 计算实际难度：

```text
real_difficulty
    = ceil(base_difficulty * difficulty_factor_bps / BPS_DENOMINATOR)
```

`base_difficulty` 必须是正整数。若 `base_difficulty = 0`，validator / mining policy 必须视为无效输入。

`base_difficulty` 的来源、更新规则、ETHW 网络上下文和 payload 编码由 UIP-0006 或 ETHW mining policy 定义。USDB indexer 禁止为了计算本文字段而查询 ETHW `base_difficulty`。

整数 `ceil` 必须按如下方式实现：

```text
ceil_mul_div(a, b, d) = floor((a * b + d - 1) / d)
```

其中 `d > 0`。实现必须使用足够宽的整数或安全大整数避免 `a * b` 溢出。

## Rounding 原则

`real_difficulty` 使用向上取整，原因是：

- 不会比精确有理数结果更低。
- 可以避免小 `base_difficulty` 在折算后变为 0。
- 对矿工证折扣保持保守口径。

# 查询与 Payload 字段

USDB indexer 查询接口应该携带以下 BTC-side 派生字段：

| 字段 | 类型建议 | 含义 |
| --- | --- | --- |
| `effective_energy` | decimal string | UIP-0004 输出的有效能量。 |
| `level` | integer | 按本文阈值表计算的等级。 |
| `difficulty_factor_bps` | integer | 难度折算系数。 |
| `formula_version` | string | 绑定 UIP-0005 参数版本。 |

这些字段可以动态计算，不需要作为独立状态持久化。实现可以缓存查询结果，但缓存不得改变历史重放语义。

validator payload 可以携带 `level` 和 `difficulty_factor_bps` 作为审计友好的明细字段。validator 必须按本文规则从 `effective_energy` 重算 `level` 和 `difficulty_factor_bps`。若重算结果与 payload 中的字段不一致，必须拒绝该 payload 或将其标记为 invalid。

ETHW block、mining proof 或 validator payload 若携带以下 ETHW-side 字段，其来源和编码由 UIP-0006 或 ETHW mining policy 定义：

| 字段 | 类型建议 | 含义 |
| --- | --- | --- |
| `base_difficulty` | decimal string | ETHW 当前基础难度。 |
| `real_difficulty` | decimal string | ETHW 侧折算后的实际难度。 |

如果 payload 同时携带 `base_difficulty`、`difficulty_factor_bps` 与 `real_difficulty`，ETHW validator 必须重算 `real_difficulty`。若重算结果不一致，必须拒绝该 payload 或将其标记为 invalid。

# 历史查询与 Reorg 语义

`level(pass, h)` 和 `difficulty_factor_bps(pass, h)` 是 BTC 高度相关的派生视图：

```text
level(pass, h)
    = f(effective_energy(pass, h), UIP-0005 parameters at h)
```

因此：

- 历史高度查询必须使用该高度激活的 UIP-0005 参数。
- BTC reorg 改变 pass 状态、raw energy 或 collab contribution 时，`level` 和 `difficulty_factor_bps` 必须随 `effective_energy` 重算。
- ETHW 侧 base difficulty 或 eligibility policy 的变化不得写回 USDB indexer 的 BTC 状态。
- `real_difficulty` 不是单纯 BTC 高度函数，不能作为 USDB indexer 的历史状态字段。

# 与前后 UIP 的边界

| UIP | 边界 |
| --- | --- |
| UIP-0003 | 产出 `raw_energy` 和 `inheritable_energy`；禁止读取或继承 level / difficulty。 |
| UIP-0004 | 产出 `effective_energy`；不定义 level 和 difficulty。 |
| UIP-0005 | 从 `effective_energy` 派生 `level` 和 `difficulty_factor_bps`；定义 ETHW 侧 real difficulty 折算公式。 |
| UIP-0006 | 定义 validator payload 如何携带或验证这些派生字段，以及 ETHW `base_difficulty` 的编码。 |
| UIP-0008 | 定义参数变更、阈值表变更和激活高度。 |

# 安全性

## 禁止浮点共识分叉

不同语言、不同 CPU 和不同标准库对浮点 `log` / `pow` 的边界舍入可能不同。本文用阈值表替代运行时数学函数，避免同一 `effective_energy` 在不同实现中得到不同 `level`。

## 禁止难度下穿

`difficulty_factor_bps` 的下界为 `5000`，因此矿工证最多降低 50% base difficulty。实现不得允许 `level > MAX_LEVEL` 或 `difficulty_factor_bps < MIN_DIFFICULTY_FACTOR_BPS`。

## 禁止派生值继承

以下值都不得进入 UIP-0003 的 `inheritable_energy`：

- `level`
- `difficulty_factor_bps`
- `real_difficulty`
- difficulty discount 带来的任何收益

# 测试要求

实现 UIP-0005 时，至少需要覆盖：

- `effective_energy = 0` 时 `level = 0`。
- 每个 `level_threshold[L] - 1` 映射到 `L - 1`。
- 每个 `level_threshold[L]` 映射到 `L`。
- `effective_energy >= level_threshold[MAX_LEVEL]` 时固定为 `MAX_LEVEL`。
- collab pass 和非 Active pass 的 `level = 0`。
- `difficulty_factor_bps` 不低于 `5000`。
- usdb-indexer 查询可动态返回 `level` 和 `difficulty_factor_bps`，但不需要持久化它们。
- usdb-indexer 不查询、不持久化 `base_difficulty` 和 `real_difficulty`。
- ETHW 侧 `real_difficulty` 使用向上取整，例如 `base_difficulty = 101, factor = 9900` 时结果为 `100`。
- payload 中携带的 `level` / `difficulty_factor_bps` / `real_difficulty` 与重算结果不一致时拒绝。
- 参数表变更时，历史高度按当时激活版本重算。

# 待审计问题

1. `base_difficulty` 的具体来源和数据类型是否应在 UIP-0006 中固定为 ETHW `uint256` decimal string。
2. ETHW validator payload 是否必须显式携带 `base_difficulty` 和 `real_difficulty`，还是只携带可重算输入。
3. local / regtest 的 level 参数 override 如何标识，避免误进入公开网络配置。
