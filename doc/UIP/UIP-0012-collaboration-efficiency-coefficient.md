UIP: UIP-0012
Title: Collaboration Efficiency Coefficient K
Status: Draft
Type: Standards Track
Layer: ETHW Reward / Economic Policy
Created: 2026-04-29
Requires: UIP-0000, UIP-0004, UIP-0006, UIP-0008, UIP-0009, UIP-0011
Activation: ETHW network activation matrix; first official networks define K policy version before public launch

# 摘要

本文定义 USDB CoinBase 公式中的协作效率系数 `K`。

`K` 用 `k_bps` 表示：

```text
10000 == K 1.0
8000 < k_bps <= 20000
```

核心规则：

- 当前区块的 `CE_N` 使用出块 Leader 在 UIP-0006 历史 context 下的 `collab_contribution`。
- 历史平均值 `AE_N` 使用过去固定数量 ETHW reward blocks 的 `CE` rolling window。
- rolling window 状态存入 ETHW reserved system account storage，并由 `stateRoot` 承诺。
- warmup 阶段不使用动态 `K`，固定 `k_bps = 10000`。
- UIP-0011 只消费本文输出的 `k_bps`，不重新定义 `K` 的窗口或公式。

# 动机

经济模型设计大纲要求 `K` 与当前出块 Leader 的协作能量表现有关：

```text
CE = 当前有效 Leader 出块时，其协作矿工能量总和
AE = 过去 1 周有效 Leader 出块时的协作矿工能量平均值
0.8 < K <= 2.0
```

如果每个区块都扫描过去一周区块来计算 `AE`，验证成本会随窗口长度增长，不适合作为 ETHW 共识路径。

因此，本文把 `K` 设计为一个由 `stateRoot` 承诺的 rolling-window policy。每个区块只更新固定数量的 reserved storage slots，同时保留完整的 reorg 和历史审计语义。

# 非目标

本文不定义：

- CoinBase emission 公式，见 UIP-0011。
- `price` / `real_price` 更新规则，见 UIP-0013。
- 辅助算力池提交和奖励规则，见 UIP-0014。
- collab pass schema、Leader 解析或 `collab_contribution` 计算公式，见 UIP-0001 / UIP-0004 / UIP-0006。
- SourceDAO / Dividend fee split 冷启动，见 UIP-0010。

# 术语

| 术语 | 含义 |
| --- | --- |
| `CE_N` | 区块 `N` 的 current collaboration energy sample。v1 使用当前 Leader 的 `collab_contribution`。 |
| `AE_N` | 区块 `N` 计算 `K` 时使用的 historical average collaboration energy。 |
| `k_bps` | `K` 的 basis points 表示。`10000` 表示 `K = 1.0`。 |
| `K_WINDOW_BLOCKS` | rolling window 长度，使用固定 ETHW block 数量。 |
| `K warmup` | rolling window 未填满时固定 `k_bps = 10000` 的阶段。 |
| `K ring buffer` | reserved system storage 中存放最近 `K_WINDOW_BLOCKS` 个 `CE` sample 的环形窗口。 |
| `collaboration_efficiency_policy_version` | 本文定义的 `K` policy 版本。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 版本

首版建议版本：

```text
collaboration_efficiency_policy_version = 1
```

该版本必须由 ETHW chain config、activation matrix 或 reward policy version 集合确定。

如果未来只修改 `K` 函数、窗口长度、warmup 规则或 `CE` sample 口径，而 UIP-0007 payload 字节布局不变，应升级 `collaboration_efficiency_policy_version` 或对应 reward policy version，不应强制升级 `payload_version`。

# 常量

首版草案使用以下参数名：

| 参数 | 值 | 状态 | 说明 |
| --- | --- | --- | --- |
| `K_BPS_BASE` | `10000` | 固定 | `K = 1.0`。 |
| `K_BPS_MIN` | `8001` | 建议固定 | 整数 bps 下满足 `K > 0.8` 的最小值。 |
| `K_BPS_MAX` | `20000` | 固定 | `K <= 2.0`。 |
| `K_WINDOW_BLOCKS` | `50400` | v1 固定 | 以 12 秒平均出块间隔计算，目标对应约 1 周。 |
| `K_SAMPLE_KIND` | `collab_contribution` | v1 固定 | 当前区块 `CE_N` 的样本口径。 |

`K_WINDOW_BLOCKS` 不应使用 wall-clock 动态计算。v1 按以太坊 12 秒平均出块间隔计算：

```text
7 days * 24 hours * 60 minutes * 60 seconds / 12 seconds = 50400 blocks
```

public network 必须在 activation matrix 或 chain config 中固定精确 block 数量。若未来网络目标出块间隔发生变化，必须通过新的 `collaboration_efficiency_policy_version` 激活新窗口参数，不得在同一 policy version 下动态改变窗口长度。

# CE Sample 口径

v1 固定：

```text
CE_N = resolved_profile.pass.collab_contribution
```

其中 `resolved_profile` 来自区块 `N` 的 UIP-0007 `ProfileSelectorPayload`，并按 UIP-0006 查询。

选择 `collab_contribution` 的原因：

- 它已经是 UIP-0004 定义的 Leader 有效协作贡献口径。
- 它与 Leader `effective_energy` 使用同一套权重和历史解析规则。
- 当前 `COLLAB_WEIGHT_BPS` 是固定线性折算，用于 `CE / AE` 比值时不会改变相对增长趋势。
- 不需要在 UIP-0006 额外引入 `collab_raw_energy_sum` 才能实现 v1。

未来如果希望 `K` 衡量未折算的协作规模，可以新增 `K_SAMPLE_KIND = collab_raw_energy_sum`，但必须先在 UIP-0006 增加可历史重放的 aggregate 字段，并升级 policy version。

# AE Rolling Window

`AE_N` 必须只使用区块 `N` 之前的 canonical reward blocks，不包含当前区块 `N` 的 `CE_N`。

当窗口已填满时：

```text
AE_N = floor(K_WINDOW_SUM_BEFORE_N / K_WINDOW_BLOCKS)
```

当窗口未填满时：

```text
k_bps_N = K_BPS_BASE
AE_N = unavailable
```

不建议使用未填满窗口的部分平均值。原因是冷启动初期样本太少，`AE` 容易被极少数区块操纵，使 `K` 过早触及上限或下限。

如果窗口已填满但 `AE_N == 0`：

```text
k_bps_N = K_BPS_BASE
```

该规则避免在过去一周没有协作样本时，因为当前单个非零 `CE_N` 直接获得最大 boost。

# Reserved System Storage

`K` rolling window 状态必须存放在 ETHW reserved system account storage 中，并由每个区块的 `stateRoot` 承诺。

建议定义：

```text
USDB_SYSTEM_STATE_ADDRESS = <TODO>

K_WINDOW_SUM_SLOT         = <TODO>  // uint256
K_WINDOW_COUNT_SLOT       = <TODO>  // uint64 encoded as uint256
K_WINDOW_CURSOR_SLOT      = <TODO>  // uint64 encoded as uint256
K_CE_RING_SLOT_BASE       = <TODO>  // CE sample ring base slot

K_LAST_CE_SLOT            = <TODO>  // optional audit slot
K_LAST_AE_SLOT            = <TODO>  // optional audit slot
K_LAST_K_BPS_SLOT         = <TODO>  // optional audit slot
```

必填状态：

- `K_WINDOW_SUM_SLOT`
- `K_WINDOW_COUNT_SLOT`
- `K_WINDOW_CURSOR_SLOT`
- `K_CE_RING_SLOT_BASE[i]` for `0 <= i < K_WINDOW_BLOCKS`

可选审计状态：

- `K_LAST_CE_SLOT`
- `K_LAST_AE_SLOT`
- `K_LAST_K_BPS_SLOT`

可选审计 slot 只保存当前 canonical head 对应的 last values。历史高度的 `last` 值仍由 archive / snapshot state 查询。

# State Transition

验证区块 `N` 时，validator 必须按以下顺序处理：

```text
sum_before    = read(K_WINDOW_SUM_SLOT from parent state)
count_before  = read(K_WINDOW_COUNT_SLOT from parent state)
cursor_before = read(K_WINDOW_CURSOR_SLOT from parent state)

CE_N = resolved_profile.pass.collab_contribution

if count_before < K_WINDOW_BLOCKS:
    AE_N = unavailable
    k_bps_N = K_BPS_BASE
else:
    AE_N = floor(sum_before / K_WINDOW_BLOCKS)
    if AE_N == 0:
        k_bps_N = K_BPS_BASE
    else:
        k_bps_N = compute_k_bps(CE_N, AE_N)

UIP-0011 computes CoinBase using k_bps_N

old_sample = count_before == K_WINDOW_BLOCKS
    ? read(K_CE_RING_SLOT_BASE[cursor_before])
    : 0

write(K_CE_RING_SLOT_BASE[cursor_before], CE_N)
write(K_WINDOW_SUM_SLOT, sum_before - old_sample + CE_N)
write(K_WINDOW_COUNT_SLOT, min(count_before + 1, K_WINDOW_BLOCKS))
write(K_WINDOW_CURSOR_SLOT, (cursor_before + 1) % K_WINDOW_BLOCKS)
write(optional last slots)
```

当前区块 `CE_N` 必须在计算 `k_bps_N` 之后再写入窗口。否则当前区块会同时影响分子和分母，削弱 `K` 的激励语义。

# `compute_k_bps`

设计大纲给出目标：

```text
0.8 < K <= 2.0
K 随 CE / AE 单调不减
```

v1 候选整数公式：

```text
if AE == 0:
    k_bps = 10000
elif CE < AE:
    penalty = ceil(60000 * AE / (CE + 5 * AE))
    k_bps = max(8001, 20000 - penalty)
else:
    k_bps = min(20000, floor(10000 * CE / AE))
```

性质：

- `CE == AE` 时，`k_bps = 10000`。
- `CE == 0` 且 `AE > 0` 时，`k_bps = 8001`。
- `CE >= 2 * AE` 时，`k_bps = 20000`。
- 全程只使用整数运算。
- 所有乘法和加法必须使用足够宽的 unsigned integer，并在溢出时 fail closed。

该公式仍需重点审计。本文 Draft 阶段不排除替换为更平滑或更保守的整数函数。

# Reorg 语义

ETHW reorg 时：

- `K` rolling window storage 必须随 ETHW state 回滚。
- `K_LAST_*` 审计 slot 如存在，也必须随 ETHW state 回滚。
- 区块引用的 UIP-0007 payload 不变，validator 重放时必须按该 payload 重新查询对应历史 USDB state，并得到同一 `CE_N`。

USDB / BTC 侧 reorg 由 UIP-0006 / UIP-0008 的历史 selector 和 activation matrix 处理。ETHW validator 不得使用 USDB current head 替换旧块的历史 state。

# 与 UIP-0011 的关系

UIP-0011 的 CoinBase 公式消费本文输出的 `k_bps_N`：

```text
coinbase_emission_atoms
    = min(
          remaining_target_atoms,
          floor(remaining_target_atoms * k_bps_N
                / (EMISSION_BLOCKS * 10000))
      )
```

UIP-0011 不应重新定义 `CE`、`AE`、rolling window、warmup 或 `compute_k_bps`。

# 与 UIP-0006 的关系

UIP-0006 必须能在 UIP-0007 selector 指定的历史 context 下返回：

```text
resolved_profile.pass.collab_contribution
```

v1 不要求 UIP-0006 返回 `collab_raw_energy_sum`。如果后续 policy 改用 raw collab energy，则必须先升级 UIP-0006 state view。

# 实现影响

go-ethereum:

- `/home/bucky/work/go-ethereum/core/state_transition.go`
- `/home/bucky/work/go-ethereum/params/config.go`
- reward verifier / USDB companion integration

USDB indexer:

- `doc/UIP/UIP-0006-usdb-economic-state-view.md`
- `src/btc/usdb-indexer/src/index/*`
- `src/btc/usdb-indexer/src/service/client.rs`

# 测试要求

至少需要覆盖：

- warmup 阶段 `k_bps = 10000`。
- 窗口刚填满的第一个动态 K 区块。
- `AE == 0` 时 `k_bps = 10000`。
- `CE == 0 && AE > 0` 时 `k_bps = 8001`。
- `CE == AE` 时 `k_bps = 10000`。
- `CE >= 2 * AE` 时 `k_bps = 20000`。
- ring buffer 覆盖旧 sample 后 `K_WINDOW_SUM_SLOT` 正确更新。
- reorg 后 rolling window 和 last audit slots 正确回滚。
- 旧块重放时不得使用当前 USDB state 的 `collab_contribution`。

# 待审计问题

| 问题 | 当前草案结论 | 后续动作 |
| --- | --- | --- |
| `CE_N` 使用 raw collab energy 还是 `collab_contribution` | v1 使用 `collab_contribution`。 | 审计 `COLLAB_WEIGHT_BPS` 变化是否需要同步升级 K policy。 |
| `K_WINDOW_BLOCKS` 精确值 | v1 固定为 `50400`，按 12 秒平均出块间隔对应 1 周。 | 若未来调整目标出块间隔，升级 K policy version。 |
| warmup 阶段策略 | 窗口未填满时固定 `k_bps = 10000`。 | 审计冷启动阶段是否需要单独的 activation delay。 |
| `compute_k_bps` 公式 | 使用整数候选公式。 | 做参数表、边界测试和经济攻击审计。 |
| 是否保存 `K_LAST_*` slots | 建议作为审计便利字段，但不是计算必需。 | 实现阶段评估 storage 成本和 RPC 查询需求。 |
