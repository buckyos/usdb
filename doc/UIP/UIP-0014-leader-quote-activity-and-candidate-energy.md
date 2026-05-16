UIP: UIP-0014
Title: Leader Quote Activity and Candidate Energy Policy
Status: Draft
Type: Standards Track
Layer: ETHW Validator / Economic Policy
Created: 2026-05-08
Requires: UIP-0000, UIP-0004, UIP-0005, UIP-0006, UIP-0007, UIP-0008, UIP-0013
Activation: ETHW network activation matrix; first official networks enable v1 quote activity policy from genesis

# 摘要

本文定义 Leader 主动报价活跃性如何影响 ETHW 出块候选能量。

核心规则：

- UIP-0004 的 `raw_energy`、`collab_contribution`、`effective_energy` 仍由 USDB indexer 按 BTC 历史状态派生。
- ETHW 侧单独维护 Leader quote activity state。
- Leader 超过一周没有有效主动报价时，仍可作为普通 standard pass 出块，但不得使用协作者能量。
- v1 使用 `FixedPrice` 时，主动报价是 activity heartbeat，不改变 UIP-0013 的 fixed price。
- candidate energy 和 candidate level 必须使用 parent ETHW state 中已经生效的 quote activity。
- block `N` 内的有效 quote 最早影响 block `N+1` 的 candidate energy。

# 动机

经济模型设计要求 Leader 持续参与报价。如果一个 Leader 长期不主动报价，却仍能长期使用协作者能量降低出块难度，会削弱协作机制对 Leader 行为的约束。

同时，USDB indexer 不应反向依赖 ETHW 出块历史。UIP-0004 已明确：

```text
raw_energy(pass, h)
collab_contribution(pass, h)
effective_energy(pass, h)
```

这些值只依赖 BTC 侧历史状态。Leader 是否最近主动报价，属于 ETHW 侧本地可验证规则。

因此，本文把“名义有效能量”和“实际候选能量”分离：

```text
nominal_effective_energy
    = raw_energy + collab_contribution

candidate_energy
    = nominal_effective_energy, if leader_quote_active
    = raw_energy, otherwise
```

# 非目标

本文不定义：

- `raw_energy`、`collab_contribution` 或 `effective_energy` 的 BTC 侧计算公式。
- `level` 阈值表和 `difficulty_factor_bps` 公式。
- `price_atoms_per_btc` 或 `real_price_atoms_per_btc` 更新规则。
- 动态 price source、DeFi 证明或双边挂单机制。
- CoinBase emission、fee split 或辅助算力池。
- USDB indexer 持久化 quote activity。

# 术语

| 术语 | 含义 |
| --- | --- |
| `leader_quote` | Leader 在 ETHW 区块中提交的主动报价 heartbeat。 |
| `leader_quote_active` | Leader 在最近窗口内存在有效 quote 的状态。 |
| `last_valid_quote_block` | ETHW state 中记录的某个 Leader 最近一次有效 quote 所在区块高度。 |
| `self_energy` | standard pass 自身的 `raw_energy`。 |
| `nominal_effective_energy` | UIP-0004 的 `raw_energy + collab_contribution`。 |
| `candidate_energy` | ETHW 出块候选、level 和 difficulty 实际使用的能量。 |
| `self_level` | `level(self_energy)`。 |
| `nominal_leader_level` | `level(nominal_effective_energy)`。 |
| `candidate_level` | `level(candidate_energy)`。 |
| `quote_policy_version` | 本文定义的 quote activity 规则版本。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 常量

v1 固定：

| 参数 | 值 | 状态 | 说明 |
| --- | --- | --- | --- |
| `QUOTE_POLICY_VERSION` | `1` | v1 固定 | Leader quote activity policy 版本。 |
| `LEADER_QUOTE_WINDOW_BLOCKS` | `50400` | v1 固定 | 以 12 秒平均出块间隔计算，目标对应约 1 周。 |
| `QUOTE_SOURCE_KIND` | `FixedPriceHeartbeat` | v1 固定 | 固定价格阶段的 quote 只作为 heartbeat。 |

窗口计算：

```text
7 days * 24 hours * 60 minutes * 60 seconds / 12 seconds = 50400 blocks
```

`LEADER_QUOTE_WINDOW_BLOCKS` 不应使用 wall-clock 动态计算。public network 必须在 activation matrix 或 chain config 中固定精确 block 数量。

# Leader Quote Subject

v1 建议以 active standard pass 的 `pass_id` 作为 quote subject：

```text
leader_quote_subject = resolved_profile.pass_id
```

原因：

- UIP-0007 payload 已显式选择出块 pass。
- pass remint 后，新 pass 必须重新建立 quote activity，避免旧 pass quote 自动继承到新 pass。
- 协作者通过 `leader_btc_addr` 自动跟随新 active pass 时，collab contribution 仍会进入该新 pass 的 `nominal_effective_energy`，但该新 pass 必须先完成有效 quote，才能把 collab contribution 用于 `candidate_energy`。

如果未来希望 quote activity 按 `owner_script_hash` 或 BTC 地址继承，必须升级 quote policy version，并审计 remint、转移和多 active pass 异常路径。

# 能量视图

给定 UIP-0006 返回的 profile：

```text
raw_energy
collab_contribution
effective_energy
```

本文定义：

```text
self_energy(leader, h)
    = raw_energy(leader, h)

nominal_effective_energy(leader, h)
    = raw_energy(leader, h) + collab_contribution(leader, h)
```

对于 collab pass：

```text
candidate_energy(collab, h) = 0
```

collab pass 不直接进入 ETHW validator candidate set。

# Quote Active 规则

验证 ETHW 区块 `N` 时，validator 从 parent state 读取：

```text
last_valid_quote_block(leader)
```

如果不存在记录：

```text
leader_quote_active_N = false
```

如果存在记录：

```text
leader_quote_active_N
    = (N - last_valid_quote_block(leader)) <= LEADER_QUOTE_WINDOW_BLOCKS
```

所有计算必须使用 unsigned integer，并在 underflow / overflow 时 fail closed。

quote active 状态不修改 USDB indexer 里的 `effective_energy`。它只决定 ETHW 侧 candidate energy 如何从 USDB indexer 返回的能量视图中选取。

# Candidate Energy

验证区块 `N` 时：

```text
if leader_quote_active_N:
    candidate_energy_N = nominal_effective_energy
else:
    candidate_energy_N = self_energy
```

这意味着：

- quote active 时，Leader 可以使用协作者能量。
- quote stale 时，Leader 仍可作为普通 standard pass 出块。
- quote stale 时，协作者能量不计入该 Leader 的出块难度折算和候选能量。
- quote stale 不改变 collab pass 的 BTC 侧绑定，也不销毁协作者自身 raw energy。

# Candidate Level

UIP-0005 的 `level` 公式保持不变。本文只定义 ETHW 侧应把哪个 energy 作为输入。

派生视图：

```text
self_level
    = level_from_energy(self_energy)

nominal_leader_level
    = level_from_energy(nominal_effective_energy)

candidate_level
    = level_from_energy(candidate_energy)
```

ETHW difficulty policy 必须使用：

```text
candidate_level
candidate_difficulty_factor_bps
```

而不是无条件使用 `nominal_leader_level`。

USDB indexer 可以继续返回 `raw_energy`、`collab_contribution`、`effective_energy` 和按 `effective_energy` 派生的 `level` 作为审计视图。ETHW validator 必须按本文规则自行计算 `candidate_energy` 和 `candidate_level`。

# FixedPrice V1 Quote

UIP-0013 v1 使用 `FixedPrice`，价格不会被 miner quote 修改。

因此，v1 `leader_quote` 只作为 heartbeat：

```text
leader_quote {
    quote_policy_version = 1
    price_policy_version = active UIP-0013 price policy version
    price_source_kind = FixedPrice
    quoted_price_atoms_per_btc = parent PRICE_ATOMS_PER_BTC_SLOT
}
```

有效 quote 必须满足：

- block 的 `resolved_profile.pass_kind` 是 `standard`。
- block 的 `resolved_profile.pass.state` 是 `active`。
- block reward recipient 校验通过。
- `quote_policy_version == 1`。
- `price_source_kind == FixedPrice`。
- `quoted_price_atoms_per_btc` 等于 parent state 中的 `PRICE_ATOMS_PER_BTC_SLOT`。
- 当前 active `PricePolicyRange` 允许 `FixedPriceHeartbeat` quote。

如果 quote payload 缺失：

```text
last_valid_quote_block 不更新
```

如果 quote payload 存在但无效：

```text
block invalid
```

该规则避免 miner 提交看似报价但不可验证的 payload。

# Parent State 语义

为了避免同一区块内先 quote 恢复 Leader 活跃性，再立即使用协作者能量降低本块难度，本文采用 parent state 语义。

验证区块 `N` 时：

```text
leader_quote_active_N
    = compute from parent state last_valid_quote_block

candidate_energy_N
    = compute from leader_quote_active_N

validate block difficulty using candidate_level_N

if block N contains valid leader_quote:
    write last_valid_quote_block(leader) = N into child state
```

因此，block `N` 的有效 quote 最早影响 block `N+1`。

# Reserved System Storage

Leader quote activity state 必须存放在 ETHW reserved system account storage 中，并由每个区块的 `stateRoot` 承诺。

建议定义：

```text
USDB_SYSTEM_STATE_ADDRESS             = <TODO>

QUOTE_POLICY_VERSION_SLOT             = <TODO>  // uint32 encoded as uint256
LEADER_QUOTE_WINDOW_BLOCKS_SLOT       = <TODO>  // uint64 encoded as uint256
LEADER_LAST_VALID_QUOTE_BLOCK_MAP     = <TODO>  // mapping leader_quote_subject -> uint64
```

`LEADER_LAST_VALID_QUOTE_BLOCK_MAP` 的 key canonical encoding 由实现规范固定。v1 建议：

```text
key = hash("usdb.leader_quote", quote_policy_version, pass_id)
```

在 canonical encoding Final 前，本文保留 `<TODO>`。

普通 EVM 交易、用户合约和 SourceDAO / Dividend 合约不得直接修改 quote activity storage。

# Genesis 初始化

v1 genesis 默认不预置任何 Leader 的 `last_valid_quote_block`：

```text
LEADER_LAST_VALID_QUOTE_BLOCK_MAP = empty
```

因此，网络启动初期所有 Leader 默认只能按 `self_energy` 出块。某个 Leader 在出块中提交有效 quote 后，其协作者能量最早从下一块开始进入 `candidate_energy`。

如果 public network 希望为特定 genesis Leader 预置 quote activity，必须在 genesis config / activation registry 中显式列出，并进入 `USDBGenesisHash` 或等价 bootstrap commitment。v1 草案不建议默认预置。

# State Transition

验证区块 `N` 时：

```text
parent_quote_state = read quote activity storage from parent state
resolved_profile = resolve UIP-0007 selector through UIP-0006

self_energy = resolved_profile.raw_energy
nominal_effective_energy = resolved_profile.raw_energy
                           + resolved_profile.collab_contribution

leader_quote_active = compute from parent_quote_state

candidate_energy = leader_quote_active
    ? nominal_effective_energy
    : self_energy

candidate_level = UIP-0005.level(candidate_energy)
candidate_difficulty_factor_bps
    = UIP-0005.difficulty_factor_bps(candidate_level)

validate block difficulty / reward policy using candidate values

if block contains valid leader_quote:
    write last_valid_quote_block(leader) = N into child state
```

如果 validator 计算出的 `candidate_level` 与 block / payload 中任何声明值不一致，必须以本地重算值为准；若协议要求携带声明值，则不一致时区块无效。

# 历史重放

历史区块重放必须只依赖：

- UIP-0007 profile selector。
- UIP-0006 在对应历史 context 下返回的 USDB economic profile。
- parent ETHW state 中的 quote activity storage。
- 当前区块中携带的 quote payload。
- 当时 active 的 quote policy version / activation matrix。

禁止通过 RPC 查询当前 head 的 Leader 报价状态来验证历史区块。

ETHW reorg 时：

- `last_valid_quote_block` 必须随 ETHW state 回滚。
- candidate energy 和 candidate level 必须按回滚后的 parent state 重算。
- quote payload 本身随区块历史重放。

# 与 UIP-0004 的关系

UIP-0004 定义：

```text
effective_energy = raw_energy + collab_contribution
```

本文不修改该定义。

本文只规定 ETHW 出块时何时允许使用 `collab_contribution`：

```text
candidate_energy = effective_energy, if leader_quote_active
candidate_energy = raw_energy, otherwise
```

因此，Leader quote stale 不会改变 BTC 侧 pass 状态，也不会改变 USDB indexer 的 energy ledger。

# 与 UIP-0005 的关系

UIP-0005 的 level 阈值表和 difficulty 折算公式保持不变。

本文新增：

```text
candidate_level = level(candidate_energy)
```

ETHW 出块难度必须使用 `candidate_level`，而不是直接使用 USDB indexer 返回的 nominal effective level。

# 与 UIP-0013 的关系

UIP-0013 v1 的 fixed price 不会被 quote 修改。

本文 v1 quote 必须引用 parent state 中的 fixed price，只作为 Leader activity heartbeat。

未来 dynamic price source UIP 可以把 leader quote 与真实价格更新合并，但必须显式定义：

- price update 是否也是 activity quote。
- price update 和 activity quote 的失败语义是否一致。
- price update 是否仍采用 parent state / one-block delay。

# 测试要求

至少需要覆盖：

- 无 `last_valid_quote_block` 时，candidate energy 只使用 `raw_energy`。
- quote active 时，candidate energy 使用 `raw_energy + collab_contribution`。
- quote stale 后，candidate energy 回落到 `raw_energy`。
- `LEADER_QUOTE_WINDOW_BLOCKS = 50400` 边界高度。
- block `N` 的有效 quote 只影响 block `N+1`。
- FixedPrice v1 中 quoted price 必须等于 parent `PRICE_ATOMS_PER_BTC_SLOT`。
- quote payload 缺失时不更新 activity state。
- quote payload 存在但无效时区块无效。
- pass remint 后，新 pass 不继承旧 pass 的 quote activity。
- reorg 后 `last_valid_quote_block` 回滚，candidate level 重算。

# 待审计问题

| 问题 | 当前草案结论 | 后续动作 |
| --- | --- | --- |
| quote window 长度 | v1 使用 `50400` ETHW blocks，约 1 周。 | 与 public network block time 一起复核。 |
| quote subject | v1 建议使用 active standard pass `pass_id`。 | 审计是否需要按 BTC owner / address 继承。 |
| quote payload 编码位置 | 必须进入共识可见数据。 | 在 UIP-0007 payload 扩展或系统交易中固定 canonical encoding。 |
| quote 是否每块必填 | 不强制；缺失则不更新 activity。 | 评估是否对 public network 强制每个 standard block 携带 quote。 |
| stale 后是否完全失去 Leader 资格 | 当前草案为仅失去 collab energy，仍可按 self energy 出块。 | 委员会确认是否需要更严厉处罚。 |
| quote update 激励 | v1 没有额外奖励。 | 如果活跃性不足，考虑后续奖励或义务机制。 |
| 与 dynamic price update 的合并 | 本文只定义 FixedPrice heartbeat。 | 后续 dynamic price source UIP 决定是否合并。 |
