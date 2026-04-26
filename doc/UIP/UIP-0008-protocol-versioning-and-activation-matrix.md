UIP: UIP-0008
Title: Protocol Versioning and Activation Matrix
Status: Draft
Type: Process / Standards Track
Layer: Process / Consensus / Indexer / Validator
Created: 2026-04-26
Requires: UIP-0000, UIP-0001, UIP-0002, UIP-0003, UIP-0004, UIP-0005, UIP-0006, UIP-0007
Activation: See network activation matrix

# 摘要

本文定义 USDB 经济模型相关协议的版本字段、激活矩阵、历史重放规则和 state commit 承诺边界。

UIP-0008 不直接定义新的 pass schema、energy 公式或 ETHW reward 公式。它定义的是：

- 不同协议版本字段的职责边界。
- 某个网络、某个高度应该使用哪个版本。
- 历史查询和 validator replay 如何按历史高度选择版本。
- `snapshot_id`、`system_state_id`、`local_state_commit` 应如何与激活版本关联。

# 动机

当前代码中存在类似 `USDB_INDEX_FORMULA_VERSION` 的全局常量。开发阶段这可以工作，但正式网络不能依赖代码发布来隐式改变共识结果。

影响共识或经济结果的变更必须满足：

- 新版本必须有显式版本号。
- 新版本必须有网络化激活规则。
- 历史高度必须按当时激活的版本重放。
- BTC 侧状态派生、USDB state view 和 ETHW validator 校验不能各自使用不同版本。

# 非目标

本文不定义：

- 具体 pass inscription schema。
- 具体 pass 状态机。
- 具体 energy / effective energy / level 公式。
- 具体 ETHW reward、difficulty、CoinBase、price 或分账公式。
- 主网最终激活高度。

这些内容由对应 UIP 定义。本文只定义版本和激活机制。

# 术语

| 术语 | 含义 |
| --- | --- |
| `activation_record` | 描述某个版本在某链、某网络、某锚点生效的记录。 |
| `activation_matrix` | 一组 `activation_record`。 |
| `activation_registry_id` | 激活矩阵 canonical encoding 的哈希，用于审计节点使用的激活配置。 |
| `active_version_set` | 在指定 chain context 下生效的一组版本字段和值。 |
| `active_version_set_id` | `active_version_set` canonical encoding 的哈希。 |
| `version_family` | 一类版本字段，例如 `energy_formula_version`、`payload_version`。 |
| `chain_context` | 进行版本选择所需的链、网络和高度信息。 |

# 激活机制概念

激活机制用于回答一个问题：在某条链、某个网络、某个高度，系统应该使用哪一组协议规则。

它不是代码发布机制，也不是运行时开关。代码可以同时支持多个版本，但只有 activation matrix 中已经对目标网络和目标高度生效的版本，才可以用于共识、历史查询和 validator replay。

核心关系如下：

```text
activation_record  = 激活配置的一行
activation_matrix  = 多行 activation_record
active_version_set = 在某个链/网络/高度查出来的当前生效版本集合
```

例如：

```text
activation_matrix:
    BTC btc-regtest btc_height >= 0 -> energy_formula_version = uip-0003-pass-energy-formula:v1
    BTC btc-regtest btc_height >= 0 -> level_formula_version = uip-0005-level-and-real-difficulty:v1

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

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 版本族

不同版本字段有不同职责。实现不得把所有变更合并成一个全局版本号。

| Version Family | 类型 | 主要链路 | 说明 |
| --- | --- | --- | --- |
| `inscription_schema_version` | string | BTC | pass 铭文 JSON schema 和字段解释。 |
| `pass_state_machine_version` | string | BTC | pass 状态转移、terminal state、remint / consume 语义。 |
| `energy_formula_version` | string | BTC / USDB indexer | raw energy、penalty、inheritance、settlement 公式。 |
| `effective_energy_formula_version` | string | BTC / USDB indexer | collab contribution、Leader effective energy 聚合规则。 |
| `level_formula_version` | string | BTC / USDB indexer / ETHW validator | `effective_energy -> level -> difficulty_factor_bps` 规则。 |
| `query_semantics_version` | string | RPC / indexer | historical query、pagination、projection、exact / at_or_before 语义。 |
| `state_view_version` | string | RPC / validator replay | UIP-0006 state view JSON 结构版本。 |
| `payload_version` | uint8 | ETHW header | UIP-0007 `ProfileSelectorPayload` binary layout。 |
| `difficulty_policy_version` | uint16 | ETHW header / chain config | `level -> real difficulty` 共识算法版本。 |
| `reward_rule_version` | uint16 | ETHW chain config | reward、bonus、fee split 或发放规则版本。 |
| `commit_protocol_version` | string | USDB local state | `local_state_commit` / `system_state_id` 输入与编码规则。 |
| `balance_history_semantics_version` | string | balance-history | upstream balance snapshot / UTXO query 语义。 |

字符串版本建议使用：

```text
uip-0003-pass-energy-formula:v1
uip-0004-collab-leader-effective-energy:v1
uip-0006-usdb-economic-state-view:v1
```

进入 ETHW block header 或 chain config 的版本字段应该使用固定宽度整数。首个正式版本必须使用正整数版本号，例如 `payload_version = 1`、`difficulty_policy_version = 1`。

USDB 首个正式 ETHW 网络必须启用 level-based difficulty policy，不定义 `difficulty_policy_version = 0` 作为“未启用”保留值。若未来某个独立测试网络确实需要无 difficulty policy 模式，必须由后续 UIP 单独定义，不得复用正式网络语义。

# 激活记录

每条激活记录必须至少包含：

| 字段 | 类型 | 说明 |
| --- | --- | --- |
| `uip` | string | 例如 `UIP-0003`。 |
| `version_family` | string | 例如 `energy_formula_version`。 |
| `version_value` | string / integer | 该 family 的具体版本值。 |
| `chain` | enum | `BTC`、`ETHW` 或 `CrossChain`。 |
| `network_type` | enum | `mainnet`、`testnet`、`signet`、`regtest`、`devnet`、`local`。 |
| `network_id` | string | 具体网络 ID，禁止省略。 |
| `activation_anchor` | enum | `btc_height`、`ethw_block`、`governance`、`manual` 或 `hybrid`。 |
| `activation_value` | string / integer | 激活高度、治理决议编号或 dev/local 手动标识。 |
| `status` | enum | `Planned`、`Active`、`Deferred`、`Superseded`。 |
| `supersedes` | optional | 被替代的 version value。 |
| `notes` | string | 必要说明。 |

示例：

```json
{
  "uip": "UIP-0003",
  "version_family": "energy_formula_version",
  "version_value": "uip-0003-pass-energy-formula:v1",
  "chain": "BTC",
  "network_type": "regtest",
  "network_id": "btc-regtest",
  "activation_anchor": "btc_height",
  "activation_value": 0,
  "status": "Active",
  "supersedes": null,
  "notes": "Development network activates v1 from genesis after implementation."
}
```

# 激活矩阵规则

激活矩阵必须遵循：

- 未列出的网络不得默认激活。
- public mainnet / testnet 不得使用 `manual` 激活。
- 同一 `version_family`、同一 chain、同一 network、同一高度只能有一个 active version。
- 后激活的版本必须显式 `supersedes` 被替代版本，除非该 family 之前没有 active version。
- `Planned` 记录不得影响 validator、indexer 或 RPC 查询结果。
- `Deferred` 和 `Superseded` 记录只能用于审计和历史说明。

若两个 active 记录在同一高度冲突，节点必须拒绝启动公开网络服务，不能任选其一。

# Version Lookup

实现必须在每次历史查询或 validator replay 时按 context 查询版本，而不是读取全局常量。

输入：

```text
chain_context =
    chain
    network_type
    network_id
    btc_height?
    ethw_block?
    governance_state?
```

输出：

```text
active_version_set =
    inscription_schema_version?
    pass_state_machine_version?
    energy_formula_version?
    effective_energy_formula_version?
    level_formula_version?
    query_semantics_version?
    state_view_version?
    payload_version?
    difficulty_policy_version?
    reward_rule_version?
    commit_protocol_version?
    balance_history_semantics_version?
```

规则：

- BTC-side pass、balance、state 和 energy 派生必须使用 `btc_height` 选择版本。
- ETHW-side payload、difficulty、reward 和执行规则必须使用 `ethw_block` 或 governance state 选择版本。
- CrossChain 规则必须明确主锚点和辅助条件。
- 查询历史 BTC 高度时，禁止用当前 BTC head 的版本解释旧高度。
- 校验历史 ETHW block 时，禁止用当前 ETHW head 的版本解释旧块。

# CrossChain 激活

跨链规则必须写明主锚点。

推荐语义：

```text
active_if =
    primary_anchor_condition
    AND all_auxiliary_conditions
```

例如 ETHW reward rule 以 `ethw_block` 为主锚点，但它引用的 USDB profile 必须按 payload 中的 `btc_height` 使用 BTC-side active version set 解析。

这意味着：

- ETHW block 的 `payload_version`、`difficulty_policy_version`、`reward_rule_version` 由 ETHW chain config / activation matrix 决定。
- payload 指向的 BTC / USDB state 由 `btc_height` 对应的 BTC-side activation matrix 决定。
- 两者都必须可重放，且不得互相覆盖。

# State Commit 绑定

`snapshot_id` 是 upstream balance-history 的 state identity，不应该承诺 USDB indexer 的 energy 或 pass 公式。

USDB 的 `system_state_id` / `local_state_commit` 必须绑定足够信息，使 validator 和审计工具能够发现版本不一致。至少应绑定：

- upstream `snapshot_id`。
- `commit_protocol_version`。
- 当前 context 下的 `active_version_set_id`。
- 影响派生状态的输入数据 commit。

`local_state_commit` 不需要直接包含完整 `active_version_set`。它只需要承诺 `active_version_set_id`，前提是节点、validator 和审计工具可以通过稳定的 activation registry 查询到该 id 对应的完整 version set。

推荐定义：

```text
activation_registry_id = sha256(canonical_activation_records)
active_version_set_id  = sha256(canonical_active_version_set)
local_state_commit     = hash(commit_protocol_version, snapshot_id, active_version_set_id, derived_state_root)
system_state_id        = hash(snapshot_id, local_state_commit)
```

`activation_registry_id` 和 `active_version_set_id` 的 canonical encoding 当前保持 TODO。可以先在实现 UIP 或机器可读 activation registry 中固定；稳定后再更新本文或新增专门 UIP。canonical encoding 固定前，UIP-0006 state view 至少必须返回人类可读的 `usdb_index_protocol_version`、`usdb_index_formula_version` 和相关 view/query version 字段。

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
- local override 不得写入 public network activation matrix。
- 开发期数据迁移不构成主网兼容承诺。

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
| `PAYLOAD_VERSION_MISMATCH` | ETHW header payload version 不匹配。 |
| `DIFFICULTY_POLICY_VERSION_MISMATCH` | ETHW payload 声明的 difficulty policy version 与 expected version 不一致。 |
| `COMMIT_PROTOCOL_VERSION_MISMATCH` | local state commit 编码版本不匹配。 |

# Backwards Compatibility

当前 USDB 项目仍处于开发阶段。尚未在公开主网激活的旧实现行为属于 pre-standard implementation draft，不需要作为长期兼容版本保留。

一旦某个 public network 进入 `Active`：

- 后续变更必须新增 version 或 activation record。
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
- `/home/bucky/work/go-ethereum` 的 ETHW chain config、payload verifier 和 miner payload generation。

# 测试要求

至少需要覆盖：

- 不同 BTC height 返回不同 `energy_formula_version`。
- 激活高度前、激活高度、激活高度后行为。
- 未列出网络不激活。
- local/regtest height 0 激活。
- public network 禁止 `manual` 激活。
- conflicting activation records fail closed。
- historical RPC 按目标高度选择版本。
- reorg 跨激活高度后重新选择版本。
- `active_version_set_id` mismatch。
- `prev` 跨版本继承测试。
- ETHW `difficulty_policy_version` mismatch。

# 初始激活矩阵草案

以下只作为 v1 开发期草案，不代表 public network 激活：

| UIP | Version Family | Version Value | Chain | Network Type | Network ID | Anchor | Value | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| UIP-0001 | `inscription_schema_version` | `uip-0001-miner-pass-inscription:v1` | BTC | regtest | btc-regtest | btc_height | 0 | Planned | 实现完成后可在 regtest 从 genesis 重建。 |
| UIP-0002 | `pass_state_machine_version` | `uip-0002-pass-state-machine:v1` | BTC | regtest | btc-regtest | btc_height | 0 | Planned | pass 状态机 v1。 |
| UIP-0003 | `energy_formula_version` | `uip-0003-pass-energy-formula:v1` | BTC | regtest | btc-regtest | btc_height | 0 | Planned | raw energy v1。 |
| UIP-0004 | `effective_energy_formula_version` | `uip-0004-collab-leader-effective-energy:v1` | BTC | regtest | btc-regtest | btc_height | 0 | Planned | effective energy v1。 |
| UIP-0005 | `level_formula_version` | `uip-0005-level-and-real-difficulty:v1` | BTC | regtest | btc-regtest | btc_height | 0 | Planned | level / factor v1。 |
| UIP-0006 | `state_view_version` | `uip-0006-usdb-economic-state-view:v1` | BTC | regtest | btc-regtest | btc_height | 0 | Planned | economic state view v1。 |
| UIP-0007 | `payload_version` | `1` | ETHW | devnet | ethw-devnet-TODO | ethw_block | 0 | Planned | ProfileSelectorPayload 107 bytes。 |
| UIP-0007 | `difficulty_policy_version` | `1` | ETHW | devnet | ethw-devnet-TODO | ethw_block | 0 | Planned | 首个正式 USDB level-based difficulty policy 版本。 |

public testnet / mainnet 激活高度必须在进入 Review / Last Call 前补充。

# 机器可读 Activation Registry

机器可读 activation registry 指可以被节点、测试脚本或审计工具直接解析的激活表，例如：

```text
doc/UIP/activation-matrix.json
doc/UIP/activation-matrix.yaml
```

它和 Markdown 表格表达同一组 activation records，但结构固定，更适合自动测试、节点启动校验和生成 `activation_registry_id`。

机器可读 registry 可以先作为纯文档资产落地，不依赖运行时代码。但如果它要参与 `activation_registry_id`、`active_version_set_id` 或节点启动校验，则必须同时固定 canonical encoding、排序、字段类型、未知字段策略和冲突处理规则。

当前建议：本 UIP 保留机器可读 registry 为 TODO，等实现层开始消费 activation matrix 后，再固定 JSON/YAML schema 和 canonical encoding。

# 待审计问题

1. `activation_registry_id` 和 `active_version_set_id` 的 canonical encoding 是否在实现 UIP 中固定，还是新增专门 UIP 固定。
2. 机器可读 activation registry 是否先作为纯文档资产落地，还是等实现层开始消费后再落地。
