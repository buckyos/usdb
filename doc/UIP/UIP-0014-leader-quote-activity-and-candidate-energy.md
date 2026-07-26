UIP: UIP-0014
Title: Leader Quote Activity and Candidate Energy Policy
Status: Draft
Type: Standards Track
Layer: USDB Validator / Economic Policy
Created: 2026-05-08
Requires: UIP-0000, UIP-0004, UIP-0005, UIP-0006, UIP-0007, UIP-0008, UIP-0013
Activation: Disabled on first public networks with `quote_policy_version = 0`; formal v1 requires a future USDB-chain activation checkpoint

# 摘要

本文定义 USDB chain 如何把 Leader 报价证据转换为出块候选能量、难度折算和
UIP-0012 协作能量输入。

首个 public network 完全禁用报价机制：

```text
quote_policy_version = 0
```

禁用状态下：

- 不携带 quote payload。
- 不解析或认可隐含的矿工报价。
- 不创建 per-Leader quote activity state。
- difficulty 使用 UIP-0006 profile 的名义 `effective_energy` / `difficulty_factor_bps`。
- UIP-0012 `CE_N` 使用名义 `collab_contribution`。

`quote_policy_version = 1` 保留给未来第一个正式、可验证、由 Leader 实际参与的报价
policy。FixedPrice 阶段不要求先激活一个形式化 heartbeat 中间版本；activation matrix
可以在正式 v1 完成后直接从 `0` 升级到 `1`。

实现可以保留 build-tagged `FixedPriceHeartbeat` conformance policy，验证 quote
version 分派、current-block 决策、difficulty、reward、restart、reorg 和历史重放。
该测试 policy 不属于 public protocol，也不占用正式 v1 的版本号或语义。

# 动机

UIP-0004 已定义 BTC 历史状态派生的：

```text
raw_energy
collab_contribution
effective_energy = raw_energy + collab_contribution
```

未来如果 Leader 需要提交自己的挂单、报价或可验证 quote source reference，USDB
chain 必须确定：

- 当前区块是否包含有效报价证据。
- 该证据是否绑定 UIP-0007 选中的 standard pass。
- 当前区块 difficulty 是否允许使用 `collab_contribution`。
- UIP-0012 当前样本 `CE_N` 是否使用同一份协作能量。
- 报价状态如何随 restart、历史重放和 reorg 保持确定性。

FixedPrice v1 已由 UIP-0013 作为共识常量固定。矿工成功产生有效区块本身就表示遵守
固定价格，因此没有必要为了形式完整而在 public network 启用无实际价格输入的
heartbeat policy。

# 非目标

本文不定义：

- UIP-0003 至 UIP-0005 的 energy、level 或 factor 公式。
- UIP-0013 的 fixed/dynamic price 更新公式。
- 第一个正式动态 quote source 的 payload、签名或 proof 格式。
- 外部 Ethereum DeFi proof、USDB native orderbook 或 BTC bridge。
- CoinBase emission、fee split 或 auxiliary hashpower pool。
- USDB indexer 持久化 USDB-chain quote activity。

# 术语

| 术语 | 含义 |
| --- | --- |
| `selected_pass` | UIP-0007 `ProfileSelectorPayload` 选择并经 UIP-0006 验证的 active standard pass。 |
| `nominal_effective_energy` | UIP-0004 `effective_energy`，即 `raw_energy + collab_contribution`。 |
| `quote_evidence` | 当前区块中可供 validator 在 PoW 校验前确定性验证的正式报价证据。 |
| `quote_policy_version` | 把 profile、当前区块输入和 quote evidence 转换为 quote decision 的规则版本。 |
| `candidate_energy` | 当前 USDB 区块 difficulty 实际使用的能量。 |
| `candidate_level` | UIP-0005 `level(candidate_energy)`。 |
| `candidate_difficulty_factor_bps` | UIP-0005 从 `candidate_level` 派生的 difficulty factor。 |
| `collaboration_energy_for_k` | 同一 quote decision 提供给 UIP-0012 当前样本的协作能量。 |
| `current_block_quote_accepted` | 当前区块 quote evidence 已通过 active policy 验证。 |
| `FixedPriceHeartbeat` | 仅用于 build-tagged activation conformance 的隐含固定价格 heartbeat，不是正式 public policy。 |

# 版本模型

| Version | 状态 | 语义 |
| --- | --- | --- |
| `0` | 首版正式启用 | quote 机制完全禁用，使用 nominal effective energy / CE。 |
| `1` | 保留、未实现 | 未来第一个正式 evidence-backed Leader quote policy。 |
| `0xfffe` | 测试专用 | fake v2，无有效 quote，使用 raw energy / `CE=0`。 |
| `0xffff` | 测试专用 | fake v3，当前块隐含 FixedPriceHeartbeat，使用 effective energy / nominal CE。 |

规则：

- 默认 production build 必须拒绝所有未知非零版本，包括尚未实现的正式 v1。
- reserved test ID 只能存在于 build-tagged binary 和测试 genesis。
- public genesis、release manifest 和 production activation matrix 禁止使用 reserved ID。
- public activation 不要求依次经过测试版本或 FixedPriceHeartbeat。
- 正式 v1 必须定义从 `0 -> 1` 的直接 activation、初始化和历史重放规则。

# 禁用策略

`quote_policy_version = 0` 的共识语义固定为：

```text
current_block_quote_accepted = false
candidate_energy = nominal_effective_energy
candidate_level = level(candidate_energy)
candidate_difficulty_factor_bps = difficulty_factor_bps(candidate_level)
collaboration_energy_for_k = collab_contribution
```

该状态中的 `false` 表示 quote 不适用，不表示 Leader quote stale。

规则：

- header、system transaction 和普通 transaction 均不要求 quote evidence。
- `QUOTE_POLICY_VERSION_SLOT` 保持 `0`。
- `LEADER_LAST_VALID_QUOTE_BLOCK_MAP` 不创建记录。
- 不允许使用 CLI 或本地运行时开关模拟 quote active / stale。
- 不能把名义 effective energy 解释为 validator 认可了一条矿工报价。

# Quote Policy Decision

miner 与 validator 必须使用同一个纯派生接口：

```text
QuotePolicyContext {
    resolved_profile
    block_number
    reward_recipient
    active_price_policy_version
    current_block_quote_evidence
}

QuotePolicyDecision {
    policy_version
    candidate_energy
    candidate_level
    candidate_difficulty_factor_bps
    collaboration_energy_for_k
    current_block_quote_accepted
}
```

决策必须在修改 state 前完成。difficulty、UIP-0012 `K` 和 reward transition 必须消费
同一个 decision，禁止各自重新解释 quote 状态。

输入必须满足：

- `raw_energy`、`collab_contribution`、`effective_energy` 都是合法 `uint128`。
- `effective_energy = saturating_u128(raw_energy + collab_contribution)`。
- selected pass 已通过 UIP-0006 active standard candidate 校验。
- reward recipient 与 quote evidence 中声明的主体满足 active policy。
- 未知 policy、损坏 evidence 或不一致 profile 必须 fail closed。

# Header Verification Boundary

只要 quote 会改变当前块 difficulty，validator 就必须在验证 header difficulty 和 PoW
之前获得完整 quote evidence。

允许的 carrier 必须由正式 v1 冻结，例如：

- UIP-0007 payload 的新 canonical 版本。
- header 中对 quote body/system transaction 的 commitment，并配套可用于 header
  validation 的确定性规则。

不能只把影响 difficulty 的 quote 放入普通 transaction body，同时仍要求独立
`VerifyHeader` 在看不到 body 时完成 difficulty 校验。

builder 的顺序必须是：

```text
build selector and quote evidence
resolve QuotePolicyDecision
set candidate difficulty
seal block
```

validator 必须从 block 自身重建同一 decision。

# Future Formal V1

`quote_policy_version = 1` 只保留版本号，不在本 Draft 中冻结具体 source、window、
payload 或授权方式。

正式 v1 激活前必须由 UIP 或本 UIP 后续修订冻结：

- 至少一种具有实际经济含义的 per-Leader quote source。
- quote subject 与 active standard pass 的绑定。
- canonical payload / commitment encoding。
- quote owner、signature 或 source-state authorization。
- 当前块 quote 是否影响当前块或下一块。
- quote 缺失、无效和过期的 candidate energy / CE 语义。
- activation bootstrap、grace period 和 stale Leader 恢复路径。
- per-Leader state 的容量上限、清理和 state-sync 成本。
- reorg、restart、historical replay 和 CheckCompatible 行为。

如果未来 price 仅来自一个全局 oracle 或统一 DeFi 状态，而 miner 不提交自己的报价，
则不应仅为形式完整而激活 UIP-0014 v1。

# Test-only FixedPriceHeartbeat

FixedPriceHeartbeat 仅用于验证未来正式 v1 会复用的共识接线，不作为 public economic
policy。

测试 current-block 语义：

```text
implicit_fixed_price_heartbeat_valid =
    resolved_profile.pass is active standard
    AND header.Coinbase == resolved_profile.reward_recipient
    AND active price_policy_version == UIP-0013 FixedPrice v1
    AND explicit quote evidence is empty
```

该 heartbeat 在 builder 开始 PoW 前由现有 selector、Coinbase 和 active price policy
隐含派生。validator 在校验 difficulty 前重建，因此不需要先挖一个只用于恢复状态的
“报价 block”。

fake v2：

```text
current_block_quote_accepted = false
candidate_energy = raw_energy
collaboration_energy_for_k = 0
```

fake v3：

```text
current_block_quote_accepted = true
candidate_energy = effective_energy
collaboration_energy_for_k = collab_contribution
```

测试 FixedPriceHeartbeat：

- 不编码额外 quote payload。
- 不写 `last_valid_quote_block`。
- 不使用 quote window。
- 不宣称矿工提交了真实市场价格。
- 不冻结未来正式 v1 的 source、authorization 或 state semantics。

它的价值是让 fake v2/fake v3 产生可观察的 difficulty、K 和 reward 差异，并验证版本
升级、进程切换和历史重放。

# Reserved System Storage

以下 slot 地址已为 UIP-0014 预留，避免后续与其他经济状态冲突：

```text
USDB_SYSTEM_STATE_ADDRESS
  = 0x0000000000000000000000000000000000001000

QUOTE_POLICY_VERSION_SLOT
  = 0x06ed1ff69c0a83234a648936403718a01fd0c0e6caabe4eea61d7735f63db832
LEADER_QUOTE_WINDOW_BLOCKS_SLOT
  = 0x34d422b9f7b2447c9ad568159320894837919eacfd196ee5c5ede41376c56358
LEADER_LAST_VALID_QUOTE_BLOCK_MAP_BASE
  = 0x9f4c948c72431d7f43911f1f1231509866c87a43729568fdf10a86f9291b9cba
```

预留 slot 不表示对应正式 v1 状态转换已经冻结：

- policy `0` 只要求 `QUOTE_POLICY_VERSION_SLOT = 0`。
- test-only FixedPriceHeartbeat 只审计 active policy version，不写 per-Leader map。
- 正式 v1 必须在激活前重新审计 window、mapping key、容量和清理规则。
- 普通 EVM transaction 和用户合约不得直接修改 reserved system storage。

# 历史重放

历史重放必须只使用：

- 对应高度的 UIP-0008 USDB activation checkpoint。
- checkpoint 绑定的 BTC activation registry revision。
- UIP-0007 selector 和 UIP-0006 historical profile。
- header 中的 current-block quote evidence 或可验证 commitment。
- parent USDB state 中正式 policy 明确定义的 quote state。

禁止查询当前 head 的 quote 状态或实时外部 RPC 来验证历史区块。

升级后的 binary 必须继续支持链历史中所有曾正式激活的 quote policy。reserved
conformance binary 只需按测试计划支持 fake history，例如 fake v3 binary 必须能重放
fake v2 区块。

# 与其他 UIP 的关系

UIP-0004：

- nominal `effective_energy` 定义不变。
- quote policy 只决定 USDB chain 当前块实际使用哪个 energy。

UIP-0005：

- level thresholds 和 difficulty factor 公式不变。
- 输入改为 `QuotePolicyDecision.candidate_energy`。

UIP-0006/UIP-0007：

- selected pass 和 historical profile 仍是 quote subject 的基础。
- 正式 quote evidence 必须绑定 selector，不能选择另一个 pass。

UIP-0012：

- 当前样本必须使用 `QuotePolicyDecision.collaboration_energy_for_k`。
- difficulty 和 K 不得使用不同 quote 解释。

UIP-0013：

- FixedPrice v1 不接受 miner price report。
- test-only FixedPriceHeartbeat 不改变 fixed price。
- 正式 v1 是否与 dynamic price update 合并，由后续 price source 设计冻结。

# 实现影响

go-ethereum：

- `internal/usdb/quote_policy.go`
- `internal/usdb/economic_activation_conformance*.go`
- `consensus/ethash/consensus.go`
- `consensus/ethash/usdb_reward.go`
- `core/usdbstate/state.go`
- `params/config.go`

实现必须保持：

- 公共 quote context / decision 不依赖 build tag。
- fake policy 语义只在 conformance build tag 下编译。
- default binary 对正式 v1 和 reserved ID 都 fail closed。
- 所有 state writes 在完整验证后原子应用。

# 测试要求

至少覆盖：

- policy `0` 使用 nominal effective energy / CE，且不认可 quote。
- policy `0` decision 不引用可变 profile big integer。
- 正式 v1 未实现时 fail closed。
- default binary 拒绝 reserved fake ID。
- fake v2 使用 raw energy / `CE=0`。
- fake v3 的 current-block implicit FixedPriceHeartbeat 使用 effective energy / nominal CE。
- fake v3 拒绝错误 price policy、错误 reward recipient 和显式伪造 evidence。
- builder 与 validator 对同一 header 得到相同 decision 和 difficulty。
- reward/K 与 difficulty 使用同一 policy decision。
- decision version 与 activation 不一致时不产生 state writes。
- default -> fake v2 -> fake v3 restart/replay。
- same-parent replay 得到相同 state root。
- reorg 后按 replacement branch 的 activation 和 decision 重算。
- conformance binary 和 reserved ID 不进入 production artifact。

# 待审计问题

| 问题 | 当前结论 | 后续动作 |
| --- | --- | --- |
| 首版 public policy | `quote_policy_version = 0`，完全禁用。 | 保持 nominal energy / CE 回归测试。 |
| FixedPriceHeartbeat | 仅作 build-tagged conformance，不是正式 v1。 | 禁止进入 public activation。 |
| 正式 v1 source | 尚未冻结，必须有实际 per-Leader 报价意义。 | 与 dynamic price source UIP 一并设计。 |
| 当前块还是下一块生效 | conformance heartbeat 当前块生效；不约束正式 v1。 | 正式 v1 按 evidence carrier 和抗操纵要求冻结。 |
| quote authorization | 未冻结。 | 明确 pass owner、quote key、signature 或 source proof。 |
| quote state/window | slot 地址预留，语义未冻结。 | 激活前评估容量、清理、bootstrap 和恢复路径。 |
| 是否需要 UIP0014 | 只有 miner 实际参与报价或明确需要 activity gate 时才启用。 | 全局 oracle 模式下允许长期保持 policy `0`。 |
