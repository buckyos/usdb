UIP: UIP-0011
Title: CoinBase Emission and Reward Split
Status: Draft
Type: Standards Track
Layer: ETHW Reward / Economic Policy
Created: 2026-04-28
Requires: UIP-0000, UIP-0006, UIP-0007, UIP-0008, UIP-0009, UIP-0010
Activation: ETHW network activation matrix; first official networks define reward policy versions before public launch

# 摘要

本文定义 USDB ETHW 区块的 CoinBase 释放公式、手续费分账和奖励接收方校验边界。

UIP-0011 解决的问题是：

- ETHW validator 如何根据 UIP-0007 `ProfileSelectorPayload` 和 UIP-0006 `resolved_profile` 重算区块奖励。
- CoinBase emission 如何绑定矿工 BTC 资产总量、`price` 和已发行 USDB。
- 交易手续费如何在矿工和 SourceDAO / Dividend 分红池之间分配。
- 辅助算力池、协作效率系数 `K`、叔块奖励和收益合约在 v1 中如何留出版本边界。

本文只定义 reward / fee split 的共识公式和状态转换边界，不定义 `price` / `real_price` 更新证明、辅助算力证明格式、SourceDAO / Dividend 冷启动或合约内部二次分润。

# 动机

USDB 的发行目标是让 ETHW 区块奖励与 BTC 侧矿工资产和价格状态绑定，而不是继续使用固定静态 block reward。

同时，奖励计算必须可历史重放：

- miner 出块时可以查询当前可用的 USDB / BTC 历史状态。
- validator 在未来重放旧块时，必须能通过区块头中的 selector 回到同一份历史状态。
- reward 结果必须由 ETHW state transition 本地重算，不能信任 miner 提交的金额。

因此，UIP-0011 只消费 UIP-0007 的最小链上 selector 和 UIP-0006 的历史经济状态视图，不在区块头中携带 reward amount、collab list 或价格证明明细。

# 非目标

本文不定义：

- 协作效率系数 `K` 的 rolling window、状态存储和计算公式，见 UIP-0012。
- `price` / `real_price` 的更新规则、挂单证明或收敛公式，见 UIP-0013。
- 辅助算力池提交格式、BTC 难度证明或多提交者分配规则，见 UIP-0015。
- SourceDAO / Dividend 的 system contract 地址、genesis predeploy 和 bootstrap 流程，见 UIP-0010。
- pass 铭文 schema、state machine、energy、level 或 real difficulty 公式。
- SourceDAO / Dividend 合约内部如何把收到的资金继续分配给 DAO 成员。
- 协作者直接 claim、collab breakdown 分润或 Leader 收益合约内部逻辑。

# 术语

| 术语 | 含义 |
| --- | --- |
| `CoinBase` | ETHW 区块新发行的 USDB native token 数量。本文用 `coinbase_emission_atoms` 表示最小单位。 |
| `USDB atom` | USDB native token 最小单位。若执行层继承 EVM `wei` 语义，`1 USDB = 10^18 atoms`。 |
| `tx_fees_atoms` | 一个 ETHW 区块中可分配的交易手续费总额，单位为 USDB atoms。 |
| `reward_recipient` | 当前区块 miner reward 的接收地址，v1 来自 standard pass 的 `eth_main`。 |
| `dao_fee_recipient` | DAO / Dividend 分红池手续费接收地址，来自 UIP-0010 `DividendAddress`。 |
| `total_miner_btc_sats` | 参与发行目标计算的矿工 BTC 资产总量，单位为 sat。 |
| `issued_usdb_atoms` | 当前区块执行前已经发行的 USDB native token 总量，单位为 atoms。 |
| `issued_usdb_state_slot` | ETHW reserved system account storage 中记录 `issued_usdb_atoms` 的协议状态 slot。 |
| `price_atoms_per_btc` | 1 BTC 对应的 USDB atoms 数量，由 UIP-0013 price state 提供。 |
| `k_bps` | UIP-0012 定义的协作效率系数 `K`，basis points 表示，`10000` 表示 `K = 1.0`。 |
| `reward_rule_version` | ETHW reward 公式和奖励接收方校验版本。 |
| `coinbase_emission_policy_version` | CoinBase emission 公式版本。 |
| `fee_split_policy_version` | 交易手续费分账公式版本。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 版本

首版建议版本：

```text
reward_rule_version = 1
coinbase_emission_policy_version = 1
fee_split_policy_version = 1
```

版本边界：

- `reward_rule_version` 描述 reward 输入校验、奖励接收方校验和最终 reward state transition。
- `coinbase_emission_policy_version` 描述 CoinBase emission 公式。
- `fee_split_policy_version` 描述 `tx_fees_atoms` 如何拆分给 miner 和 DAO / Dividend。
- `payload_version` 仍由 UIP-0007 定义，不因 reward 公式参数变化而自动升级。
- `difficulty_policy_version` 仍由 UIP-0005 / UIP-0007 / UIP-0009 定义，不属于本文。

如果未来只修改手续费比例或 aux pool 分配比例，但 UIP-0007 payload 字节布局不变，应升级上述 policy version，而不是强制升级 `payload_version`。`K` 函数变更由 UIP-0012 的 policy version 管理。

# 常量

首版草案使用以下参数名：

| 参数 | 值 | 状态 | 说明 |
| --- | --- | --- | --- |
| `USDB_ATOMS_PER_USDB` | `1_000_000_000_000_000_000` | 建议固定 | EVM native token 最小单位。 |
| `BTC_SATS_PER_BTC` | `100_000_000` | 固定 | BTC sat 换算。 |
| `EMISSION_BLOCKS` | `157_680` | 来自设计大纲 | 释放平滑窗口。 |
| `MINER_FEE_BPS` | `6000` | 来自设计大纲 | fee split 激活后矿工手续费份额。 |
| `DAO_FEE_BPS` | `4000` | 来自设计大纲 | fee split 激活后 DAO / Dividend 手续费份额。 |
| `AUX_POOL_COINBASE_BPS` | `2500` | 等 UIP-0015 激活 | 辅助算力池启用后的 CoinBase 份额。 |
| `K_BPS_BASE` | `10000` | 建议 v1 fallback | `K = 1.0`。 |
| `K_BPS_MIN_EXCLUSIVE` | `8000` | 来自设计大纲 | `K > 0.8`。 |
| `K_BPS_MAX` | `20000` | 来自设计大纲 | `K <= 2.0`。 |

所有金额计算必须使用无符号整数。对外 JSON / RPC 中超出 JavaScript safe integer 的金额必须使用 canonical decimal string。

# Reward 输入

ETHW validator 计算区块 `B` 的 reward 时，必须取得以下输入：

| 输入 | 来源 | 说明 |
| --- | --- | --- |
| `ProfileSelectorPayload` | UIP-0007 `header.Extra` | 指向历史 USDB state 和 miner pass。 |
| `resolved_profile` | UIP-0006 state view | 按 payload selector 查询得到的 pass economic profile。 |
| `header.Coinbase` | ETHW block header | 必须等于 reward recipient。 |
| `tx_fees_atoms` | ETHW execution | 当前区块可分配交易手续费。 |
| `DividendAddress` | UIP-0010 / ETHW chain config | DAO / Dividend 手续费接收方。 |
| `DividendFeeSplitBlock` | UIP-0010 / ETHW chain config | fee split 生效高度。 |
| `price_atoms_per_btc` | UIP-0013 price state | 当前区块使用的 BTC 价格。 |
| `total_miner_btc_sats` | USDB / BTC historical state | 当前发行目标的 BTC 资产总量。 |
| `issued_usdb_atoms` | ETHW parent state reserved storage | 当前区块执行前已发行 USDB native token 总量。 |
| `k_bps` | UIP-0012 K policy | 协作效率系数。 |
| `aux_pool_state` | UIP-0015 | 辅助算力池是否启用及接收方。 |

## Reward Recipient

v1 的 `reward_recipient` 来自 standard pass 的 `eth_main`。

验证规则：

- `resolved_profile.pass.state` 必须是 `active`。
- `resolved_profile.pass.pass_kind` 必须是 `standard`。
- validator 必须能从该 pass 的铭文 schema 或 UIP-0006 扩展字段解析出 `eth_main`。
- `header.Coinbase` 必须等于该 `eth_main`。
- 如果 `header.Coinbase` 与 `eth_main` 不一致，区块必须无效。

原因：

- UIP-0007 payload 显式携带 `pass_id`，避免通过 `coinbase` 隐式反查 pass。
- 但 reward 发放仍必须防止 miner 引用他人的 `pass_id`，再把收益发给自己的 `coinbase`。

如果未来支持单独的 miner income contract 或收益合约地址，必须通过后续 UIP 增加明确字段和校验规则。v1 不从 `header.Coinbase` 之外推导额外收益合约。

# `total_miner_btc_sats`

`total_miner_btc_sats` 是 CoinBase 目标供应量的 BTC 侧输入。

首版建议口径：

```text
active_miner_owner_set(h)
    = unique owner_script_hash of all active valid miner passes at BTC height h

total_miner_btc_sats(h)
    = sum(balance_sats(owner_script_hash, h)
          for owner_script_hash in active_miner_owner_set(h))
```

说明：

- active valid miner pass 包含 `standard` 和 `collab` pass。
- `Consumed`、`Dormant`、`Burned`、`Invalid` pass 不进入集合。
- 同一个 `owner_script_hash` 即使因为实现缺陷或历史兼容产生多个 active pass，也只能计入一次，避免 BTC 余额重复计数。
- `balance_sats` 必须来自与 payload selector 绑定的 BTC / USDB 历史状态，不得查询 current head。

是否只统计 standard pass，还是统计 standard + collab pass，是本文最重要的待审计问题之一。当前草案倾向统计 standard + collab，因为 collab pass 仍代表 BTC owner 锁定在矿工经济系统中的资产和能量贡献。

# `issued_usdb_atoms`

`issued_usdb_atoms` 是 CoinBase 公式的累计已发行供给输入。

首版口径：

```text
issued_usdb_atoms_before_block
    = genesis_alloc_supply_atoms
      + cumulative_prior_coinbase_emission_atoms
```

规则：

- contract-held balance 仍然属于已发行供应。
- SourceDAO / Dividend / bootstrapAdmin 的 genesis 余额如果进入 genesis alloc，应计入 `genesis_alloc_supply_atoms`。
- token burn 不从 `issued_usdb_atoms` 中扣除。
- 普通账户之间转账不改变 `issued_usdb_atoms`。
- `issued_usdb_atoms` 不表示当前可流通量，也不尝试追踪用户、合约或官方合约的销毁行为。

## Reserved System Storage

`issued_usdb_atoms` 必须存放在 ETHW reserved system account storage 中，并由每个区块的 `stateRoot` 承诺。

建议定义：

```text
USDB_SYSTEM_STATE_ADDRESS = <TODO>
ISSUED_USDB_ATOMS_SLOT   = <TODO>
ISSUED_USDB_ATOMS_TYPE   = uint256
```

该 storage slot 是 ETHW reward state transition 的协议状态：

- genesis 必须初始化 `ISSUED_USDB_ATOMS_SLOT = genesis_alloc_supply_atoms`。
- 验证区块 `N` 时，validator 从 parent state 读取 `issued_usdb_atoms_before_block_N`。
- validator 按本文公式计算 `coinbase_emission_atoms_N`。
- state transition 将 `ISSUED_USDB_ATOMS_SLOT` 写为：
  ```text
  issued_usdb_atoms_after_block_N
      = issued_usdb_atoms_before_block_N + coinbase_emission_atoms_N
  ```
- 写入后的 storage root 必须进入区块 `stateRoot`。
- 普通 EVM 交易、用户合约和 SourceDAO / Dividend 合约不得直接修改该 slot。

该设计不是本地隐藏数据库。新节点同步区块时通过执行 state transition 重建该 storage；reorg 时该 storage 随 ETHW state 一起回滚；archive / snapshot 节点可按普通历史 state 查询该值。

本文不要求把 `issued_usdb_atoms` 放入 block header 或 UIP-0007 `header.Extra`。如未来为了轻客户端或审计直观性需要显式 header commitment，应通过后续 payload / header UIP 单独定义。

# CoinBase Emission

CoinBase 目标公式来自经济模型设计大纲：

```text
CoinBase
    = K * (TOTAL_MINER_BTC * price - ISSUED_USDB) / EMISSION_BLOCKS
```

本文将其转换为整数公式：

```text
target_supply_atoms
    = floor(total_miner_btc_sats * price_atoms_per_btc / BTC_SATS_PER_BTC)

remaining_target_atoms
    = max(0, target_supply_atoms - issued_usdb_atoms_before_block)

coinbase_emission_atoms
    = min(
          remaining_target_atoms,
          floor(remaining_target_atoms * k_bps
                / (EMISSION_BLOCKS * 10000))
      )
```

归零条件：

- `total_miner_btc_sats == 0` 时，`coinbase_emission_atoms = 0`。
- `price_atoms_per_btc == 0` 时，区块必须无效或 price policy 必须 fail closed；不应默默按 0 发行。
- `target_supply_atoms <= issued_usdb_atoms_before_block` 时，`coinbase_emission_atoms = 0`。
- `resolved_profile` 不满足 reward recipient 校验时，区块无效，而不是发行 0。

`price_atoms_per_btc` 的来源、初始值和更新时机由 UIP-0013 定义。在 UIP-0013 Final 前，本文不能进入 Final。

# 协作效率系数 K

协作效率系数 `K` 由 UIP-0012 定义。本文只消费 UIP-0012 输出的 `k_bps`。

v1 要求：

- `k_bps` 必须由 ETHW validator 在执行区块时本地重算。
- `k_bps` 必须由 reserved system storage 中的 UIP-0012 rolling window 状态和当前区块的 UIP-0006 `collab_contribution` 决定。
- `k_bps` 不得由 miner 在区块头或交易中直接声明。
- UIP-0012 warmup 阶段输出 `k_bps = 10000`。

UIP-0011 不重新定义 `CE`、`AE`、rolling window、warmup 或 `compute_k_bps`。

# CoinBase Split

## 辅助算力池未启用

在 UIP-0015 未激活或 aux pool 未启用时：

```text
miner_coinbase_atoms = coinbase_emission_atoms
aux_pool_coinbase_atoms = 0
```

## 辅助算力池启用

在 UIP-0015 激活且 aux pool 对当前区块有效时，使用设计大纲中的 75% / 25% 目标：

```text
aux_pool_coinbase_atoms
    = floor(coinbase_emission_atoms * AUX_POOL_COINBASE_BPS / 10000)

miner_coinbase_atoms
    = coinbase_emission_atoms - aux_pool_coinbase_atoms
```

按该规则，整除余数归矿工，确保：

```text
miner_coinbase_atoms + aux_pool_coinbase_atoms == coinbase_emission_atoms
```

在 UIP-0015 Final 前，public network 不应启用 aux pool split。

# Fee Split

fee split 只在 UIP-0010 `DividendFeeSplitBlock` 已到达，且 `DividendAddress != 0x0` 时启用。

未启用时：

```text
miner_fee_atoms = tx_fees_atoms
dao_fee_atoms = 0
```

启用后：

```text
dao_fee_atoms
    = floor(tx_fees_atoms * DAO_FEE_BPS / 10000)

miner_fee_atoms
    = tx_fees_atoms - dao_fee_atoms
```

整除余数归矿工，确保：

```text
miner_fee_atoms + dao_fee_atoms == tx_fees_atoms
```

`dao_fee_atoms` 必须进入 UIP-0010 `DividendAddress`。如果 `DividendFeeSplitBlock` 已到达但 `DividendAddress` 无 code 或 bootstrap state 未完成，validator 必须 fail closed。

# Final Reward Outputs

正常区块的 reward output：

```text
miner_reward_atoms
    = miner_coinbase_atoms + miner_fee_atoms

dao_reward_atoms
    = dao_fee_atoms

aux_pool_reward_atoms
    = aux_pool_coinbase_atoms
```

state transition 必须满足：

```text
minted_atoms == coinbase_emission_atoms
distributed_fee_atoms == tx_fees_atoms
```

其中：

- `miner_reward_atoms` 发给 `header.Coinbase`，且 `header.Coinbase == resolved_profile.eth_main`。
- `dao_reward_atoms` 发给 `DividendAddress`。
- `aux_pool_reward_atoms` 发给 UIP-0015 定义的 aux pool recipient。

# Uncle / Ommer Reward

设计大纲要求“矿工的实际 CoinBase 收入在上述数值上应用 ETH 的叔块奖励规则”。

当前草案不直接固定 uncle / ommer reward，原因是仍需明确：

- USDB ETHW v1 是否保留 uncle / ommer 机制。
- 采用哪一版 Ethereum / ETHW uncle reward 公式。
- uncle 区块是否也必须携带 UIP-0007 profile selector。
- uncle reward 是否影响 `issued_usdb_atoms` system storage。
- uncle 与 BTC 历史 state selector 的绑定方式。

建议 v1 在未完成该审计前禁用 USDB-specific uncle reward，或将 uncle reward 固定为 `0`。如果 public network 需要 uncle reward，必须在本文 Final 前补充完整规则。

# Reorg 语义

ETHW reorg 时：

- `issued_usdb_atoms` reserved system storage 必须随 ETHW state 回滚。
- fee split、DAO reward、aux pool reward 必须随 ETHW state 回滚。
- 区块引用的 UIP-0007 payload 不变，validator 重放时必须按该 payload 重新查询对应历史 USDB state。

BTC / USDB 侧 reorg 已由 UIP-0006 / UIP-0008 的历史 state selector 和 activation matrix 处理。ETHW validator 不得用 USDB current head 替换旧块 payload 中的历史 state。

# 实现影响

go-ethereum:

- `/home/bucky/work/go-ethereum/core/state_transition.go`
- `/home/bucky/work/go-ethereum/consensus/ethash/consensus.go`
- `/home/bucky/work/go-ethereum/params/config.go`
- `/home/bucky/work/go-ethereum/docs/usdb/usdb-ethw-reward-integration.md`
- `/home/bucky/work/go-ethereum/docs/usdb/usdb-ethw-fee-split-integration.md`

USDB indexer / state view:

- `doc/UIP/UIP-0006-usdb-economic-state-view.md`
- `src/btc/usdb-indexer/src/service/client.rs`
- `src/btc/usdb-indexer/src/index/*`

需要确认 UIP-0006 是否要在 `pass_economic_profile` 中显式返回：

- `eth_main` / `reward_recipient`。
- `total_miner_btc_sats` 或可审计的 aggregate view。
- `price_state_id` 或与 UIP-0013 绑定的 price state reference。

# 测试要求

至少需要覆盖：

- reward payload selector 指向 active standard pass，`header.Coinbase == eth_main` 时区块有效。
- `header.Coinbase != eth_main` 时区块无效。
- collab pass 不能直接作为 reward pass。
- missing / stale / current-head USDB state query 必须 fail closed。
- `target_supply_atoms <= issued_usdb_atoms` 时 CoinBase 为 0。
- `total_miner_btc_sats = 0` 时 CoinBase 为 0。
- genesis 初始化 `ISSUED_USDB_ATOMS_SLOT = genesis_alloc_supply_atoms`。
- 每个区块执行后 `ISSUED_USDB_ATOMS_SLOT` 增加当前区块 `coinbase_emission_atoms`。
- 用户、合约或官方合约 burn 不改变 `ISSUED_USDB_ATOMS_SLOT`。
- fee split 未激活时所有 fee 归 miner。
- fee split 激活后 60% / 40% 分账，rounding remainder 归 miner。
- aux pool 未激活时 CoinBase 100% 归 miner。
- aux pool 激活后 75% / 25% 分账，rounding remainder 归 miner。
- reorg 后 `issued_usdb_atoms` system storage 和 fee split state 正确回滚。
- joiner 重放旧块时不能使用当前 USDB state 重新计算 reward。

# 待审计问题

| 问题 | 当前草案结论 | 后续动作 |
| --- | --- | --- |
| `total_miner_btc_sats` 是否统计 standard + collab pass | 当前倾向统计所有 active valid miner pass 的 unique owner balance。 | 需要审计是否会放大协作资产或产生重复计数。 |
| `issued_usdb_atoms` 是否包含 genesis alloc | 包含 genesis alloc 和 prior CoinBase，不扣除 burn。 | 固定 reserved system account address / storage slot，并在 genesis 中初始化。 |
| `price_atoms_per_btc` 来源和 scale | 由 UIP-0013 定义；本文只消费 price state。 | UIP-0013 必须固定初始值、更新顺序和 decimal encoding。 |
| 动态 `K` 是否进入首个 public network | 已拆分到 UIP-0012；v1 使用 `collab_contribution` 作为 `CE_N`。 | Review UIP-0012 rolling window、warmup 和整数 `compute_k_bps`。 |
| aux pool split 如何激活 | 初始 `aux_pool_policy_version = 0`，UIP-0015 Final 前不启用。 | UIP-0015 定义证明格式、recipient、verifier code hash 后，通过 activation matrix 在指定高度激活。 |
| uncle / ommer reward | 当前建议禁用或置 0，直到完整规则确定。 | 决定 USDB ETHW v1 是否保留 uncle 机制。 |
| miner income contract | 当前 v1 使用 `eth_main` / `header.Coinbase`。 | 若要独立收益合约，需新增铭文字段或治理配置。 |
| fee accounting 与 EVM fork 语义 | 本文只消费 `tx_fees_atoms`。 | 需要在 go-ethereum 实现中确认 EIP-1559 burn、tips、base fee 的具体路径。 |
| UIP-0006 是否需要新增 reward fields | 当前需要 `eth_main`、aggregate supply inputs 或可审计查询。 | 后续 review UIP-0006 state view 是否扩展。 |
