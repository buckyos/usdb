UBIP: UBIP-01
Layer: Application / Consensus
Title: USDB 经济模型设计
Author: <TODO>
Discussions-To: <TODO>
Status: Draft
Type: Standards Track
Created: 2026-04-10
License: <TODO>
Requires: 矿工证铭文协议, usdb-indexer-rpc-v1, usdb-indexer-validator-block-body-e2e-design

# 摘要

本文定义 USDB 的统一经济模型草案

本文覆盖：

- 矿工证资产对象与状态机。
- 能量（`energy`）的抽象公式与实现兼容映射。
- 协作矿工证、Leader、有效能量（`effective_energy`）。
- 等级（`level`）、难度（`difficulty` / `real_difficulty`）。
- CoinBase 释放、手续费分配、辅助算力池、DAO 分红池。
- `price` / `real_price` 价格机制。
- reorg、历史查询、共识输入边界与版本治理。

还未确定的内容，本文统一使用 `<TODO>` 标记。

# 动机

当前仓库中，USDB 经济相关规则分散在：

- 矿工证铭文协议与设计文档。
- `usdb-indexer` 的 pass / energy 实现。
- `review.md` 一类讨论稿。
- validator payload、RPC、错误契约等外围文档。

现状已经具备一套可运行的“矿工证状态与能量索引模型”，但仍缺少一份把下列层次统一收口的规范：

- 术语、状态、历史高度语义。
- 经济公式与参数。
- 事件驱动规则。
- 共识输入与查询字段边界。
- 协议版本与公式版本治理。

本文的目标不是复述代码，而是给出“目标经济模型”的单一规范，并明确：

- 哪些规则已经在当前实现中落地。
- 哪些规则仍是目标设计。
- 从当前实现迁移到目标模型时，需要收敛的差异点。

# 术语与解释约定

本文中的关键词“必须”、“禁止”、“应该”、“可以”分别表示强约束、强禁止、建议约束与可选实现。

除非另有说明，本文中的“区块高度”均指 BTC 区块高度。

## 术语

- **矿工证 / pass**：由 BTC 铭文表达的核心资产对象。
- **标准矿工证**：可独立参与挖矿难度计算与收益结算的矿工证。
- **协作矿工证 / collab pass**：不能独立用于挖矿、但可向 Leader 提供有效能量加成的矿工证。
- **Leader**：在某一统计窗口内满足有效性条件、并可接受协作矿工加成的矿工证。
- **Owner**：矿工证当前对应的 BTC 地址所有者语义。
- **Active**：矿工证处于活跃状态，允许继续增长 energy。
- **Dormant**：矿工证已冻结，不再增长 energy，但可被后续 `prev` 继承消耗。
- **Consumed**：矿工证已被后续 `prev` 消耗继承，energy 清零。
- **Burned**：矿工证已被销毁，不可再参与任何经济行为。
- **Invalid**：矿工证或状态转换不满足协议约束。

## 记号

- `energy`：矿工证能量。
- `effective_energy`：用于等级与难度计算的有效能量。
- `balance_units`：矿工证 owner 当前有效 BTC 余额的抽象单位。
- `lost_units`：余额减少时损失的抽象余额单位。
- `active_block_height`：当前活跃增长窗口的起始 BTC 高度。
- `current_block_height`：当前 BTC 高度。
- `H_now = current_block_height - active_block_height`。
- `level`：挖矿等级。
- `difficulty`：算法标称难度。
- `real_difficulty`：应用等级折减后的实际难度。
- `price`：BTC 算法价格。
- `real_price`：出块者可更新的实际价格。
- `CoinBase`：一个区块释放的 USDB 总数。
- `K`：协作矿工效率系数。
- `CE`：当前有效 Leader 在出块时，其协作矿工能量总和。
- `AE`：过去一周有效 Leader 出块时的协作矿工能量平均值。

# 设计原则

## 1. 分层收敛

本文把规则拆成三层：

1. **目标经济模型**：本文规范性内容。
2. **当前实现兼容层**：用于解释仓库当前行为与目标模型的对应关系。
3. **待收敛项**：尚未在协议或实现中确定的内容，以 `<TODO>` 标注。

## 2. 代码与规范的关系

对“后续协议目标与实现方向”的解释，以本文为准。

## 3. 确定性优先

所有进入 validator、难度、奖励和供给计算的字段，都必须能够由确定性输入重放得到。任何只用于 UI、排行榜或缓存优化的字段，不得反向影响共识结果。

# 非目标

本文暂不定义：

- 具体前端显示口径。
- 完整 DeFi 合约 ABI 与撮合细节。
- 叔块奖励在 EVM 链侧的全部执行细节。
- 跨链桥、WBTC 铸造与赎回协议的完整规范。

这些内容可以在后续子规范中补充，但不得与本文的经济结算语义冲突。

# 规范

## 1. 规则来源与优先级

USDB 经济规则的解释优先级应为：

1. 当前已激活的协议版本与公式版本。
2. 本文规范性章节。
3. 参考实现与测试。
4. 讨论稿与设计说明。


## 2. 资产对象与铭文字段

### 2.1 基础对象

USDB 的核心资产对象是矿工证。矿工证由 BTC 铭文表达，当前已知支持的 mint 语义包括：

- `p == "usdb"`
- `op == "mint"`

### 2.2 字段

当前实现已经识别的字段包括：

- `eth_main`
- `eth_collab`
- `prev`

目标模型中：

- `eth_main` 必须存在，并且必须是合法 EVM 地址。
- `eth_collab` 协作矿工证必须存在，指向其Leader矿工的eth_main地址
- `prev` 是被继承矿工证列表，用于 remint 与能量继承。

### 2.3 协作关系字段

当前实现只保留了 `eth_collab` 地址校验，但这不足以唯一表达“协作矿工证绑定哪个 Leader 矿工证”。

因此，目标协议必须增加或明确以下之一：

- 绑定 `leader_pass_id`；或
- 绑定 `leader_btc_addr`；

## 3. 状态机

### 3.1 Pass 状态

矿工证状态定义为：

- `Active`
- `Dormant`
- `Consumed`
- `Burned`
- `Invalid`

### 3.2 状态语义

- `Active`：允许 energy 持续增长，允许用于难度/收益相关派生计算。
- `Dormant`：energy 冻结，不再增长；可作为 `prev` 被消耗继承。
- `Consumed`：已被 `prev` 消耗继承；energy 必须为 `0`。
- `Burned`：已销毁；energy 必须为 `0`，不可再次激活，不可再次继承。
- `Invalid`：不满足协议条件；不得参与任何经济计算。

### 3.3 单地址单活跃矿工证

任一 BTC owner 在同一时刻必须最多只有一张 `Active` pass。

当同一 owner 成功 mint 新 pass 时：

1. 旧 `Active` pass 必须先结算到当前高度。
2. 旧 pass 必须转为 `Dormant`。
3. 新 pass 才能写入为 `Active`。

## 4. 所有权与事件驱动规则

### 4.1 mint

当一张新 pass 被 mint 时：

- 如果没有 `prev`，其初始 `energy = 0`。
- 如果存在 `prev`，必须先完成 `prev` 校验与继承结算。
- mint 成功后，新 pass 必须写入一条当前高度的 `Active` energy 记录。

### 4.2 transfer 到新 owner

当 `Active` pass 转移给新 owner 时：

1. 必须先把该 pass 的 energy 结算到转移高度。
2. 该 pass 必须转为 `Dormant`。
3. owner 与 satpoint 更新到新位置。
4. 该 pass 不再继续增长 energy。

新 owner 若希望继续使用该 pass 的能量，必须通过新的 mint + `prev` 继承流程重建 `Active` pass。

### 4.3 同 owner 的 UTXO 搬移

如果铭文 UTXO 只是转移到同一 owner：

- 必须只更新 satpoint。
- 不得改变 pass owner。
- 不得改变 pass state。
- 不得额外扣除 energy。

### 4.4 burn

当 pass 被 burn 时：

- pass state 必须写为 `Burned`。
- energy 状态机必须同步写入 `Burned` 记录。
- `Burned` 记录的 `energy` 必须为 `0`。
- 任何查询都不得再从 burn 之前的 `Active` 记录继续投影能量。

这条规则用于消除当前实现中 pass snapshot 与 energy snapshot 可能分叉的问题。

### 4.5 remint(prev)

`prev` 继承必须采用严格校验，而不是“warn + skip”。目标规则如下：

对 `prev` 中的每个引用：

- 必须存在。
- 必须未被重复消费。
- 必须当前处于 `Dormant`。
- 必须满足所有权一致性约束。`<TODO: 精确定义为 owner 相同、控制权相同还是 lineage 相同>`
- 不得重复出现在同一个 `prev` 列表中。

如果任一 `prev` 不满足条件，则本次 mint 必须判为 `Invalid`，不得部分成功。

这条规则与当前实现的宽松继承不同，属于后续版本升级项。

## 5. 能量模型

## 5.1 抽象单位

本文采用抽象余额单位：

- `1 BTC = 1000 balance_units`
- 等价地，`1 balance_unit = 0.001 BTC = 100_000 sats`

因此：

```text
balance_units = owner_balance_sats / 100_000
lost_units    = abs(owner_delta_sats) / 100_000
```

这里的 `balance_units` 与 `lost_units` 允许按定点数或有理数表示；实现不得使用浮点非确定性计算。`<TODO: 推荐的定点精度>`

## 5.2 最低增长阈值

当 `balance_units < 1` 时，pass 不增长 energy。

该规则与当前实现中“余额低于 `0.001 BTC` 不增长 energy”的行为一致。

## 5.3 每区块增长语义

对任意处于 `Active` 状态的 pass，在不考虑余额减少惩罚时，其 per-block 语义为：

```text
if balance_units >= 1:
    energy(next_block) = energy(current_block) + balance_units
else:
    energy(next_block) = energy(current_block)
```

也就是说，资产规模越大，能量增长斜率越大；资产放大 100 倍，能量成长速度也放大 100 倍。

## 5.4 余额增加

当 owner 余额增加时：

- 既有 energy 不回溯修改。
- 从该变化点开始，后续每区块增长斜率按新的 `balance_units` 计算。
- `active_block_height` 不因正向增资而重置。

## 5.5 余额减少与惩罚

当 owner 余额减少时，系统必须在变化点立即施加惩罚：

```text
H_now   = current_block_height - active_block_height
penalty = lost_units * H_now * lambda
lambda  = 1.5
```

惩罚后的能量为：

```text
energy' = max(0, energy - penalty)
```

为了表示“部分余额被抽走以后，剩余余额的年龄被按比例折旧”，本文将原讨论式进一步明确为：

```text
active_block_height'
    = active_block_height
    + (lost_units / balance_units_before_loss) * H_now
```

其中 `balance_units_before_loss` 表示本次余额减少发生前的余额单位数。

如果 `lost_units >= balance_units_before_loss`，则：

```text
active_block_height' = current_block_height
```

这样可避免分母为零，并表达“余额被全部抽走后，增长窗口完全重置”。

## 5.6 事件驱动实现等价性

实现不要求真的在每个区块写一条 energy 记录；可以采用“变化点懒结算”的方式，只要满足：

- 对任意目标高度的历史查询结果，与 per-block 语义严格等价。
- `Active` 状态下的未来投影只能基于最后一条合法 checkpoint 继续计算。
- `Dormant` / `Consumed` / `Burned` 不得再向未来投影增长。

## 5.7 初始能量与继承能量

新 pass 的初始能量定义为：

```text
initial_energy = Σ inherit(prev_i)
```

目标模型建议：

```text
inherit(prev_i) = dormant_energy(prev_i) * (1 - INHERIT_DISCOUNT)
INHERIT_DISCOUNT = 0.05
```

即默认继承折损率为 `5%`。

### 5.7.1 当前实现兼容说明

当前实现尚未落地继承折损，行为更接近：

```text
inherit(prev_i) = dormant_energy(prev_i)
```

因此，继承折损属于目标模型与当前实现之间的显式差异。

### 5.7.2 被继承旧 pass 的处理

每个被成功继承的旧 pass 必须：

- 写入一条 `Consumed` energy 记录。
- 将 `energy` 写为 `0`。
- 不得再次被后续 `prev` 使用。

## 5.8 Dormant / Consumed / Burned 的能量语义

- `Dormant`：冻结最后结算时点的能量值，不再继续增长。
- `Consumed`：能量必须为 `0`。
- `Burned`：能量必须为 `0`。

任何历史查询如果命中 `Dormant`，只能返回冻结值；如果命中 `Consumed` 或 `Burned`，只能返回 `0`。

## 5.9 抽象公式到当前实现常量的映射

为了兼容当前 `usdb-indexer` 的 raw energy 实现，建议采用以下解释层：

```text
BALANCE_UNIT_SATS = 100_000
ENERGY_SCALE      = 1_000_000_000
```

则每区块的 raw energy 增长可写为：

```text
growth_delta_raw
    = balance_units * ENERGY_SCALE
    = (owner_balance_sats / 100_000) * 1_000_000_000
    = owner_balance_sats * 10_000
```

这与当前实现中的：

```text
growth_delta = owner_balance * 10_000 * r
```

是等价的。

对于惩罚项，目标模型的 raw 形式应为：

```text
penalty_raw_target
    = lost_units * H_now * 1.5 * ENERGY_SCALE
    = abs(owner_delta_sats) * H_now * 15_000
```

当前实现采用：

```text
penalty_raw_current = abs(owner_delta_sats) * 43_200_000
```

可将其理解为把 `H_now` 近似固定为 `2880` blocks 时的兼容常量：

```text
15_000 * 2880 = 43_200_000
```

因此：

- 当前实现的增长模型，与本文抽象模型可以严格对齐。
- 当前实现的惩罚模型，可理解为对 `lambda * H_now` 的固定窗口近似，而非完整目标模型。

## 6. 协作矿工证与 Leader

## 6.1 协作矿工证

协作矿工证在经济上具有如下目标语义：

- 其基础 energy 曲线与标准矿工证一致。
- 它不能独立用于挖矿难度计算或出块资格。
- 它的 energy 可按权重并入某个有效 Leader 的 `effective_energy`。

### 6.1.1 创建时绑定

协作矿工证在创建时必须绑定一个 Leader 引用 `leader_ref`。当前协议字段尚未完全确定。`<TODO>`

### 6.1.2 退出协作关系

协作矿工证通过一次交易后，可以转换为普通矿工证，但该过程应伴随能量损失(损失10%）。由于当前文档未给出精确附加损失参数，本文保留：

- `COLLAB_EXIT_PENALTY = <TODO>`

在 `COLLAB_EXIT_PENALTY` 未确定前，至少必须触发一次与普通所有权转移一致的冻结/继承流程，不得无损从协作态直接变成独立挖矿态。

## 6.2 Leader 有效性

一个矿工证要成为“有效 Leader”，至少需要满足：

- 在最近一周内，存在一次“带报价的 USDB 出块”。


## 6.3 有效能量

普通矿工证的有效能量：

```text
effective_energy = energy
```

有效 Leader 的有效能量：

```text
effective_energy
    = self_energy + Σ(collab_energy_i * COLLAB_WEIGHT)
COLLAB_WEIGHT = 0.5
```

也即：

- Leader 自身 energy 100% 计入。
- 其所有有效协作矿工证 energy 的 50% 计入。

### 6.3.1 协作矿工证自身的有效能量

为避免双重计数，协作矿工证在“独立挖矿资格”口径下的 `effective_energy` 应视为 `0`；其基础 `energy` 只通过所属有效 Leader 折算进入 `effective_energy`。

### 6.3.2 Leader 无效时

当 Leader 不再满足有效性窗口时：

- 协作矿工 energy 不再计入其 `effective_energy`。
- Leader 自身基础 `energy` 仍保留。

## 7. 等级与难度

## 7.1 等级公式

等级由有效能量决定：

```text
level(effective_energy)
    = floor(log_q(1 + (q - 1) * effective_energy / E0))
```

其中：

```text
E0 = 1_000_000
q  = 1.18
```

若 `effective_energy = 0`，则 `level = 0`。

## 7.2 实际难度

矿工实际难度为：

```text
real_difficulty = difficulty * (1 - level * 0.01)
```

### 7.2.1 下界约束

实现不得允许 `real_difficulty <= 0`。实际上的约束，是level很难超过50
也就是说，最多通过矿工证，降低1半的USDB PoW难度


## 8. 收益与发行

## 8.1 区块总收益

一个区块的总收益定义为：

```text
block_reward_total = CoinBase + tx_fees
```

## 8.2 CoinBase 释放公式

目标释放公式为：

```text
CoinBase
    = K * (TOTAL_MINER_BTC * price - ISSUED_USDB) / EMISSION_BLOCKS
EMISSION_BLOCKS = 157_680
```

其中：

- `TOTAL_MINER_BTC`：所有矿工证中BTC余额 总量。统计口径待明确
- `ISSUED_USDB`：截至当前区块已发行的 USDB 总量。
- `price`：BTC 算法价格。
- `K`：协作效率系数。


<TODO> 当满足下列条件是，CoinBase为0


## 8.3 协作效率系数 K

`K` 与当前出块 Leader 的协作能量表现有关：

```text
CE = 当前有效 Leader 出块时，其协作矿工能量总和
AE = 过去 1 周有效 Leader 出块时的协作矿工能量平均值
```

必须满足：

```text
0.8 < K <= 2.0
```


### 8.3.1 候选函数

讨论稿给出了一段候选函数，但其 `max(..., 2.0)` 与上界约束冲突。因此本文只把“区间约束”和“单调性目标”作为规范，函数本体暂定为 `<TODO>`。

建议的有界候选函数为：

```python
def compute_k(current, avg):
    if avg <= 0:
        return 1.0
    r = current / avg
    if r < 1.0:
        return 2.0 - 6.0 / (r + 5.0)
    return min(1.0 + (r - 1.0), 2.0)
```

最终采用的函数必须：

- 单调不减。
- 满足 `0.8 < K <= 2.0`。
- 在所有验证节点上结果完全一致。

## 8.4 CoinBase 与手续费分配

### 8.4.1 辅助算力池未启用时

当辅助算力池未启用时：

- 矿工获得全部 `CoinBase`。

### 8.4.2 辅助算力池启用时

当辅助算力池启用时：

- 矿工基础 CoinBase 份额为 `75%`。
- 辅助算力池份额为 `25%`。

即：

```text
miner_coinbase_base = CoinBase * 0.75
aux_pool_reward     = CoinBase * 0.25
```

### 8.4.3 叔块奖励兼容

讨论稿要求“矿工的实际 CoinBase 收入在上述数值上应用 ETH 的叔块奖励规则”。

由于当前尚未确定：

- 采用哪一版 ETH 规则；
- 对应区块/叔块关系的链上表达；
- 与 BTC 出块映射的精确语义；

本文保留：

- `UNCLE_REWARD_RULE = <TODO>`

在该规则明确前，`miner_coinbase_base` 只表示叔块奖励应用前的基础份额。

### 8.4.4 手续费分配

手续费分配目标为：

```text
miner_fee_reward = tx_fees * 0.60
dao_fee_reward   = tx_fees * 0.40
```

其中：

- `miner_fee_reward` 进入矿工收入合约。
- `dao_fee_reward` 进入 BDT DAO 分红池。

### 8.4.5 收入合约

矿工可以为自己的收益指定收入合约地址，用于后续与协作矿工的分红规则定制。

- 合约内部分红逻辑不在本文定义范围内。
- 但进入收入合约的总额必须与本文分账公式一致。

### 8.4.6 不可停止约束

讨论稿指出：

- 辅助算力池合约一旦设置，不可停止。
- DAO 分红池合约一旦设置，不可停止。

其链上治理与配置入口仍需单独规范。`<TODO>`

## 9. 辅助算力池

辅助算力池的目标逻辑为：

- 持有矿工证的 BTC 矿工可以支付手续费，向辅助算力池提交有效 BTC 算力。
- 只接受最近 `2` 个区块高度以内的有效算力提交。
- 有效算力门槛为：提交的有效 BTC 算力应大于 BTC 出块难度的 `75%`。

当前尚未明确的关键点包括：

- 提交的证明格式。`<TODO>`
- 重复提交如何处理。`<TODO>`
- 多提交者竞争同一奖励时如何分配。`<TODO>`
- 算力证明与矿工证 owner 的绑定方式。`<TODO>`

在上述细节未明确前，辅助算力池只能视为目标机制，不应作为已完成的共识组件对外宣称。

## 10. BTC 算法价格与实际价格

## 10.1 实际价格更新权

出块者可以更新 `real_price`，但必须满足内置 DeFi 合约约束。

## 10.2 更新约束

要更新 `real_price`，出块者必须同时满足：

1. 存在以 `real_price * 0.8` 挂出的买单（使用 USDB 购买 WBTC），总量不小于：
   ```text
   min(1 WBTC, miner_btc_balance * 5%)
   ```
2. 存在以 `real_price * 1.2` 挂出的卖单（使用 WBTC 购买 USDB），总量不小于：
   ```text
   min(1 WBTC, miner_btc_balance * 5%)
   ```

其中 `miner_btc_balance` 的统计口径尚需明确：

- 是当前出块 pass 的 owner BTC 余额；
- 还是 Leader 关联资产口径；
- 还是某个收益合约净值口径。

记为 `<TODO>`。

## 10.3 `price` 向 `real_price` 收敛

全局 `price` 应按如下规则逐步向 `real_price` 收敛：

```text
if price < real_price:
    price = min(price * 1.001, real_price)
elif price > real_price:
    price = max(price * 0.999, real_price)
else:
    price = price
```

## 10.4 初始值与更新时机

以下参数仍需明确：

- `price` 初始值。`100000`
- `real_price` 的初始值。`100000`
- 价格更新是在每个BTC高度变化时应用


## 11. 历史查询、共识输入与查询边界

## 11.1 历史查询

系统必须支持至少以下历史能力：

- 按精确高度查询 pass state。
- 按精确高度或 `at_or_before` 查询 energy state。
- 在 `Active` 状态下，基于最近合法 checkpoint 投影到目标高度。

## 11.2 共识关键输入

以下数据一旦参与出块资格、收益、供给或难度判断，即属于共识关键输入：

- pass 状态与状态转换结果。
- owner 与 owner 余额在指定高度的确定值。
- energy checkpoint、`active_block_height` 与查询语义。
- `prev` 校验结果与消费状态。
- Leader 绑定与有效性窗口。
- `effective_energy`、`level`、`real_difficulty`。
- `price` / `real_price` 及其证明。
- `TOTAL_MINER_BTC`、`ISSUED_USDB`、`CE`、`AE`、`K`。
- 公式版本与协议版本。

## 11.3 查询专用字段

以下内容可以作为查询层或缓存层字段，但不得反向影响共识：

- 排行榜结果。
- UI 显示格式化值。
- 非规范化的近实时预测缓存。
- 非共识必须的聚合索引。

## 12. Reorg 与回滚

当 BTC 发生 reorg 时，系统必须：

1. 回滚到共同祖先高度。
2. 删除共同祖先之后由被回滚分支产生的 pass/energy/price/leader 相关派生状态。
3. 依据新分支顺序重新回放。

以下内容在回滚后都必须可重放恢复：

- pass 状态机。
- energy checkpoint 与历史查询结果。
- `prev` 消费关系。
- Leader 有效窗口。
- `CE` / `AE` / `K`。
- `price` / `real_price`。
- 所有奖励与供给累计量。

任何仅依赖缓存、无法在 reorg 后确定性重建的字段，都不得作为共识输入。

## 13. 版本治理

系统至少应维护以下版本：

- `protocol_version`：矿工证协议版本。
- `formula_version`：经济公式版本。
- `query_semantics_version`：历史查询与投影语义版本。

### 13.1 升级原则

凡是影响以下结果的变更，都必须视为 `formula_version` 升级：

- `energy`。
- `effective_energy`。
- `level`。
- `real_difficulty`。
- `CoinBase`。
- 奖励分配。
- `price` / `real_price` 收敛行为。

### 13.2 激活方式

版本切换必须通过明确的激活高度或治理决议进行，不得以“代码发布即生效”的隐式方式触发。`<TODO>`

# 设计理由

## 1. 采用抽象单位而不是直接暴露实现常量

`review.md` 使用 `balance_units` 描述经济直觉，当前实现使用 sats 和大整数乘子。本文通过：

- `1 balance_unit = 100_000 sats`
- `ENERGY_SCALE = 1_000_000_000`

把两者衔接起来。

这样既能保留“1 BTC = 1000 单位”的直观模型，又能解释为什么当前代码中会出现 `10_000` 的增长乘子。

## 2. 对惩罚模型显式分离“目标公式”与“当前兼容近似”

当前实现的惩罚常量已经可运行，但它没有显式包含 `H_now`。如果直接把它写成“最终经济规则”，会让读文档的人误以为系统已经严格实现了 `lost_units * H_now * lambda`。

因此，本文把两者分开：

- 目标规则：比例年龄惩罚。
- 当前兼容：固定窗口近似。

## 3. 对 `prev` 采用严格失败，而非部分成功

宽松的 `warn + skip` 适合研发早期容错，但不适合共识规则。严格失败可以避免：

- 部分继承造成的不可预期价值分配。
- 不同实现对异常输入做出不同处理。
- 未来回放时出现历史歧义。

## 4. 将 Burned 同步写入 energy 状态机

只在 pass snapshot 中写 `Burned`，而不在 energy 状态机中封口，会导致历史查询与状态机分叉。本文要求 burn 必须在 energy 侧也写终态记录，从而统一“可查询状态”和“可推导状态”。


# 参考实现状态

下表描述当前仓库与目标模型之间的对应关系。

| 模块 | 目标规则 | 当前实现状态 | 备注 |
| --- | --- | --- | --- |
| pass 状态机 | `Active/Dormant/Consumed/Burned/Invalid` | 已部分实现 | `Burned` 与 energy 尚未完全对齐 |
| 单地址单活跃 | 必须 | 已实现 | 可作为现有行为基线 |
| 同 owner 搬移 | 不休眠、不惩罚 | 已实现 | satpoint 更新 |
| `prev` 继承 | 严格校验 + 单次消费 | 已部分实现 | 当前偏宽松 |
| energy 增长 | 抽象单位模型 | 已实现兼容映射 | raw scale 可对齐 |
| energy 惩罚 | `lost_units * H_now * lambda` | 已近似实现 | 当前为固定窗口近似 |
| 继承折损 | 默认 5% | 未实现 | 当前为全额继承 |
| Leader/collab | 必须支持 | 未完整实现 | 仅看到字段保留 |
| `effective_energy` | 必须支持 | 未实现 | 需补模块 |
| `level/difficulty` | 必须支持 | 未实现 | 需补模块 |
| CoinBase/分账 | 必须支持 | 未实现 | 需补模块 |
| `price/real_price` | 必须支持 | 未实现 | 需补模块 |
| reorg 回放 | 必须支持 | 已有基础 | 需覆盖更多经济字段 |

# 安全性考虑

## 1. `prev` 重放与双花继承

如果 `prev` 允许宽松跳过或重复消费，会导致能量继承语义不确定，甚至出现价值重复计算。因此，`prev` 必须唯一、存在、未消费且状态正确。

## 2. Burned 状态分叉

如果 burn 不同步写入 energy 终态，攻击者可能利用查询层与状态层的分叉结果获取错误排名、错误难度折减或错误收益。

## 3. 协作矿工的双重计数

如果不把 collab pass 的“基础 energy”与“独立挖矿有效能量”分开，可能出现：

- collab 自己参与挖矿；同时
- 其 energy 又计入 Leader。

本文要求协作矿工证在独立挖矿口径下 `effective_energy = 0`，以避免双计。

## 4. 价格操纵

`real_price` 如果缺乏足够深度的挂单约束，容易被浅流动性操纵。因此更新 `real_price` 必须附带双边挂单约束，并且该证明必须进入验证路径。

## 5. 难度下穿

`real_difficulty = difficulty * (1 - level * 0.01)` 如果不设边界，可能导致难度降为零或负值。实现必须在公式版本中显式约束这一点。

# 待定问题

1. `leader_ref` 的协议字段如何表达。`<TODO>`
2. `eth_collab` 在目标协议中的最终语义。`<TODO>`
3. `prev` 的所有权一致性应如何精确定义。`<TODO>`
4. 协作矿工退出普通矿工时的附加损失参数。`<TODO>`
5. “带报价的 B 出块” 的共识字段与验证方式。`<TODO>`
6. `effective_energy`、`level` 与 `real_difficulty` 的 rounding 规则。`<TODO>`
7. `MAX_LEVEL` 或 `MIN_DIFFICULTY_FACTOR` 的选择。`<TODO>`
8. `TOTAL_MINER_BTC` 的统计口径。`<TODO>`
9. `K = f(CE, AE)` 的最终函数。`<TODO>`
10. 叔块奖励规则的精确定义。`<TODO>`
11. `price` / `real_price` 的初始值与激活时机。`<TODO>`
12. 辅助算力池证明格式、分配规则与反作弊机制。`<TODO>`
13. 版本激活方式与治理流程。`<TODO>`

# 参考资料

- `review.md`
- `usdb 现有经济模型.md`
- `src/btc/usdb-indexer/src/index/pass.rs`
- `src/btc/usdb-indexer/src/index/energy.rs`
- `src/btc/usdb-indexer/src/index/energy_formula.rs`
- `doc/矿工证铭文协议.md`
- `doc/矿工证铭文设计.md`
- `doc/usdb-indexer/usdb-indexer-rpc-v1.md`
- `doc/btc-consensus-rpc-error-contract-design.md`
- `doc/usdb-indexer/usdb-indexer-validator-block-body-e2e-design.md`
