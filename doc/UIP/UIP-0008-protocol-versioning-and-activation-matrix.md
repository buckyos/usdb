UIP: UIP-0008
Title: Protocol Versioning and Activation Matrix
Status: Draft
Type: Process / Standards Track
Layer: Process / Consensus / Indexer / Validator
Created: 2026-04-26
Requires: UIP-0000, UIP-0001, UIP-0002, UIP-0003, UIP-0004, UIP-0005, UIP-0006, UIP-0007
Activation: See owner-scoped BTC registries and USDB activation schedules

# 摘要

[UIP-0016](./UIP-0016-miner-pass-operation-eligibility.md) 规划 MinerPass 操作资格和跨地址继承，需要按 BTC source、rules scope 和历史 BTC 高度分派 schema/state-machine 版本。其状态机 v2 沿用 JSON schema v1，二者独立编号；受影响编码须完成专项验收；本引用不添加 activation record，也不修改现有 legacy registry。

本文定义 USDB 经济模型相关协议的版本字段、激活矩阵、历史重放规则和 state commit 承诺边界。

UIP-0008 不直接定义新的 pass schema、energy 公式或 USDB chain reward 公式。它定义的是：

- 不同协议版本字段的职责边界。
- 某个网络、某个高度应该使用哪个版本。
- 历史查询和 validator replay 如何按历史高度选择版本。
- `snapshot_id`、`system_state_id`、`local_state_commit` 应如何与激活版本关联。

# 动机

旧实现曾使用类似 `USDB_INDEX_FORMULA_VERSION` 的全局常量。当前参考实现已按共识所有权拆分版本来源：BTC-side USDB 服务按 BTC source、USDB rules scope、registry revision 和历史高度查询本地机器可读 registry；USDB chain 节点按自身 genesis / chain config 和 USDB block number 查询版本。正式网络不得依赖代码发布或远程 RPC 来隐式改变本链共识规则。

影响共识或经济结果的变更必须满足：

- 新版本必须有显式版本号。
- 新版本必须有网络化激活规则。
- 历史高度必须按当时激活的版本重放。
- BTC 侧状态派生、BTC-side USDB state view 和 USDB validator 校验不能各自使用不同版本。

# 非目标

本文不定义：

- 具体 pass inscription schema。
- 具体 pass 状态机。
- 具体 energy / effective energy / level 公式。
- 具体 USDB chain reward、difficulty、CoinBase、price 或分账公式。
- 主网最终激活高度。

这些内容由对应 UIP 定义。本文只定义版本和激活机制。

# 术语

| 术语 | 含义 |
| --- | --- |
| `btc_activation_record` | BTC registry `records[]` 中的一项；描述一个 `version_family` 在某个 BTC 高度的状态和版本值。 |
| `rules_scope` | 独立的 USDB BTC 数据解释域；同一个 BTC source 可以供不同 rules scope 使用。旧 v2 registry 隐式属于 `legacy`。 |
| `btc_registry_revision` | 单个 BTC source / rules scope registry 的一次完整、不可变快照；包含该 revision 之前的全部历史记录，不是某个协议的版本号。 |
| `activation_registry_id` | 一个 `btc_registry_revision` canonical encoding 的哈希；v3 绑定 BTC source 与 rules scope，不包含 USDB chain config。 |
| `usdb_activation_checkpoint` | `ChainConfig.usdb.activations[]` 中的一项；以 USDB block 为锚点，完整携带全部 USDB version fields、一个 BTC registry binding 和该阶段的 BTC anchor max age。 |
| `usdb_activation_schedule` | 同一 USDB network 按 block 严格排序的全部 `usdb_activation_checkpoint`；代码中的 `activations[]` 即该 schedule。 |
| `activation_matrix` | 对按 chain/network/height 选择规则这一机制的总称。具体讨论时必须明确是 BTC registry records，还是 USDB activation schedule。 |
| `active_version_set` | 在指定 BTC registry revision 和 BTC 高度下派生出的 BTC-side version fields；由 UIP-0006 external state 暴露。 |
| `active_version_set_id` | BTC-side `active_version_set` canonical encoding 的哈希。 |
| `resolved_usdb_versions` | 按 USDB block 从最近一个 `usdb_activation_checkpoint` 取得的完整 USDB-chain version fields。 |
| `version_family` | 一类版本字段，例如 `energy_formula_version`、`payload_version`。 |
| `chain_context` | 进行版本选择所需的链、网络和高度信息。 |
| `cross_chain_release_manifest` | 将独立的 BTC registry ID 与 USDB chain genesis / chain config 身份关联起来的发布审计文件；不参与任一链的运行时版本选择。 |

为避免歧义，本文不使用未限定所有者的“每条 activation”表达规范要求：

- “BTC activation record”始终指 registry `records[]` 中的单版本族记录。
- “USDB activation checkpoint”始终指 `ChainConfig.usdb.activations[]` 中的完整检查点。
- “registry revision”始终指 BTC registry 的完整快照，不指单条记录，也不指 USDB activation checkpoint。

# 激活机制概念

激活机制用于回答一个问题：在某条链、某个网络、某个高度，系统应该使用哪一组协议规则。

它不是代码发布机制，也不是运行时开关。代码可以同时支持多个版本，但只有对应 BTC registry 或 USDB activation schedule 已经对目标网络和目标高度激活的版本，才可以用于共识、历史查询和 validator replay。

BTC registry 和 USDB chain config 使用同一套高度选择原则，但数据形状不同：

```text
BTC:
    btc_registry_revision = 一个 BTC source / rules scope 的完整 registry 快照
    btc_activation_record = records[] 中一个 version family 的激活记录
    active_version_set    = 在指定 registry revision / BTC height 下派生出的版本集合

USDB:
    usdb_activation_schedule   = ChainConfig.usdb.activations[]
    usdb_activation_checkpoint = schedule 中一个完整版本快照、BTC registry binding 和 anchor max age
    resolved_usdb_versions     = 目标 USDB block 最近一个已生效 checkpoint 的完整 versions
```

BTC 示例：

```text
registry scope:
    chain = BTC
    network_id = btc-regtest
    anchor = btc_height

btc_activation_records:
    btc_height >= 0 -> energy_formula_version = uip-0003-pass-energy-formula:v1
    btc_height >= 0 -> level_formula_version = uip-0005-level-and-real-difficulty:v1

query context:
    chain = BTC
    network_id = btc-regtest
    btc_height = 100

active_version_set:
    energy_formula_version = uip-0003-pass-energy-formula:v1
    level_formula_version = uip-0005-level-and-real-difficulty:v1
```

后续如果 `energy_formula_version:v2` 在 `btc_height = 200_000` 激活，则：

- 查询 `btc_height = 199_999` 必须使用 v1。
- 查询 `btc_height = 200_000` 必须使用 v2。
- reorg 后必须按新 canonical 分支上的高度重新判断版本。

USDB activation schedule 示例：

```text
USDB block 0:
    btc_registry = R1
    btc_anchor_max_age_blocks = 6650
    versions = { payload=1, btc_anchor=1, difficulty=1, reward=0, ... }

USDB block 100:
    btc_registry = R2
    versions = { payload=1, difficulty=1, reward=0, ... }

USDB block 200:
    btc_registry = R2
    versions = { payload=1, difficulty=1, reward=1, ... }
```

block 100 是 registry-only checkpoint：USDB version fields 没有变化，但从该高度起绑定新的 BTC registry revision。block 200 是 policy checkpoint：只改变 `reward_rule_version`，但记录仍必须重复完整 USDB version set。若多个 USDB policy 在同一 block 生效，必须合并到同一个 checkpoint，不能创建同高多条记录。

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 版本族

不同版本字段有不同职责。实现不得把所有变更合并成一个全局版本号。

铭文 JSON `v` 对应载荷 schema，不选择资格执行器。当前受支持组合是 `inscription_schema_version=v1` 与 `pass_state_machine_version=v2`；零余额开户、来源检查和跨地址继承改变状态机，不改变现有 JSON 字段契约。未来应逐族评审升级，不要求各版本数字一致。

本文维护 version family registry 的通用字段名和激活语义。每个 version family 的业务含义、输入输出、fail-closed 条件和可选 disabled 状态由对应 UIP 定义。

| Version Family | 类型 | 主要链路 | 说明 |
| --- | --- | --- | --- |
| `inscription_schema_version` | string | BTC | pass 铭文 JSON schema 和字段解释。 |
| `pass_state_machine_version` | string | BTC | pass 状态转移、terminal state、remint / consume 语义。 |
| `energy_formula_version` | string | BTC-side `usdb-indexer` | raw energy、penalty、inheritance、settlement 公式。 |
| `effective_energy_formula_version` | string | BTC-side `usdb-indexer` | collab contribution、Leader effective energy 聚合规则。 |
| `level_formula_version` | string | BTC-side `usdb-indexer` / USDB validator | `effective_energy -> level -> difficulty_factor_bps` 规则。 |
| `query_semantics_version` | string | RPC / indexer | historical query、pagination、projection、exact / at_or_before 语义。 |
| `state_view_version` | string | RPC / validator replay | UIP-0006 state view JSON 结构版本。 |
| `payload_version` | uint8 | USDB chain header | UIP-0007 `ProfileSelectorPayload` binary layout。 |
| `btc_anchor_policy_version` | uint16 | USDB chain header transition / chain config | UIP-0007 父子 BTC anchor 单调推进、同高 identity 和 bounded-reuse 规则。 |
| `difficulty_policy_version` | uint16 | USDB chain header / chain config | `level -> real difficulty` 共识算法版本。 |
| `reward_rule_version` | uint16 | USDB chain reward / execution | reward 输入校验、reward recipient 校验和最终 reward state transition。 |
| `coinbase_emission_policy_version` | uint16 | USDB chain reward / execution | UIP-0011 CoinBase emission 公式版本。 |
| `fee_split_policy_version` | uint16 | USDB chain reward / execution | UIP-0011 / UIP-0010 交易手续费分账公式和 Dividend activation 版本。 |
| `collaboration_efficiency_policy_version` | uint16 | USDB chain reward / reserved storage | UIP-0012 协作效率系数 `K`、rolling window、warmup 和 state update 规则版本。 |
| `price_policy_version` | uint32 | USDB chain price state / reward | UIP-0013 `price_atoms_per_btc` 状态转换、source kind 和 range 规则版本。 |
| `quote_policy_version` | uint16 | USDB validator / reward | UIP-0014 Leader quote activity、candidate energy 和 candidate level 规则版本。 |
| `aux_pool_policy_version` | uint16 | USDB chain reward / system contract | UIP-0015 辅助算力池证明、分配和状态转换规则版本；`0` 可表示 disabled，但只能由 UIP-0015 明确定义。 |
| `commit_protocol_version` | string | USDB local state | `local_state_commit` / `system_state_id` 输入与编码规则。 |
| `balance_history_semantics_version` | string | balance-history | upstream balance snapshot / UTXO query 语义。 |

字符串版本建议使用：

```text
uip-0003-pass-energy-formula:v1
uip-0004-collab-leader-effective-energy:v1
uip-0006-usdb-economic-state-view:v1
```

进入 USDB block header、chain config、USDB activation schedule 或 reserved system state
的版本字段应该使用固定宽度整数。首个启用版本必须使用正整数版本号，例如
`payload_version = 1`、`btc_anchor_policy_version = 1`、
`difficulty_policy_version = 1`。

首个正式 USDB-chain 网络必须启用 level-based difficulty policy，不定义 `difficulty_policy_version = 0` 作为“未启用”保留值。若未来某个独立测试网络确实需要无 difficulty policy 模式，必须由后续 UIP 单独定义，不得复用正式网络语义。

可选经济组件如果需要 disabled 状态，必须由对应 UIP 明确允许 `0` 的含义。UIP-0014
允许 `quote_policy_version = 0` 表示不启用 quote activity、直接使用 nominal
effective energy；UIP-0015 允许 `aux_pool_policy_version = 0` 表示辅助算力池未启用。
这不代表其他 version family 自动允许 `0`。

实现测试可以占用高位 reserved version ID 做 activation conformance，但这些 ID
不得进入 public genesis、release manifest 或 production artifact。默认构建必须
拒绝 reserved test ID；测试结果不能冻结 future production policy 的版本号或语义。

# 两类激活记录与所有权

BTC activation records 和 USDB activation checkpoints 由不同的本地共识配置拥有，禁止合并成一个运行时 registry。

## BTC Source 与 USDB Rules Scope

BTC source 表示被读取的 Bitcoin 链；`rules_scope` 表示 USDB 如何解释该链上的 pass、energy 和 economic state。USDB 测试网与正式网可以同时读取 `btc-mainnet`，但必须能够分别维护规则历史和激活日程。两者不能仅凭 BTC network 自动共用同一个 current registry。

- v2 registry 不允许出现 `rules_scope` 字段，按 `legacy` 解释；原始 JSON、registry ID、active set ID 和已冻结 embedded artifacts 保持不变。
- v3 registry 使用 schema `uip-0008-btc-activation-registry:v3`，必须在 `scope` 内显式声明非 `legacy` 的 `rules_scope`。
- scope token 必须是 1 至 64 个 ASCII 字符，匹配 `[a-z0-9]+(?:-[a-z0-9]+)*`；`legacy` 是兼容保留名，禁止用于 v3。
- scope 是共识身份的一部分，不是运营者可自行选择的功能开关；它与精确 registry ID 必须由目标 USDB 网络的规范配置冻结。
- 更换 scope 不会改变 BTC source，也不会自动改变 chain ID、genesis 或铭文 JSON。当前铭文没有因此新增 chain ID 字段，同一 BTC 铭文在不同 scope 中的解释仍取决于各自索引规则和 origin。

### Registry Scope 字段

每个 BTC registry 文件只允许描述一个 BTC source 和一个 rules scope。顶层 scope 包含：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `network_type` | enum | `mainnet`、`testnet`、`signet` 或 `regtest`，必须与 Bitcoin Core network 对应。 |
| `network_id` | string | 具体 BTC source network ID，禁止省略。 |
| `rules_scope` | string | v3 必填的独立 USDB 规则域；v2 禁止出现该字段并隐式使用 `legacy`。 |
| `stable_lag_blocks` | uint32 | balance-history 从 BTC tip 排除的确认块数；必须为正数并进入 registry identity。 |

BTC record 固定以 `btc_height` 为 anchor，不再逐条重复 chain/network/anchor：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `uip` | string | 例如 `UIP-0003`。 |
| `version_family` | string | 只允许 BTC-side family。 |
| `version_value` | string | 该 family 的具体版本值。 |
| `activation_height` | uint64 | 生效 BTC 高度。 |
| `status` | enum | `Planned`、`Active`、`Deferred`、`Superseded`。 |
| `supersedes` | optional | 被替代的 version value。 |
| `notes` | string | 必要说明。 |

示例：

```json
{
  "schema_version": "uip-0008-btc-activation-registry:v2",
  "scope": {
    "network_type": "regtest",
    "network_id": "btc-regtest",
    "stable_lag_blocks": 10
  },
  "records": [{
    "uip": "UIP-0003",
    "version_family": "energy_formula_version",
    "version_value": "uip-0003-pass-energy-formula:v1",
    "activation_height": 0,
    "status": "Active",
    "supersedes": null,
    "notes": "Regtest activates raw energy formula v1 from genesis."
  }]
}
```

## USDB Chain Config

USDB-chain integer version families 必须写入 USDB chain genesis / `ChainConfig.usdb.activations[]`。数组中的每个 USDB activation checkpoint 以 `usdb_block` 为 anchor，并携带该高度完整的 USDB chain version set、`btcActivationRegistryId` 与正数 `btcAnchorMaxAgeBlocks`。Go 类型名 `USDBConsensusActivation` 和 JSON 字段名 `activations[]` 保持不变，但其元素语义是完整 checkpoint，不是单个 version family 的增量记录。USDB chain 节点不得读取 BTC registry 或调用 companion RPC 来决定 expected USDB chain version。

`btcAnchorMaxAgeBlocks` 是 `btc_anchor_policy_version` 的 activation-bound 参数，
不是 version family，也不是从本地 block time / wall clock / BTC RPC tip 动态计算的值。
已生效 checkpoint 的该值进入 `CheckCompatible`；future checkpoint 只可在生效前调整。

每个 `ChainConfig.usdb.activations[i]` checkpoint 的 `btcActivationRegistryId` 绑定从该 USDB block 起允许引用的 immutable BTC source / rules scope registry revision。该字段是 cross-chain historical profile 的辅助共识条件，不是 USDB-chain version activation source：

- miner/validator 必须在本地 Go golden artifact 中找到该 registry ID，否则 fail closed。
- payload 的 `btc_height` 必须在该 registry 的高度表中解析 expected `active_version_set`。
- companion RPC 返回的 `activation_registry_id + active_version_set + active_version_set_id` 必须与本地 lookup 精确一致。
- 后续 USDB activation checkpoint 可以切换到同一 BTC source / rules scope catalog 的新 revision；旧 revision 必须继续保留以便历史 replay。更换 scope 是独立的状态解释迁移，不能伪装成原 catalog 的普通 revision。
- 已生效 checkpoint 的 registry ID 变更必须进入 `CheckCompatible`，不能由 CLI、RPC 或普通配置热更新覆盖。

# 激活矩阵规则

## BTC Registry 规则

BTC registry 必须遵循：

- 未列出的 BTC network 不得默认激活；缺少对应 BTC registry artifact 时必须 fail closed。
- 单个 BTC registry 文件不得包含其他 BTC network 或任何 USDB-chain family。
- 同一 `version_family`、同一 BTC source / rules scope、同一高度只能有一个 active version。
- 后激活的版本必须显式 `supersedes` 被替代版本，除非该 family 之前没有 active version。
- `Planned` 记录不得影响 validator、indexer 或 RPC 查询结果。
- `Deferred` 和 `Superseded` 记录只能用于审计和历史说明。
- 同一 BTC source / rules scope 的 registry revision 必须从 1 连续递增；每个新 revision 必须逐条保留旧 revision 的记录，禁止改写历史。一个 catalog 不得混合 scope、schema 或 stable lag。
- catalog 必须显式指定一个 current revision；historical request 可以按 registry ID 读取任何保留 revision。

若两个 active 记录在同一高度冲突，节点必须拒绝启动公开网络服务，不能任选其一。

外部 catalog 使用 `uip-0008-btc-activation-registry-catalog:v1`，顶层只包含 `schema_version`、`current_registry_id` 与 `registries`；数组元素是完整 registry 文档，不是文件名。`current_registry_id` 必须指向数组中存在的 revision，并与部署冻结的精确 ID 相等。读取文件不得隐式选择最高 revision 或最近新增的 revision。历史请求只能选择同一 catalog 保留的 revision；未知 scope、source、ID、字段或缺失选择条件都必须失败关闭。

catalog 允许追加历史兼容 revision，不等于现有可写 dataset 已具备在线迁移能力。参考实现当前将 source、scope、origin 与精确 current registry ID 绑定到持久化数据；修改 current ID 时必须使用独立 dataset 并重建。后续在线迁移需要独立规范及验收，不能通过只改配置绕过绑定。

## USDB Activation Schedule 规则

USDB activation schedule 必须遵循：

- 未配置的 USDB network 不得默认激活；缺少本地 genesis / chain config schedule 时必须 fail closed。
- checkpoint 必须按 `block` 严格递增，同一 USDB block 只能有一个 checkpoint。
- 每个 checkpoint 必须完整携带所有 USDB chain version fields、
  `btcActivationRegistryId` 和正数 `btcAnchorMaxAgeBlocks`；不得把缺失字段解释为继承上一条。
- 查询目标 USDB block 时，选择 `block <= target` 的最后一个 checkpoint；首次 checkpoint 之前 USDB consensus inactive。
- 单个或多个 USDB policy 在同一 block 变化时，都必须生成一个新的完整 checkpoint。
- 仅切换 BTC registry revision 也必须生成一个新的完整 checkpoint，并重复不变的 USDB version fields。
- 已生效 checkpoint 的 versions、registry binding 或 BTC anchor max age 都属于
  chain compatibility 边界；未来 checkpoint 只可在生效前更新。

## P2P Fork ID 与激活高度

USDB 节点的 EIP-2124 fork ID 必须将 `ChainConfig.usdb.activations[].block` 纳入 USDB 分叉高度列表。仅切换 registry binding、anchor max age 或版本字段的 checkpoint 也占据一个分叉高度，不要求先判断哪些字段改变。新增高度与现有顶层 EVM fork blocks 合并后排序、去重，并去掉 block 0；同高多个规则只计一次，genesis 配置不增加额外高度条目。

这里收集的全部是 **USDB block number**。BTC registry 中的 `activation_height` / BTC height 不进入 USDB fork ID；它们由目标 USDB checkpoint 绑定的 registry 与区块 BTC anchor 校验。

`NewID`、`NewFilter` 与 `NewStaticFilter` 必须复用同一高度收集逻辑。eth Status 握手、协议广播的 ENR 属性和随 chain head 更新的 ENR 使用同一 fork ID 计算路径；`NewStaticFilter` 仍按 block 0 的静态视角过滤，不等同于已同步节点的动态 filter。

对于旧节点确实遗漏的新 USDB 高度 `H`，其余 genesis 与分叉历史相同且没有其他不兼容条件时：

| 本地升级节点状态 | 远端状态 | 新握手行为 |
| --- | --- | --- |
| head < H | 旧节点仍报告相同 checksum、`Next=0` | 可按 EIP-2124 建立连接；不能在激活前据此认定对端已升级 |
| head >= H | 旧节点报告 H 之前的 checksum、`Next=0` | 拒绝不兼容的 fork ID |
| head >= H | 已升级但尚未同步到 H 的节点，报告旧 checksum、`Next=H` | 允许其按已知日程追块 |

已有连接不因经过 H 自动重新握手或主动断开；收到区块后的共识验证仍负责拒绝不兼容内容。这项检查不能远程强制所有节点升级，也不能阻止仍有人按旧规则运行旧链。

fork ID 包含 4 字节 CRC32 `Hash` 和下一个分叉高度 `Next`，是 genesis 与分叉高度历史的兼容性摘要，不是版本字段、完整 chain config 或 registry ID 的密码学承诺。相同高度配置不同规则时可能有相同 fork ID；若 H 已作为顶层 EVM fork 出现在旧列表中，去重后也不能识别遗漏的 USDB 规则。这些情况仍须通过冻结 chain config、registry identity 和区块共识验证处理。

当前 testnet-v0 仅有 block 0 的 USDB checkpoint，按上述规则不会改变既有 fork ID。本次完善高度收集不新增激活 checkpoint，不改变网络身份、genesis 或业务规则。

# Version Lookup

实现必须在每次历史查询或 validator replay 时按本链权威配置和历史高度查询版本，而不是读取全局常量或远程服务的 current head。

输入：

```text
btc_context  = selected_btc_registry_revision + btc_height
usdb_chain_context = local_chain_config + usdb_block
```

输出分属两个共识所有者，禁止合并为一个运行时 set：

```text
btc_active_version_set =
    inscription_schema_version?
    pass_state_machine_version?
    energy_formula_version?
    effective_energy_formula_version?
    level_formula_version?
    query_semantics_version?
    state_view_version?
    commit_protocol_version?
    balance_history_semantics_version?

resolved_usdb_versions =
    payload_version
    btc_anchor_policy_version
    difficulty_policy_version
    reward_rule_version
    coinbase_emission_policy_version
    fee_split_policy_version
    collaboration_efficiency_policy_version
    price_policy_version
    quote_policy_version
    aux_pool_policy_version
```

规则：

- BTC-side USDB pass、state 和 energy 派生必须先按配置的 BTC source、rules scope 和精确 registry ID 选择唯一规则历史，再使用 `btc_height` 选择版本。balance-history 继续提供 source-scoped 上游数据；其语义版本与 stable lag 必须满足消费者 registry，不能把可复用的上游数据等同于可复用的 USDB 索引状态。
- USDB-chain payload、difficulty、reward 和执行规则必须使用本地 genesis / chain config 按 `usdb_block` 选择完整 checkpoint 和 `resolved_usdb_versions`。
- USDB chain validator 必须使用目标 USDB block 的 activation checkpoint 所绑定的 `btcActivationRegistryId + payload.btc_height` 在本地 golden catalog 中选择 BTC active set，再与 companion historical response 交叉校验。
- CrossChain 规则必须明确主锚点和辅助条件。
- 查询历史 BTC 高度时，禁止用当前 BTC head 的版本解释旧高度。
- 校验历史 USDB block 时，禁止用当前 USDB chain head 的版本解释旧块。
- USDB chain 节点禁止通过 RPC 查询 expected USDB chain activation；RPC 只用于读取 payload 指向的历史 BTC economic state。

# CrossChain 激活

跨链规则必须写明主锚点。

推荐语义：

```text
active_if =
    primary_anchor_condition
    AND all_auxiliary_conditions
```

例如 USDB-chain reward rule 以 `usdb_block` 为主锚点，但它引用的 pass economic profile 必须按 payload 中的 `btc_height` 使用 BTC-side active version set 解析。

这意味着：

- USDB block 的 `payload_version`、`btc_anchor_policy_version`、
  `difficulty_policy_version`、`reward_rule_version`、
  `coinbase_emission_policy_version`、`fee_split_policy_version`、
  `collaboration_efficiency_policy_version`、`price_policy_version`、
  `quote_policy_version` 和 `aux_pool_policy_version` 只由 USDB chain
  genesis / chain config 决定。
- payload 指向的 BTC-side USDB state 由选定 BTC registry revision 中 `btc_height` 对应的 BTC activation records 决定。
- USDB chain config 绑定可接受的 BTC registry identity，防止两个格式正确但内容不同的 registry 被不同 validator 接受。
- 两者都必须可重放，且不得互相覆盖。

# State Commit 绑定

`snapshot_id` 是 upstream balance-history 的 state identity，不应该承诺 USDB indexer 的 energy 或 pass 公式。

USDB 的 `system_state_id` / `local_state_commit` 必须绑定足够信息，使 validator 和审计工具能够发现版本不一致。至少应绑定：

- upstream `snapshot_id`。
- `commit_protocol_version`。
- 当前 context 下的 `active_version_set_id`。
- 影响派生状态的输入数据 commit。

`local_state_commit` 不需要直接包含完整 `active_version_set`。它只需要承诺 `active_version_set_id`，前提是节点、validator 和审计工具可以通过稳定的 BTC registry revision 查询到该 id 对应的完整 version set。

推荐定义：

```text
activation_registry_id = sha256(canonical_network_scoped_btc_registry)
active_version_set_id  = sha256(canonical_active_version_set)
local_state_commit     = hash(commit_protocol_version, snapshot_id, active_version_set_id, derived_state_root)
system_state_id        = hash(snapshot_id, local_state_commit)
```

## Legacy Registry v2 与 Active Set v1 编码

旧 legacy 机器可读 registry 固定在 `src/btc/usdb-util/activation-registry/<network-id>[-revision-N].json`，当前内嵌 `btc-mainnet.json`、`btc-regtest.json` 和 staged `btc-regtest-revision-2.json`，单 revision schema 为 `uip-0008-btc-activation-registry:v2`。v2 scope 固定包含 `network_type + network_id + stable_lag_blocks`；`stable_lag_blocks` 必须为正数，是 balance-history stable frontier 的 network protocol value，不是运行参数。JSON parser 必须拒绝未知字段、重复字段、类型错误、scope 不匹配、零 lag、USDB-chain family 和同 family/height 的 active 冲突；catalog parser 还必须拒绝 revision 缺口、重复 identity、scope/lag 变化和历史 record 改写。没有独立 catalog 的网络不得回退到其他网络 registry。

v2 的 lag 在同一 network catalog 内不可升级。public freeze 前可以重写 draft artifact
并原子重生成 registry ID、Go golden、USDB chain config binding 和 release manifest；
public freeze 后若需调整，必须由新 schema 定义按高度可重放的 lag 语义，或发布新网络，
不能把 scope 变化伪装成普通 revision。

所有 string 使用 `u32 big-endian byte_length || UTF-8 bytes`。version value 和 activation height integer 使用 `u64 big-endian`；scope 的 `stable_lag_blocks` 使用 `u32 big-endian`。string/integer union 使用 `0x00` / `0x01` tag；optional value 使用 `0x00` absent 或 `0x01 || encoded_value`。hash 输出为 lowercase 64-character hex text。

`activation_registry_id` 的 hash input 为：

1. length-prefixed domain `usdb-btc-activation-registry:v2`。
2. length-prefixed `schema_version`。
3. length-prefixed fixed chain tag `BTC`。
4. length-prefixed scope `network_type`、`network_id`、fixed tag `stable_lag_blocks`、`u32 big-endian stable_lag_blocks` 和 fixed anchor tag `btc_height`。
5. `u32 big-endian record_count`。
6. canonical-sorted records。排序 key 固定为 `(version_family, activation_height, status, uip, version_value, supersedes, notes)`；每条 record 的编码字段顺序固定为 `(uip, version_family, version_value, activation_height, status, supersedes, notes)`。

排序中的 enum 顺序固定为：network type `mainnet, testnet, signet, regtest, devnet, local`；status `Planned, Active, Deferred, Superseded`；version family 使用本文 Version Family 表顺序。union value 先按 tag 排序，再按 string byte order 或 unsigned integer order 排序。

`active_version_set_id` 的 hash input 为 length-prefixed domain `usdb-active-version-set:v1`，随后按本文 Version Family 表的固定顺序编码全部 family：每个 family 先编码 length-prefixed canonical name，再编码 presence marker；present 时继续编码 tagged version value。未激活 family 必须显式编码 absent marker，禁止直接跳过。

两种 id 都使用 SHA-256。当前网络作用域 registry golden ID 为：

```text
btc-mainnet (stable_lag_blocks=10) = a6350cd6a68755ea64edf537f35c1eca4421a970e2ecfd67aaa29075aae57224
btc-regtest revision 1 (current, stable_lag_blocks=10) = bfd8c7e41ab4035db64e52eb9ea55050c08211c2ae4c2a88d8b2fc17ae1718b0
btc-regtest revision 2 (staged, stable_lag_blocks=10)  = adcca18bb4eccd4715bb0d6ec69c7b3d5e09065fac0cb33b145db7b621f59fba
```

两个 legacy 网络当前激活相同的 BTC v1 九 family，因此旧格式跨实现 golden `active_version_set_id` 相同；这一兼容行为不适用于 v3 的不同 rules scope：

```text
01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691
```

UIP-0006 state view 必须返回 `stable_lag`、`activation_registry_id`、完整 `active_version_set` 和 `active_version_set_id`。`snapshot_id` 承诺 upstream balance-history identity，其中包含 `stable_lag`，但不包含 USDB formula；USDB validator 还必须把返回的 lag 与本地 registry scope 比较。`local_state_commit` 必须绑定目标高度的 `active_version_set_id`。

## Scoped Registry v3 与 Active Set v2 编码

v3 registry 继续使用上一节的 string、integer、optional 和 record 排序编码，仅有以下差异：

1. registry hash domain 改为 `usdb-btc-activation-registry:v3`，schema 字符串为 `uip-0008-btc-activation-registry:v3`。
2. scope 编码在 `network_type`、`network_id` 之后插入 length-prefixed fixed tag `rules_scope` 和 scope token，再继续编码 `stable_lag_blocks` 与其余字段。
3. 派生出的 `active_version_set` 保留原来的 version family 字段，并新增 `scope: {"network_id": "btc-mainnet", "rules_scope": "<reviewed-scope>"}` 对象；scope 缺失只对应 legacy set，不能用 `null` 冒充缺失。
4. scoped active set 的 hash domain 为 `usdb-active-version-set:v2`；随后依次编码 length-prefixed `network_id`、`rules_scope`，最后按旧格式的 family 顺序编码全部 presence marker 与 version value。

因此，即使两个 scope 在同一 BTC 高度使用完全相同的公式版本，其 registry ID 和 active set ID 仍不同；后者经 `local_state_commit` 进一步隔离 `system_state_id`。上游 `snapshot_id` 可以相同，因为 scope 隔离不要求复制同一 BTC source 的原始余额历史。

`stable_lag_blocks` 仍是 registry 固定字段；本次 scope 扩展没有引入按高度切换 lag 的能力。所有既有 v2 registry 和 legacy active set 的编码、JSON 与 golden ID 均不改变。

## Cross-chain Release Manifest

默认 `src/btc/usdb-util/release-manifest.json` 继续使用 `uip-0008-cross-chain-release-manifest:v3`，其 legacy 内容和默认 Go golden 字节不变。新增 audit schema `uip-0008-cross-chain-release-manifest:v4` 支持在发布审计时关联：

- 每个 BTC source / rules scope catalog 的 revision、current marker、artifact 与 `activation_registry_id`。
- USDB network 的 `chain_id`、genesis hash、chain-config source，以及按 USDB block 排序的完整 activation checkpoints；checkpoint 包括 BTC registry binding、BTC anchor max age 和全部 USDB policy versions。

v4 的 BTC binding 增加可选 `rules_scope`；legacy 必须省略，新 scope 必须显式填写。revision/current 校验按 `(network_id, rules_scope)` 分组，因此一份 manifest 可以同时审计 legacy 与同一 BTC source 下的多个 scoped catalog。每组必须保持连续 revision 并有且仅有一个 current。同一 USDB chain 的全部 activation checkpoints 必须绑定同一 BTC source / rules scope；跨 scope 迁移不能由普通 checkpoint 切换冒充。

`validate_btc_catalog_bindings` 必须进行双向完整校验：manifest 声明的每个 revision 都应存在于提供的 catalog 中，提供的每个 catalog revision 也必须有对应 binding，并匹配 source、scope、revision/current 和规范 ID。不能只校验已经找到的交集而忽略缺失或额外条目。

外部审计生成使用 `generate_go_release_manifest_golden --manifest <文件> --catalog <文件> [--catalog <文件> ...] [--check] [输出路径]`；`--catalog` 必须配合 `--manifest`，`--check` 必须提供输出路径。不带 `--catalog` 的外部 manifest 解析只验证自身结构与绑定约束，不能替代 catalog 的双向完整性审计。

manifest 仅用于 release review、部署审计和 CI 一致性检查。它不得参与 BTC registry ID、BTC `active_version_set_id`、USDB chain header validation 或 USDB chain expected version lookup；修改一个网络的配置不得改变另一个网络的 runtime activation identity。

Rust `generate_go_release_manifest_golden --check` 必须保证 manifest 与 Go 内嵌 golden
一致；Go 测试必须再把该 golden 与 `params.USDBChainConfig`、`USDBGenesisHash` 逐字段
比较。该两段校验用于发现发布物漂移，不改变任一运行时 lookup authority。

# 历史重放规则

历史重放必须满足：

- 激活高度之前的事件按旧版本解释。
- 激活高度及之后的事件按新版本解释。
- reorg 后按新 canonical 分支重新选择 active version。
- 同一 historical context 下，`active_version_set` 必须稳定。
- 如果本地节点不支持目标高度需要的版本，必须返回明确错误，不能用最近版本替代。

版本变更不得 retroactively 改写旧高度，除非该 UIP 明确是开发期重建规则，并且未在公开网络激活。

# 跨版本 `prev` 继承

当 pass 通过 `prev` 继承旧 pass 时，继承边界必须按事件高度解释：

- `prev` pass 的 terminal / consumed 状态按该状态发生高度的 active version 计算。
- 新 pass 的 mint 和后续增长按新 mint 高度的 active version 计算。
- 如果公式升级改变 energy 单位、rounding 或可继承字段，升级 UIP 必须定义迁移函数。
- 如果公式升级保持可继承字段兼容，升级 UIP 必须显式说明可以直接继承。
- 未定义迁移函数或兼容声明时，不得允许跨版本继承产生新的 active pass。

当前开发阶段的 v1 公式可以从高度 `0` 重建，不需要长期保留 pre-standard v0 继承语义。

# Development 网络

开发网络可以在实现合并后从高度 `0` 激活 v1 规则，但必须满足：

- `network_type` 必须是 `regtest`、`devnet` 或 `local`。
- `network_id` 不得伪装成 mainnet / public testnet。
- local override 不得写入 public BTC registry 或 public USDB activation schedule。
- 开发期数据迁移不构成主网兼容承诺。

这里的 public/development 分类描述 USDB protocol network 的发布状态。`btc-mainnet` registry 只表示 indexer 读取的 BTC source network，并不等价于 USDB chain/USDB public mainnet 已激活；当前 height 0 记录使配置的 USDB indexing origin 之后统一使用 v1 解释。

# 首次公开网络上线

正式网和官方测试网首次上线时，首个实现完成的 v1 版本应该从 genesis / block 0 激活。

因此首次上线不需要考虑 pre-standard 历史版本的迁移窗口，也不需要为开发期 v0 行为保留长期兼容路径。迁移问题只适用于已经公开运行并已经存在历史状态的网络。

# Version Mismatch 错误

实现至少需要区分：

| 错误 | 触发条件 |
| --- | --- |
| `ACTIVATION_RECORD_NOT_FOUND` | 目标 network / height 找不到所需 version family。 |
| `ACTIVATION_RECORD_CONFLICT` | 同一 family 在同一 context 下存在多个 active version。 |
| `VERSION_NOT_SUPPORTED` | 本地实现不支持目标 active version。 |
| `ACTIVE_VERSION_SET_MISMATCH` | state view / local commit 声明的 active set 与本地 lookup 不一致。 |
| `FORMULA_VERSION_MISMATCH` | 派生字段使用的 formula version 与 expected version 不一致。 |
| `QUERY_SEMANTICS_VERSION_MISMATCH` | RPC 查询语义版本不匹配。 |
| `PAYLOAD_VERSION_MISMATCH` | USDB chain header payload version 不匹配。 |
| `DIFFICULTY_POLICY_VERSION_MISMATCH` | USDB chain payload 声明的 difficulty policy version 与 expected version 不一致。 |
| `COMMIT_PROTOCOL_VERSION_MISMATCH` | local state commit 编码版本不匹配。 |

# Backwards Compatibility

当前 USDB 项目仍处于开发阶段。尚未在公开主网激活的旧实现行为属于 pre-standard implementation draft，不需要作为长期兼容版本保留。

一旦某个 public network 进入 `Active`：

- BTC-side 后续变更必须新增 version 和对应 BTC activation record；USDB-chain 后续变更必须新增完整 activation checkpoint。
- 不得通过代码发布直接改变旧高度解释。
- 若无法双版本重放，必须提供一次性迁移和冻结高度说明。

# 参考实现影响

预计需要影响：

- `src/btc/usdb-util/src/types.rs`
- `src/btc/usdb-indexer/src/service/rpc.rs`
- `src/btc/usdb-indexer/src/index/energy.rs`
- `src/btc/usdb-indexer/src/index/energy_formula.rs`
- `src/btc/usdb-indexer/src/index/system_state.rs`
- balance-history snapshot semantics / RPC version exposure。
- `/home/bucky/work/go-ethereum` 的 USDB chain config、payload verifier 和 miner payload generation。

# 测试要求

至少需要覆盖：

- 不同 BTC height 返回不同 `energy_formula_version`。
- 激活高度前、激活高度、激活高度后行为。
- 未列出网络不激活。
- mainnet/regtest 使用各自文件且 registry ID 不同。
- registry source / rules scope 与配置不匹配、current pin 不匹配或 catalog 缺失时 fail closed。
- 同一 BTC source、同一公式版本、不同 scope 得到不同 registry、active set 和 local/system state identity；legacy golden 不变。
- 同一 source 的两个 scope 可以采用不同升级日程；测试 scope 的 catalog 修改不改变另一 scope 的结果。
- pass SQLite 与 energy RocksDB 同时校验 source/scope/origin/current ID；跨 scope、跨 origin、跨 revision 或混合双库均拒绝启动。
- 新 scope 拒绝认领非空但尚未绑定的旧库；旧 legacy 库可按兼容路径记录首次绑定。
- checkpoint 在安装前同时核对目标配置 pin、manifest registry ID、双库绑定和离线重算结果；跨 scope 失败关闭。
- BTC registry 拒绝 USDB-chain version family。
- conflicting BTC activation records fail closed。
- P2P 高度收集合并嵌套 USDB checkpoints 与顶层 fork blocks，排序、去重、去 0；registry-only checkpoint 也被收集。
- fork ID 在 H-1/H/H+1、新旧节点混合、落后节点追块及 head 跨 H 回退时行为符合 EIP-2124。
- eth Status 与 ENR 共用同一 fork ID；v0 block-0-only checkpoint 不改变原 checksum/next。
- 相同高度但不同规则或 registry 不被错误表述为 fork ID 可识别的差异；已连接节点不会因该 filter 自动断开。
- historical RPC 按目标高度选择版本。
- reorg 跨激活高度后重新选择版本。
- `active_version_set_id` mismatch。
- chain config 绑定未知或错误的 `activation_registry_id` 时 fail closed。
- registry catalog 拒绝 revision 缺口、多个 current、旧 record 改写和历史 active record 插入。
- Rust generator `--check` 可以证明提交的 Go golden artifact 与全部 BTC registry revisions 完全一致。
- validator 按 payload BTC 高度选择 expected set，并按 `energy/effective-energy/level` version 分派公式；未知版本 fail closed。
- `prev` 跨版本继承测试。
- USDB chain `difficulty_policy_version` mismatch。
- `btc_anchor_policy_version` 未知、`btcAnchorMaxAgeBlocks=0`、
  已生效 max-age 修改和 future max-age 修改的 `CheckCompatible` 边界。
- parent/child BTC height regression、同高 identity mismatch、age increment/reset、
  exact max 与 max+1。
- release manifest 中的 BTC registry ID 可由 artifact 重算。
- USDB chain validator 在 companion RPC 不可用时停止，但 expected USDB chain version 仍只来自本地 chain config。

# 初始激活配置草案

当前 reference artifacts 为开发期状态，不代表 USDB chain public network 激活：

| Owner | Network | Anchor | Active configuration |
| --- | --- | --- | --- |
| BTC registry | `btc-mainnet` | BTC height 0 | UIP-0001 至 UIP-0006 的九个 BTC v1 family，包括 commit protocol 与 balance-history semantics。 |
| BTC registry | `btc-regtest` revision 1 | BTC height 0 | 当前 revision；与 `btc-mainnet` 相同的九个 BTC v1 family，但使用独立 registry artifact 和 ID。 |
| BTC registry | `btc-regtest` revision 2 | BTC height 100000 | staged revision；只增加一个 `Planned` formula marker，因此不改变任何高度的 active set，也不作为 BTC 服务 current revision。 |
| USDB chain config | `usdb-devnet-20260323` | USDB block 0 | 绑定 `btc-regtest` registry ID；`payload_version=1`、`btc_anchor_policy_version=1`、development `btcAnchorMaxAgeBlocks=6650`、`difficulty_policy_version=1`、`reward_rule_version=1`、`coinbase_emission_policy_version=1`、`collaboration_efficiency_policy_version=1`、`price_policy_version=1`；`fee_split_policy_version=0` 使用启动窗口规则，`quote_policy_version=0` 与 `aux_pool_policy_version=0` 表示 disabled。 |

正式 USDB chain testnet/mainnet 的 genesis、chain ID 和具体 activation block 必须在进入 Review / Last Call 前冻结。BTC source-network registry 与 USDB-chain network 发布矩阵必须分别 review，再由 release manifest 关联 artifact identity。

# 机器可读 Artifacts

参考实现使用两个彼此独立的 artifact 类别：

```text
src/btc/usdb-util/activation-registry/btc-mainnet.json
src/btc/usdb-util/activation-registry/btc-regtest.json
src/btc/usdb-util/activation-registry/btc-regtest-revision-2.json
src/btc/usdb-util/release-manifest.json
src/btc/usdb-util/src/bin/generate_go_btc_activation_golden.rs
src/btc/usdb-util/src/bin/generate_go_release_manifest_golden.rs
go-ethereum/internal/usdb/btc_activation_golden.json
go-ethereum/internal/usdb/cross_chain_release_manifest.json
go-ethereum params.ChainConfig.USDB
```

旧 BTC JSON 与 Markdown 表格表达同一组 legacy activation records，继续由 Rust 服务在构建时嵌入二进制。新增 scoped catalog 通过部署冻结的文件和精确 ID 选择；启动、扫块和 historical RPC lookup 共用同一解析与校验实现，不允许临时选择未审议规则。安装支持新 scope 的二进制不会自动切换任何已冻结网络的 scope 或业务版本。

参考 indexer 配置入口是 `usdb.rules_scope`、`usdb.activation_registry_id` 和 `usdb.activation_registry_catalog_file`：legacy 可省略三项，或省略 catalog 文件并 pin 现有 embedded current ID；新 scope 必须三项齐备。配置与持久化绑定、外部 catalog 示例见 [implementation notes](UIP-0008-activation-registry-implementation-notes.md)。

BTC generator 默认继续生成原 legacy golden；显式 `--catalog` 模式用于生成经审议的新 scoped golden，不能据此自动激活新网络。它将每个 catalog revision 的 stable lag、active 高度边界、完整 set、registry
ID、revision/current metadata 和 set ID 确定性展开到 Go golden artifact。Go validator
不在运行时读取 Rust BTC JSON，也不使用它决定 USDB chain activation；它从目标 USDB
block 的 activation checkpoint 取得绑定的 registry ID，按 payload BTC 高度查询内嵌
golden，随后重算 RPC profile 的 set ID，并比较 profile stable lag。USDB chain
expected versions 始终来自同一个本地 checkpoint。release generator 独立生成 audit-only
manifest golden，Go 测试只用它检查发布物漂移，不把它接入 header validation。

# 待审计问题

1. 正式 BTC source networks 的 indexing origin，以及 USDB chain public testnet/mainnet 的 genesis 与激活高度。
2. 后续 current revision 在线迁移、scope 迁移与 stable lag 升级应分别定义可重放规则和恢复边界。
3. cross-chain release manifest 的签名与发布流程。

`aux_pool_policy_version = 0` 必须由 USDB chain config 的完整 version set 显式表示，lookup 不提供隐式 `0` fallback。
