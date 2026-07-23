UIP: UIP-0008
Title: Protocol Versioning and Activation Matrix
Status: Draft
Type: Process / Standards Track
Layer: Process / Consensus / Indexer / Validator
Created: 2026-04-26
Requires: UIP-0000, UIP-0001, UIP-0002, UIP-0003, UIP-0004, UIP-0005, UIP-0006, UIP-0007
Activation: See owner-scoped BTC registries and USDB activation schedules

# 摘要

本文定义 USDB 经济模型相关协议的版本字段、激活矩阵、历史重放规则和 state commit 承诺边界。

UIP-0008 不直接定义新的 pass schema、energy 公式或 USDB chain reward 公式。它定义的是：

- 不同协议版本字段的职责边界。
- 某个网络、某个高度应该使用哪个版本。
- 历史查询和 validator replay 如何按历史高度选择版本。
- `snapshot_id`、`system_state_id`、`local_state_commit` 应如何与激活版本关联。

# 动机

旧实现曾使用类似 `USDB_INDEX_FORMULA_VERSION` 的全局常量。当前参考实现已按共识所有权拆分版本来源：BTC-side 服务按 BTC network 和历史高度查询本地机器可读 registry；USDB chain 节点按自身 genesis / chain config 和 USDB block number 查询版本。正式网络不得依赖代码发布或远程 RPC 来隐式改变本链共识规则。

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
| `btc_registry_revision` | 单个 BTC network registry 的一次完整、不可变快照；包含该 revision 之前的全部历史记录，不是某个协议的版本号。 |
| `activation_registry_id` | 一个 `btc_registry_revision` canonical encoding 的哈希；不得覆盖其他 BTC 网络或 USDB chain config。 |
| `usdb_activation_checkpoint` | `ChainConfig.usdb.activations[]` 中的一项；以 USDB block 为锚点，完整携带全部 USDB version fields 和一个 BTC registry binding。 |
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
    btc_registry_revision = 一个 BTC network 的完整 registry 快照
    btc_activation_record = records[] 中一个 version family 的激活记录
    active_version_set    = 在指定 registry revision / BTC height 下派生出的版本集合

USDB:
    usdb_activation_schedule   = ChainConfig.usdb.activations[]
    usdb_activation_checkpoint = schedule 中一个完整版本快照和 BTC registry binding
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
    versions = { payload=1, difficulty=1, reward=0, ... }

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

进入 USDB block header、chain config、USDB activation schedule 或 reserved system state 的版本字段应该使用固定宽度整数。首个启用版本必须使用正整数版本号，例如 `payload_version = 1`、`difficulty_policy_version = 1`。

首个正式 USDB-chain 网络必须启用 level-based difficulty policy，不定义 `difficulty_policy_version = 0` 作为“未启用”保留值。若未来某个独立测试网络确实需要无 difficulty policy 模式，必须由后续 UIP 单独定义，不得复用正式网络语义。

可选经济组件如果需要 disabled 状态，必须由对应 UIP 明确允许 `0` 的含义。例如 UIP-0015 当前草案允许 `aux_pool_policy_version = 0` 表示辅助算力池未启用；这不代表其他 version family 自动允许 `0`。

# 两类激活记录与所有权

BTC activation records 和 USDB activation checkpoints 由不同的本地共识配置拥有，禁止合并成一个运行时 registry。

## BTC Network Registry

每个 BTC registry 文件只允许描述一个 BTC network。顶层 scope 必须包含：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `network_type` | enum | `mainnet`、`testnet`、`signet` 或 `regtest`，必须与 Bitcoin Core network 对应。 |
| `network_id` | string | 具体 BTC network ID，禁止省略。 |

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
  "schema_version": "uip-0008-btc-activation-registry:v1",
  "scope": {
    "network_type": "regtest",
    "network_id": "btc-regtest"
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

USDB-chain integer version families 必须写入 USDB chain genesis / `ChainConfig.usdb.activations[]`。数组中的每个 USDB activation checkpoint 以 `usdb_block` 为 anchor，并携带该高度完整的 USDB chain version set 与 `btcActivationRegistryId`。Go 类型名 `USDBConsensusActivation` 和 JSON 字段名 `activations[]` 保持不变，但其元素语义是完整 checkpoint，不是单个 version family 的增量记录。USDB chain 节点不得读取 BTC registry 或调用 companion RPC 来决定 expected USDB chain version。

每个 `ChainConfig.usdb.activations[i]` checkpoint 的 `btcActivationRegistryId` 绑定从该 USDB block 起允许引用的 immutable BTC source-network registry revision。该字段是 cross-chain historical profile 的辅助共识条件，不是 USDB-chain version activation source：

- miner/validator 必须在本地 Go golden artifact 中找到该 registry ID，否则 fail closed。
- payload 的 `btc_height` 必须在该 registry 的高度表中解析 expected `active_version_set`。
- companion RPC 返回的 `activation_registry_id + active_version_set + active_version_set_id` 必须与本地 lookup 精确一致。
- 后续 USDB activation checkpoint 可以切换到同一 BTC network catalog 的新 revision；旧 revision 必须继续保留以便历史 replay。
- 已生效 checkpoint 的 registry ID 变更必须进入 `CheckCompatible`，不能由 CLI、RPC 或普通配置热更新覆盖。

# 激活矩阵规则

## BTC Registry 规则

BTC registry 必须遵循：

- 未列出的 BTC network 不得默认激活；缺少对应 BTC registry artifact 时必须 fail closed。
- 单个 BTC registry 文件不得包含其他 BTC network 或任何 USDB-chain family。
- 同一 `version_family`、同一 network、同一高度只能有一个 active version。
- 后激活的版本必须显式 `supersedes` 被替代版本，除非该 family 之前没有 active version。
- `Planned` 记录不得影响 validator、indexer 或 RPC 查询结果。
- `Deferred` 和 `Superseded` 记录只能用于审计和历史说明。
- 同一 BTC network 的 registry revision 必须从 1 连续递增；每个新 revision 必须逐条保留旧 revision 的记录，禁止改写历史。
- catalog 必须显式指定一个 current revision；historical request 可以按 registry ID 读取任何保留 revision。

若两个 active 记录在同一高度冲突，节点必须拒绝启动公开网络服务，不能任选其一。

## USDB Activation Schedule 规则

USDB activation schedule 必须遵循：

- 未配置的 USDB network 不得默认激活；缺少本地 genesis / chain config schedule 时必须 fail closed。
- checkpoint 必须按 `block` 严格递增，同一 USDB block 只能有一个 checkpoint。
- 每个 checkpoint 必须完整携带所有 USDB chain version fields 和 `btcActivationRegistryId`；不得把缺失字段解释为继承上一条。
- 查询目标 USDB block 时，选择 `block <= target` 的最后一个 checkpoint；首次 checkpoint 之前 USDB consensus inactive。
- 单个或多个 USDB policy 在同一 block 变化时，都必须生成一个新的完整 checkpoint。
- 仅切换 BTC registry revision 也必须生成一个新的完整 checkpoint，并重复不变的 USDB version fields。
- 已生效 checkpoint 的 versions 或 registry binding 都属于 chain compatibility 边界；未来 checkpoint 只可在生效前更新。

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

- BTC-side pass、balance、state 和 energy 派生必须先按配置的 BTC network 选择唯一 registry，再使用 `btc_height` 选择版本。
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

- USDB block 的 `payload_version`、`difficulty_policy_version`、`reward_rule_version`、`coinbase_emission_policy_version`、`fee_split_policy_version`、`collaboration_efficiency_policy_version`、`price_policy_version`、`quote_policy_version` 和 `aux_pool_policy_version` 只由 USDB chain genesis / chain config 决定。
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

## Canonical Encoding v1

BTC 机器可读 registry 固定在 `src/btc/usdb-util/activation-registry/<network-id>[-revision-N].json`，当前内嵌 `btc-mainnet.json`、`btc-regtest.json` 和 staged `btc-regtest-revision-2.json`，单 revision schema 为 `uip-0008-btc-activation-registry:v1`。JSON parser 必须拒绝未知字段、重复字段、类型错误、scope 不匹配、USDB-chain family 和同 family/height 的 active 冲突；catalog parser 还必须拒绝 revision 缺口、重复 identity 和历史 record 改写。没有独立 catalog 的网络不得回退到其他网络 registry。

所有 string 使用 `u32 big-endian byte_length || UTF-8 bytes`。所有 integer 使用 `u64 big-endian`。string/integer union 使用 `0x00` / `0x01` tag；optional value 使用 `0x00` absent 或 `0x01 || encoded_value`。hash 输出为 lowercase 64-character hex text。

`activation_registry_id` 的 hash input 为：

1. length-prefixed domain `usdb-btc-activation-registry:v1`。
2. length-prefixed `schema_version`。
3. length-prefixed fixed chain tag `BTC`。
4. length-prefixed scope `network_type`、`network_id` 和 fixed anchor tag `btc_height`。
5. `u32 big-endian record_count`。
6. canonical-sorted records。排序 key 固定为 `(version_family, activation_height, status, uip, version_value, supersedes, notes)`；每条 record 的编码字段顺序固定为 `(uip, version_family, version_value, activation_height, status, supersedes, notes)`。

排序中的 enum 顺序固定为：network type `mainnet, testnet, signet, regtest, devnet, local`；status `Planned, Active, Deferred, Superseded`；version family 使用本文 Version Family 表顺序。union value 先按 tag 排序，再按 string byte order 或 unsigned integer order 排序。

`active_version_set_id` 的 hash input 为 length-prefixed domain `usdb-active-version-set:v1`，随后按本文 Version Family 表的固定顺序编码全部 family：每个 family 先编码 length-prefixed canonical name，再编码 presence marker；present 时继续编码 tagged version value。未激活 family 必须显式编码 absent marker，禁止直接跳过。

两种 id 都使用 SHA-256。当前网络作用域 registry golden ID 为：

```text
btc-mainnet = bb751626eb1415bbc349e77f58cb412908584842cbf7d786262b7bd1f6a7d39e
btc-regtest revision 1 (current) = 22d820e6ec242b61f63473f279c41a4103af5cff13206b1925fd415cceaaf83d
btc-regtest revision 2 (staged)  = 25a39e8022e8351a40f59736b86cf81321c08042121cdb74b85a8f3918a2b973
```

两个网络当前激活相同的 BTC v1 九 family，因此跨实现 golden `active_version_set_id` 相同：

```text
01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691
```

UIP-0006 state view 必须返回 `activation_registry_id`、完整 `active_version_set` 和 `active_version_set_id`。`snapshot_id` 只表示 upstream balance-history state，不包含 USDB formula；`local_state_commit` 必须绑定目标高度的 `active_version_set_id`。

## Cross-chain Release Manifest

`src/btc/usdb-util/release-manifest.json` 的 v2 schema 可以在发布时关联：

- 每个 BTC network catalog 的 revision、current marker、artifact 与 `activation_registry_id`。
- USDB network 的 `chain_id`、genesis hash、chain-config source，以及按 USDB block 排序的 BTC registry bindings。

manifest 仅用于 release review、部署审计和 CI 一致性检查。它不得参与 BTC registry ID、BTC `active_version_set_id`、USDB chain header validation 或 USDB chain expected version lookup；修改一个网络的配置不得改变另一个网络的 runtime activation identity。

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
- registry scope 与配置 network 不匹配时 fail closed。
- BTC registry 拒绝 USDB-chain version family。
- conflicting BTC activation records fail closed。
- historical RPC 按目标高度选择版本。
- reorg 跨激活高度后重新选择版本。
- `active_version_set_id` mismatch。
- chain config 绑定未知或错误的 `activation_registry_id` 时 fail closed。
- registry catalog 拒绝 revision 缺口、多个 current、旧 record 改写和历史 active record 插入。
- Rust generator `--check` 可以证明提交的 Go golden artifact 与全部 BTC registry revisions 完全一致。
- validator 按 payload BTC 高度选择 expected set，并按 `energy/effective-energy/level` version 分派公式；未知版本 fail closed。
- `prev` 跨版本继承测试。
- USDB chain `difficulty_policy_version` mismatch。
- release manifest 中的 BTC registry ID 可由 artifact 重算。
- USDB chain validator 在 companion RPC 不可用时停止，但 expected USDB chain version 仍只来自本地 chain config。

# 初始激活配置草案

当前 reference artifacts 为开发期状态，不代表 USDB chain public network 激活：

| Owner | Network | Anchor | Active configuration |
| --- | --- | --- | --- |
| BTC registry | `btc-mainnet` | BTC height 0 | UIP-0001 至 UIP-0006 的九个 BTC v1 family，包括 commit protocol 与 balance-history semantics。 |
| BTC registry | `btc-regtest` revision 1 | BTC height 0 | 当前 revision；与 `btc-mainnet` 相同的九个 BTC v1 family，但使用独立 registry artifact 和 ID。 |
| BTC registry | `btc-regtest` revision 2 | BTC height 100000 | staged revision；只增加一个 `Planned` formula marker，因此不改变任何高度的 active set，也不作为 BTC 服务 current revision。 |
| USDB chain config | `usdb-devnet-20260323` | USDB block 0 | 绑定 `btc-regtest` registry ID；`payload_version=1`、`difficulty_policy_version=1`；尚未实现的 reward/coinbase/fee/collaboration/price/quote policy 为 development staging `0`，`aux_pool_policy_version=0` 表示 disabled。 |

正式 USDB chain testnet/mainnet 的 genesis、chain ID 和具体 activation block 必须在进入 Review / Last Call 前冻结。BTC source-network registry 与 USDB-chain network 发布矩阵必须分别 review，再由 release manifest 关联 artifact identity。

# 机器可读 Artifacts

参考实现使用两个彼此独立的 artifact 类别：

```text
src/btc/usdb-util/activation-registry/btc-mainnet.json
src/btc/usdb-util/activation-registry/btc-regtest.json
src/btc/usdb-util/activation-registry/btc-regtest-revision-2.json
src/btc/usdb-util/release-manifest.json
src/btc/usdb-util/src/bin/generate_go_btc_activation_golden.rs
go-ethereum/internal/usdb/btc_activation_golden.json
go-ethereum params.ChainConfig.USDB
```

BTC JSON 与 Markdown 表格表达同一组 BTC-side activation records，并由 Rust 服务在构建时嵌入二进制。启动、扫块和 historical RPC lookup 共用同一 network-scoped 解析与校验实现，不允许 runtime flag 覆盖。

Rust generator 将每个 catalog revision 的 active 高度边界、完整 set、registry ID、revision/current metadata 和 set ID 确定性展开到 Go golden artifact。Go validator 不在运行时读取 Rust BTC JSON，也不使用它决定 USDB chain activation；它从目标 USDB block 的 activation checkpoint 取得绑定的 registry ID，按 payload BTC 高度查询内嵌 golden，随后重算 RPC profile 的 set ID 并精确比较。USDB chain expected versions 始终来自同一个本地 checkpoint。

# 待审计问题

1. 正式 BTC source networks 的 indexing origin，以及 USDB chain public testnet/mainnet 的 genesis 与激活高度。
2. 后续 v2 是否继续在本 UIP 扩展 canonical schema，或拆分独立 registry-format UIP。
3. cross-chain release manifest 的签名与发布流程。

`aux_pool_policy_version = 0` 必须由 USDB chain config 的完整 version set 显式表示，lookup 不提供隐式 `0` fallback。
