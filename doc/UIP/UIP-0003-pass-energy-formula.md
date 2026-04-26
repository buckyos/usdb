UIP: UIP-0003
Title: Pass Raw Energy Formula and Inheritance
Status: Draft
Type: Standards Track
Layer: BTC Application / Consensus Input
Created: 2026-04-25
Requires: UIP-0000, UIP-0001, UIP-0002
Supersedes: doc/usdb-economic-model-design.md raw energy sections after activation
Activation: BTC network activation matrix; development networks activate from height 0

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
| `balance_units` | 将 owner BTC 余额按 `UNIT_SATS` 向下取整后的离散余额单位。 |
| `last_settlement_height` | 最近一条 energy checkpoint 的高度，即 `latest_energy_record.block_height`。 |
| `active_block_height` | 用于 penalty 的余额年龄起点，只由 mint 或余额减少规则调整。 |
| `energy_uint` | 无符号 128 位整数能量值。 |

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

| 参数 | 值 | 含义 |
| --- | ---: | --- |
| `UNIT_SATS` | `100_000` | 1 个离散余额单位，等于 0.001 BTC。 |
| `ENERGY_PER_UNIT_BLOCK` | `1` | 每个 `balance_unit` 每 BTC block 增长的 raw energy。 |
| `PENALTY_LAMBDA_NUM` | `3` | penalty 倍率分子。 |
| `PENALTY_LAMBDA_DEN` | `2` | penalty 倍率分母，即 `lambda = 1.5`。 |
| `INHERIT_DISCOUNT_BPS` | `500` | `prev` 继承折损，500 bps = 5%。 |
| `BPS_DENOMINATOR` | `10_000` | bps 分母。 |
| `ENERGY_MAX` | `2^128 - 1` | `energy_uint` 最大值。 |

所有公式必须使用整数或定点数实现。禁止在共识路径使用浮点数。

# Balance Units

UIP-0003 采用离散余额单位，而不是 sat 级线性增长。

余额换算规则：

```text
balance_units(balance_sats)
    = floor(balance_sats / UNIT_SATS)
```

余额低于 `UNIT_SATS` 时：

```text
balance_units = 0
```

因此低于 0.001 BTC 的余额不增长 raw energy，也不会形成 penalty 单位。

## Unit Delta

余额变化时，必须先分别计算变化前后的 unit 快照，再计算 unit delta：

```text
units_before = floor(balance_before_sats / UNIT_SATS)
units_after  = floor(balance_after_sats  / UNIT_SATS)

gained_units = max(0, units_after - units_before)
lost_units   = max(0, units_before - units_after)
```

禁止使用：

```text
floor(abs(balance_after_sats - balance_before_sats) / UNIT_SATS)
```

直接计算 `lost_units` 或 `gained_units`。该写法会在 unit 边界附近多算或少算。

示例：

| `balance_before_sats` | `balance_after_sats` | sat delta | `units_before` | `units_after` | `lost_units` | `gained_units` |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| `199_999` | `100_000` | `-99_999` | `1` | `1` | `0` | `0` |
| `100_001` | `99_999` | `-2` | `1` | `0` | `1` | `0` |
| `250_000` | `150_000` | `-100_000` | `2` | `1` | `1` | `0` |
| `99_999` | `100_000` | `+1` | `0` | `1` | `0` | `1` |
| `100_000` | `199_999` | `+99_999` | `1` | `1` | `0` | `0` |

# Raw Energy 增长

## 增长公式

Active pass 从最近结算点投影到目标高度时：

```text
last_settlement_height = latest_energy_record.block_height
block_delta = max(0, to_height - last_settlement_height)

growth_delta
    = balance_units(owner_balance_sats)
      * ENERGY_PER_UNIT_BLOCK
      * block_delta

projected_raw_energy
    = settled_raw_energy + growth_delta
```

当 `to_height <= last_settlement_height` 时，`growth_delta = 0`。

`active_block_height` 不参与增长窗口计算。它只用于余额减少时的 penalty age。

## 增长示例

假设 owner 余额不变：

| owner balance | 1 block | 144 blocks | 1008 blocks |
| ---: | ---: | ---: | ---: |
| `99_999` sats | `0` | `0` | `0` |
| `100_000` sats | `1` | `144` | `1_008` |
| `199_999` sats | `1` | `144` | `1_008` |
| `200_000` sats | `2` | `288` | `2_016` |
| `1 BTC` | `1_000` | `144_000` | `1_008_000` |

# 余额增加

当 Active pass 的 owner 余额在高度 `h` 从 `balance_before` 增加到 `balance_after` 时，必须先把旧余额从上一个 checkpoint 懒结算到高度 `h`：

```text
settled_raw_energy_h
    = projected_raw_energy(pass, h, balance_before)
```

然后写入新的 settlement record：

```text
record.block_height = h
record.owner_balance_sats = balance_after
record.raw_energy = settled_raw_energy_h
record.active_block_height = active_block_height_before
```

正向增资禁止重置或折算 `active_block_height`。

后续增长从新的 `record.block_height` 开始，并使用新的 `balance_units(balance_after)` 作为增长斜率。

# 余额减少与 Penalty

当 Active pass 的 owner 余额在高度 `h` 从 `balance_before` 减少到 `balance_after` 时：

```text
units_before = floor(balance_before / UNIT_SATS)
units_after  = floor(balance_after  / UNIT_SATS)
lost_units   = max(0, units_before - units_after)
age_before = h - active_block_height_before
```

必须先使用减少前余额结算到高度 `h`：

```text
settled_raw_energy_h
    = projected_raw_energy(pass, h, balance_before)
```

如果 `units_before = 0` 或 `lost_units = 0`，不得施加 penalty，且 `active_block_height` 保持不变。实现可以写入新的 settlement record 以更新 `owner_balance_sats`，也可以在不影响后续 `balance_units` 的前提下跳过该记录。

目标 penalty 公式：

```text
penalty
    = floor(
        lost_units
        * age_before
        * ENERGY_PER_UNIT_BLOCK
        * PENALTY_LAMBDA_NUM
        / PENALTY_LAMBDA_DEN
      )

raw_energy_after_penalty
    = max(0, settled_raw_energy_h - penalty)
```

使用建议参数时：

```text
penalty = floor(lost_units * age_before * 3 / 2)
```

## 余额年龄折旧

余额减少后，剩余余额只保留按比例折算后的年龄：

```text
remaining_age_after_loss
    = floor(age_before * units_after / units_before)

active_block_height_after
    = h - remaining_age_after_loss
```

如果 `lost_units > 0` 且 `units_after = 0`，则：

```text
active_block_height_after = h
```

使用 `floor(age_before * units_after / units_before)` 是有意选择：剩余 unit 的保留年龄不会超过精确比例值，从而避免少扣年龄。对应的损失年龄会向上取整。

余额减少后的 settlement record：

```text
record.block_height = h
record.owner_balance_sats = balance_after
record.raw_energy = raw_energy_after_penalty
record.active_block_height = active_block_height_after
```

## 当前实现兼容说明

开发期旧实现曾采用 sat 级增长和固定窗口 penalty 近似：

```text
growth_delta_legacy = owner_balance_sats * 10_000 * block_delta
```

该增长公式等价于：

```text
balance_units * 1_000_000_000 * block_delta
```

这会把 issue #23 中讨论的 unit-block 能量模型整体放大 `1_000_000_000` 倍，并导致 UIP-0005 的 `LEVEL_E0 = 1_000_000` 失去原始量纲。UIP-0003 不保留该 scale 作为正式协议语义。

旧 penalty 近似为：

```text
penalty_current = lost_sats * 43_200_000
```

该公式可理解为旧 `1_000_000_000` raw scale 下，把 `age_before` 固定为 `2880` blocks 的近似：

```text
10_000 * 1.5 * 2880 = 43_200_000
```

UIP-0003 不保留该近似作为正式协议语义。当前开发网络可以从高度 `0` 重建并使用本节定义的 unit-block growth 与 unit penalty。

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

`settled_raw_energy`、乘法中间值和结果都按 `energy_uint` 处理。

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

## 内部表示

UIP-0003 的 energy 类型为：

```text
energy_uint = uint128
ENERGY_MAX  = 2^128 - 1
```

所有 `raw_energy`、`settled_raw_energy`、`projected_raw_energy`、`inheritable_energy` 都必须使用 `energy_uint`。

协议语义定义为：

```text
energy_result = min(exact_integer_result, ENERGY_MAX)
```

也就是说，乘法、加法和多 `prev` 求和必须等价于“先用无限精度整数计算，再在最终写入 energy 字段时 saturate 到 `ENERGY_MAX`”。实现可以使用更宽整数、checked arithmetic 或 arbitrary precision，只要结果一致。

## JSON / RPC / Validator Payload 表示

任何 JSON、RPC、validator payload 或跨语言接口中的 energy 值必须编码为十进制字符串：

```json
{
  "raw_energy": "1000000000000000000"
}
```

规则：

- 使用 base-10 ASCII 字符串。
- 禁止前导零，数值 `0` 只能编码为 `"0"`。
- 禁止小数点、科学计数法、正负号和空白。
- 解码后必须满足 `0 <= value <= ENERGY_MAX`。

原因是 JavaScript number 和部分 JSON consumer 无法精确表示超过 `2^53 - 1` 的整数。

## Saturation 风险估算

使用 `UNIT_SATS = 100_000` 和 `ENERGY_PER_UNIT_BLOCK = 1` 时：

| BTC balance | units | energy / block | 到达 `u128::MAX` 的约略时间 |
| ---: | ---: | ---: | ---: |
| `1 BTC` | `1_000` | `1_000` | 约 `6.5e30` 年 |
| `1_000 BTC` | `1_000_000` | `1_000_000` | 约 `6.5e27` 年 |
| `21_000_000 BTC` | `21_000_000_000` | `21_000_000_000` | 约 `3.1e23` 年 |

因此 `uint128` saturation 是协议兜底，正常经济场景不会触发。

# 激活语义

USDB 仍处于开发阶段，UIP-0003 不需要兼容开发期旧公式。

当前开发网络和后续全量重放环境可以按如下语义处理：

```text
activation_height = 0
```

正式公开网络发布后，未来对 energy 公式的修改必须通过 UIP-0007 或后续版本激活机制定义，不得再隐式从高度 `0` 改写历史。

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

- 低于 `UNIT_SATS` 不增长。
- 达到 `UNIT_SATS` 后按离散 `balance_units` 增长。
- unit 边界附近的增减仓计算，例如 `100_001 -> 99_999` 和 `199_999 -> 100_000`。
- 正向增资只写入新的 settlement record，不重置或折算 `active_block_height`。
- 部分减仓 penalty 和 age 折算。
- 全部减仓后 `active_block_height = h`。
- 单 `prev` 继承折损和 rounding。
- 多 `prev` 逐项折损后求和。
- `Consumed` / `Burned` 查询 energy 为 `0`。
- `uint128` energy 的内部计算、saturation 和 JSON decimal string 编码。

# 已确认规则

本轮审计已确认：

1. 增长口径采用离散 `0.001 BTC` unit 模型。
2. unit delta 必须通过 `units_before` / `units_after` 快照计算，不得通过 sat delta 直接取整。
3. 正向增资只更新 settlement height 和 owner balance，不重置、不折算 `active_block_height`。
4. 首版参数固定为 `PENALTY_LAMBDA = 1.5`、`INHERIT_DISCOUNT_BPS = 500`。
5. `ENERGY_PER_UNIT_BLOCK = 1`，与 issue #23 的 unit-block 能量量纲保持一致。
6. energy 内部类型采用 `uint128`，跨语言接口使用 canonical decimal string。
7. 当前开发阶段按高度 `0` 激活 UIP-0003；未来正式网络升级由 UIP-0007 处理。

# 后续实现风险

实现层仍需专项处理：

- RocksDB `PassEnergyValue.energy` 当前为 `u64`，改为 `u128` 会改变 bincode 编码；开发期可以重建 DB，正式网络需要迁移版本。
- RPC 结构体当前以 JSON number 返回 `energy`，必须切换为 decimal string。
- 前端 TypeScript 类型当前使用 `number`，必须切换为 `string` 并只在展示层格式化。
- validator payload 和 state ref 若包含 energy，也必须使用 decimal string 并按本文规则 canonicalize。
