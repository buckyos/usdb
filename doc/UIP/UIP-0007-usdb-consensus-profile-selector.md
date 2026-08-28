UIP: UIP-0007
Title: USDB Consensus Profile Selector
Status: Draft
Type: Standards Track
Layer: USDB Header / Consensus
Created: 2026-04-25
Requires: UIP-0000, UIP-0005, UIP-0006
Activation: USDB chain activation matrix; development chains may activate from genesis

# 摘要

本文定义 USDB chain 在区块头 `header.Extra` 中携带的最小 consensus profile selector。

当前草案定义 `ProfileSelectorPayload`：

- payload 使用固定长度二进制编码。
- payload 写入 `header.Extra`。
- payload 携带历史状态 selector、`pass_id` 和 difficulty policy version commitment。
- payload 携带 `btc_anchor_age_blocks`，使 validator 可以只依赖父块和本地
  chain config 对 BTC anchor 推进关系做确定性校验。
- payload 不直接携带 `energy`、`level`、`reward`、`owner`、`state` 或 collab 明细。
- validator 必须使用 payload 里的 selector 查询 UIP-0006 定义的 USDB Economic State View，并本地重算 reward input。
- future difficulty policy 如果依赖 pass level，也必须复用同一 selector 查询同一份 resolved profile。

# 动机

USDB validator 验证旧块时，不能查询 BTC-side USDB state view 的 current head。旧块必须携带足够信息，使 validator 能回到该块出块时引用的 BTC-side USDB 历史状态。

同时，链上 payload 必须尽量小：

- `header.Extra` 是区块头字段，会进入 PoW seal hash。
- payload 长度影响所有区块。
- 审计字段可以通过 BTC-side USDB state view 查询，不需要全部写入区块头。

因此本文只标准化链上最小 profile selector；完整经济状态和审计字段由 UIP-0006 定义。

# 非目标

本文不定义：

- BTC-side USDB economic state view 的完整 JSON 字段。
- raw energy、effective energy、level、difficulty factor 的公式。
- block reward schedule、fee split、uncle reward 或 dividend 规则。
- `level -> difficulty` 的 future PoW difficulty policy；本文只要求其复用同一 profile selector。
- collab bonus、协作者分润或 price / real_price。

# 术语

| 术语 | 含义 |
| --- | --- |
| USDB miner | 组装并尝试挖出 USDB block、选择本区块 `pass_id` 并写入 `ProfileSelectorPayload` 的出块方。 |
| USDB validator | 按 USDB chain 共识规则验证区块及其 `ProfileSelectorPayload`、历史 pass economic profile、reward 和 difficulty 的验证逻辑或节点。 |
| `ProfileSelectorPayload` | 写入 USDB chain `header.Extra` 的固定二进制 profile selector payload。 |
| `profile_selector` | 用于定位某个 BTC-side USDB historical state 下某张 pass profile 的最小字段集合。 |
| `difficulty_policy_version` | 本区块声明使用的 `level -> difficulty` 算法版本。 |
| `btc_height` | payload 锁定的 BTC 历史高度。 |
| `btc_anchor_age_blocks` | 当前精确 BTC anchor 自首次引用后又被连续复用的 USDB 子块数；首次引用为 `0`。 |
| `btc_anchor_policy_version` | USDB chain config 固定的父子 BTC anchor transition 规则版本。 |
| `btc_anchor_max_age_blocks` | activation checkpoint 固定的同一 BTC anchor 最大 age；不是 wall-clock 时间或本地 BTC tip 距离。 |
| `snapshot_id` | upstream balance-history consensus snapshot id。 |
| `system_state_id` | BTC-side USDB system state id。 |
| `pass_id` | UIP-0001 canonical pass inscription id；在本文 payload 中标识 `selected_pass`。 |
| `selected_pass` | 一个具体 USDB chain 区块显式声明使用的 UIP-0006 `candidate_pass`；不要求等于 `candidate_set_view` 的 `top_ranked_candidate`。 |
| `resolved_profile` | USDB validator 根据 payload 查询 UIP-0006 后得到的 `selected_pass` 经济状态。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# Payload Version

当前 payload version：

```text
payload_version = 1
```

该字段是 1 byte unsigned integer，写入 payload 第 0 字节。

payload 编码或字段集合改变时必须升级 `payload_version`。reward 公式或 difficulty policy 公式改变但 payload 字节布局不变时，应通过 USDB chain config / fork version 管理，不一定升级 `payload_version`。

# Binary Layout

`ProfileSelectorPayload` 固定长度为 111 bytes：

| Offset | Size | 字段 | 类型 | 编码 |
| --- | --- | --- | --- | --- |
| 0 | 1 | `payload_version` | uint8 | 固定为 `1`。 |
| 1 | 2 | `difficulty_policy_version` | uint16 | big-endian。必须匹配 USDB chain config 在该 block height 下的期望值。 |
| 3 | 4 | `btc_height` | uint32 | big-endian。 |
| 7 | 4 | `btc_anchor_age_blocks` | uint32 | big-endian。按父子 transition 唯一派生。 |
| 11 | 32 | `snapshot_id` | bytes32 | 32-byte hex id 的原始字节。 |
| 43 | 32 | `system_state_id` | bytes32 | 32-byte hex id 的原始字节。 |
| 75 | 32 | `pass_txid` | bytes32 | inscription outpoint txid。 |
| 107 | 4 | `pass_index` | uint32 | inscription outpoint index，big-endian。 |

等价结构：

```text
ProfileSelectorPayload =
    uint8 payload_version
    uint16 difficulty_policy_version
    uint32 btc_height
    uint32 btc_anchor_age_blocks
    bytes32 snapshot_id
    bytes32 system_state_id
    bytes32 pass_txid
    uint32 pass_index
```

链外展示 `pass_id` 时必须使用 UIP-0001 定义的 canonical encoding：

```text
pass_id = lowercase_hex(pass_txid) + "i" + decimal(pass_index)
```

# Header Extra 规则

当 USDB-chain reward consensus rule 激活时：

- `header.Extra` 必须正好等于一个 `ProfileSelectorPayload`。
- `len(header.Extra)` 必须等于 `111`。
- 不得在 payload 前后拼接 vanity bytes、JSON、签名或其它 opaque data。
- `payload_version` 不支持时必须拒绝该区块。
- `difficulty_policy_version` 与 USDB chain config 在该 block height 下的期望值不一致时必须拒绝该区块。

legacy / non-USDB header 继续使用 32-byte `MaximumExtraDataSize`。USDB 可以使用独立的
160-byte outer limit 预留后续版本空间，但当前 v1 验证必须按 111-byte 精确长度解析，
不能把剩余 49 bytes 当成任意扩展区。

# stable_block_hash

`ProfileSelectorPayload` 不携带 `stable_block_hash`。

原因：

- `snapshot_id` 已由 USDB / balance-history 的 `ConsensusSnapshotIdentity` 派生，identity 中已经包含 `stable_block_hash`。
- `system_state_id` 又绑定 upstream `snapshot_id` 与 usdb-indexer local state commit。
- 因此 `stable_block_hash` 对链上共识约束是冗余字段，只增加诊断直观性。
- 加入该字段会让当前 111-byte v1 payload 增至 143 bytes，仍低于 160-byte USDB
  outer limit，但该字段继续保持冗余。

验证和审计时需要展示 `stable_block_hash` 的，必须通过 UIP-0006 USDB Economic State View 返回。除非未来发现 `snapshot_id` / `system_state_id` 不能覆盖某类共识安全需求，否则 v2 不应仅为了展示便利加入 `stable_block_hash`。

# 字段语义

## btc_height

`btc_height` 是 validator 构造 `ConsensusQueryContext.requested_height` 的输入。

validator 不得用当前 USDB head 替代该高度。

## btc_anchor_age_blocks

`btc_anchor_age_blocks` 不是 miner 自由选择的 freshness 声明。它由父块 selector、
当前 selector 和目标 USDB block 对应的 activation checkpoint 唯一派生：

- 首个需要 selector 的非 genesis USDB block：必须为 `0`。
- 父块尚未激活 USDB selector、当前块为首个 activation block：必须为 `0`。
- `child.btc_height > parent.btc_height`：必须为 `0`。
- `child.btc_height == parent.btc_height`：只有
  `snapshot_id` 与 `system_state_id` 都和父块完全一致时才允许继续，并且必须等于
  `parent.btc_anchor_age_blocks + 1`。
- `child.btc_height < parent.btc_height`：必须拒绝。
- 派生值大于 activation checkpoint 的 `btc_anchor_max_age_blocks`：必须拒绝。
- counter overflow：必须拒绝。

同高度时不比较 `pass_id`；矿工可以在同一 BTC-side state 中选择另一张合法 candidate
pass。`difficulty_policy_version` 仍按当前 USDB block 的 activation checkpoint 校验，
不属于 BTC anchor identity。

## snapshot_id

`snapshot_id` 锁定 upstream balance-history consensus snapshot。

validator 必须把它放入 expected state，并要求 USDB 返回同一 snapshot。

## system_state_id

`system_state_id` 锁定 usdb-indexer 暴露给下游链的顶层系统状态。

validator 必须把它放入 expected state，并要求 USDB 返回同一 system state。

## pass_id

`pass_id` 是本块声明使用的 miner pass。

本文将它称为 `selected_pass`。`selected_pass` 必须在 payload 指定的 historical state 下满足 UIP-0006 `candidate_pass` 条件，即 `state = Active` 且 `pass_kind = standard`。它不需要是 `candidate_set_view` 的 `top_ranked_candidate`；UIP-0006 ordering contract 不是 USDB block-selection policy。

v1 必须显式携带 `pass_id`，不得通过 `coinbase`、`usdb_main` 或其它地址字段隐式反查。原因是：

- 当前 USDB 稳定查询主键是 pass id / inscription id。
- 一个 USDB-chain account address 不一定唯一映射到一张 pass。
- 后续 `candidate_set_view` 或多 pass 场景需要避免隐式选择歧义。

上述限制约束的是区块 payload 与 validator replay，不禁止 miner 在组块前使用稳定的
`usdb_main` 作为本地运维身份。miner 可以调用 UIP-0006
`resolve_miner_candidate(usdb_main, context)`，在一个冻结 `external_state` 下按
`effective_energy DESC, pass_id ASC` 原子选出具体 Active Standard pass；builder 随后必须把
返回的具体 `pass_id` 写入本块 payload，并再次校验 profile 的 `usdb_main` 等于本地 miner
address。validator 仍只按 payload 的 `pass_id` 查询和验证，不调用地址选择接口。

这允许同一 `usdb_main` 的旧 pass 被 consume 后自动跟随 remint，也允许 same-height reorg
在新 state identity 下重新选择；旧 context 必须被拒绝。如果 remint 改用了另一个
`usdb_main`，原矿工配置不得自动跟随，必须停止组块并由运维显式更新地址。

# BTC Anchor 推进与新鲜度边界

`btc_anchor_policy_version = 1` 定义 `btc_anchor_age_blocks` 的 bounded-reuse 规则。
该规则解决以下问题：

- 旧 BTC 高度不能在 USDB child chain 中回退。
- 同一精确 BTC anchor 不能无限期重复引用。
- 同高度 BTC replacement 不能在一个既有 USDB parent 后静默切换 identity。
- miner builder 和 validator 使用同一个只依赖 header parent / child 与 chain config 的
  状态机，不能由 CLI 或 companion RPC 改写。

该规则不证明 payload 离真实 BTC tip 有多近。validator 不得使用“本机当前 bitcoind
tip”作为共识 freshness 条件，否则同一区块会因验证时间、节点同步进度或 RPC 视图不同
而得到不同结果。恶意矿工仍可能从不低于父块的历史 BTC 高度缓慢推进；v1 只把每个精确
anchor 的连续复用窗口限制为一个 activation-bound USDB block count。

运行节点可以用本地 BTC tip 计算 soft lag 并告警、停止本地 mining 或触发运维响应，
但该值不得参与 `VerifyHeader`。未来若引入 BTC header chain / SPV proof，应激活新的
`btc_anchor_policy_version`，由共识可验证的 BTC tip 定义 `max_anchor_lag`。完整 SPV
proof 不应直接塞入 `header.Extra`；当前 160-byte outer limit 只足以容纳小型 commitment，
不能容纳无界 proof。

BTC source network registry 另行固定 `stable_lag_blocks`。当前 draft mainnet/regtest
registry v2 均取 `5`，balance-history 只能索引并暴露
`observed_btc_tip - stable_lag_blocks` 及更早的 snapshot，运行参数不得覆盖。该规则使
深度不超过 5 个 BTC blocks、且尚未越过 stable frontier 的普通 reorg 不进入新生成的
economic state；它是 reorg 风险缓冲，不是 validator 可证明的 tip freshness。

UIP-0006 profile 必须返回 `external_state.stable_lag`。validator 必须将其与目标 USDB
activation 绑定的 BTC registry revision 中 `stable_lag_blocks` 精确比较，但不得把本机
bitcoind 当前 tip 加入比较。当前 v2 把 `stable_lag_blocks` 作为 network scope 的
不可变字段，同一 catalog 的普通 revision 不得修改。public network 冻结前可以重写
draft artifact 并重生成全部 registry/config/release identity；冻结后若需要调整，必须
引入显式 versioned lag 语义或新网络，不能只依靠普通 registry revision、USDB
activation checkpoint 或节点配置热修改。

# BTC Reorg 处理边界

`snapshot_id` 已承诺 `stable_block_hash`，因此同高度 BTC replacement 会改变
`snapshot_id`；无需在 v1 payload 重复携带 block hash。

bounded-reuse 本身不提供 BTC finality，也不解决深层 BTC reorg 后 orphan snapshot 的
长期历史可用性。当前 reference implementation 对已被 BTC canonical history 替换且
companion service 无法重放的 selector fail closed。现有节点不得把“已经导入过该
USDB block”当成绕过 fresh validator replay 的依据。当前 Go 节点尚未实现运行中
自动检测并 rewind 已导入的 orphan selector；现有 same-height replacement E2E 只证明
fresh validator 会拒绝。这是 public-network reorg policy 的实现缺口，不得标记为已完成。

正式 public network 必须在发布前冻结以下二选一的可执行运维/协议边界：

1. BTC-side archive 可按 committed snapshot identity 永久重放 orphan state，使既有
   USDB block 保持可验证；或
2. 深层 BTC reorg 触发 USDB chain 回退到最后一个仍可验证的 selector，并有确定的
   detection、rewind、restart/joiner 流程。

阈值 signer/publisher 不属于 v1 必选组件。future SPV/header-chain policy 仍保留为
去信任化升级方向。在上述 public-network reorg policy 未完成 live E2E 前，
`btc_anchor_policy_version = 1` 只能视为 bounded stale-replay guard，不能宣称提供完整
BTC finality。

当前 `stable_lag_blocks = 5` 仅降低上述缺口被普通短 reorg 触发的概率。深度超过 stable
frontier 的 reorg 仍可能使已提交 selector 指向 orphan snapshot，因此不能据此关闭
archive/rewind 的后续设计项；public testnet/mainnet 发布前仍需按实测 BTC 风险和运维
目标复核该值。

# Validator Replay

validator 必须按以下顺序验证：

1. 从 `header.Extra` 解析 `ProfileSelectorPayload`。
2. 校验 payload version 和固定长度。
3. 从目标 USDB block 的 activation checkpoint 读取
   `btc_anchor_policy_version / btc_anchor_max_age_blocks`，并在任何 companion RPC
   之前校验父子 BTC anchor transition。
4. 使用 `btc_height`、`snapshot_id`、`system_state_id` 构造 UIP-0006 `query_context` 和 `expected_state`。
5. 使用 `pass_id` 查询 UIP-0006 定义的 pass economic profile，或使用等价的历史 `get_pass_snapshot` / `get_pass_energy` RPC 组合。
6. 确认 resolved profile 对应的 `selected_pass` 满足 UIP-0006 `candidate_pass` 条件。
7. 按 USDB chain reward rule version 从 resolved profile 重算 reward input。
8. 如果 future USDB chain difficulty policy 已激活，并且该 policy 依赖 USDB level，则使用同一个 resolved profile 重算本块应有 difficulty。
9. 在 `Finalize` / state transition 中使用重算 reward 结果发放奖励。

任一步失败都必须 fail-closed。validator 不得因为 USDB 不可用、历史不可用或 mismatch 而继续接受新区块。

# Payload 不携带的字段

`ProfileSelectorPayload` 禁止直接携带：

- `energy`
- `level`
- `reward`
- `owner`
- `state`
- `pass_kind`
- `collab_contribution`
- `effective_energy`
- `difficulty_factor_bps`
- `collab_breakdown`
- `base_difficulty`
- `real_difficulty`

这些字段必须通过 UIP-0006 state view 或 USDB chain 本地 policy 在验证时重算。

# 与 UIP-0006 的关系

本文是 USDB chain payload 规范。UIP-0006 是 BTC-side USDB state view 规范。

关系如下：

```text
ProfileSelectorPayload(header.Extra)
    -> difficulty_policy_version
    -> btc_height
    -> snapshot_id
    -> system_state_id
    -> pass_id
        |
        v
USDB Economic State View(UIP-0006)
        |
        v
USDB chain reward rule / future difficulty rule
```

因此，`ProfileSelectorPayload` 是 UIP-0006 可查询状态的最小 selector，而不是 UIP-0006 JSON profile 的链上序列化。

# Reward 与 Difficulty 共享 Selector

`ProfileSelectorPayload` 中定位 historical profile 的 selector 是：

```text
btc_height + snapshot_id + system_state_id + pass_id
```

`btc_anchor_age_blocks` 不参与 UIP-0006 查询主键；它只承诺该 historical selector
相对 USDB parent 的推进关系。

reward rule 和 future difficulty policy 必须消费同一个 selector 得到的同一份 `resolved_profile`。不得定义第二套独立 difficulty payload 来携带另一组 `{btc_height, snapshot_id, system_state_id, pass_id}`。

原因：

- 独立 difficulty payload 会引入 reward 使用 pass A、difficulty 使用 pass B 的歧义。
- 同一区块的 reward、difficulty 和审计视图应引用同一张 miner pass。
- `header.Extra` 字节空间有限，重复 selector 没有必要。

如果 future difficulty policy 需要额外参数，应该优先放入 USDB chain config / difficulty policy version，而不是在 header 中复制第二套 USDB selector。

# Miner Payload Generation

miner 生成新区块时应该：

1. 从本地 BTC-side `usdb-indexer` service 获取 current system state。
2. 使用配置的 `pass_id` 在该 state 下确认 pass 可查询。
3. 从 USDB chain config 读取待挖 USDB block number 对应的 expected
   `difficulty_policy_version / btc_anchor_policy_version / btc_anchor_max_age_blocks`。
4. 使用待挖块的 parent selector 派生唯一 `btc_anchor_age_blocks`；无法推进时停止组块。
5. 将 `difficulty_policy_version`、`btc_height`、`btc_anchor_age_blocks`、
   `snapshot_id`、`system_state_id` 和 `pass_id` 编码成 `ProfileSelectorPayload`。
6. 写入待挖区块的 `header.Extra`。

miner 不能正确构造 payload 时，不应继续挖 USDB reward-enabled 区块。

# Versioning

本文区分以下版本：

| 版本 | 位置 | 作用 |
| --- | --- | --- |
| `payload_version` | `header.Extra` 第 0 字节 | 描述 payload 字节布局。 |
| `difficulty_policy_version` | `header.Extra` 第 1-2 字节；期望值来自 USDB chain config / fork policy | 描述 `level -> difficulty` 公式和校验规则。 |
| `btc_anchor_policy_version` | USDB chain config activation checkpoint | 描述父子 BTC anchor transition；不由 payload 或 RPC 选择。 |
| `reward_rule_version` | USDB chain config / fork policy | 描述 reward 公式和奖励发放规则。 |

如果未来只改变 reward multiplier、base reward、collab bonus 或 difficulty policy 公式，但 `ProfileSelectorPayload` 字节布局不变，不应强制升级 `payload_version`。

`difficulty_policy_version` 进入 payload 不是为了允许 miner 选择算法，而是为了让区块头显式承诺其声明的 difficulty policy。validator 必须用 USDB chain config / fork policy 计算该 block height 下的 expected `difficulty_policy_version`，并要求 payload 中的值完全一致。

如果未来确实需要在 header 中新增 selector 字段，则必须定义新的 payload version。仅为展示 `stable_block_hash`、列出 collab pass、或配置 difficulty policy 参数，不应升级 header payload；这些信息应优先来自 UIP-0006 state view 或 UIP-0009 chain config。

# 与 Difficulty 的边界

当前 v1 payload 不直接定义 `level -> difficulty`。

如果后续 USDB chain policy 引入 `level` 影响 PoW difficulty，应复用相同 selector 解析 UIP-0006 profile，再由新的 USDB chain difficulty policy 决定：

- 是否仍使用 `ProfileSelectorPayload`。
- 是否升级 payload version。
- `base_difficulty` 是否来自 header / parent context。
- `real_difficulty` 是否需要显式承诺。

由于 difficulty 规则可能独立于 reward 规则演进，USDB chain config 应定义独立的 `difficulty_policy_version` 激活规则。该版本字段同时进入 `header.Extra` 作为显式承诺，但 validator 必须以 chain config 的 expected version 为准，不得让 payload 中的值覆盖本地共识配置。

# 与 Collab Bonus 的边界

collab bonus 若进入 USDB chain reward rule，不得要求每个区块在 header 中携带 Leader 的完整 `collab_pass_id` 列表。

原因：

- Leader 的协作者数量可能很大。
- 把所有 collab pass id 放入区块会导致 header 或 block body 大小不可控。
- collab contribution 和 breakdown 已由 UIP-0006 在确定历史 context 下提供。

因此，v1 payload 仍只携带 Leader `pass_id`。collab bonus 的 aggregate input 应通过 UIP-0006 profile 中的 `collab_contribution`、`effective_energy` 或后续明确的 bonus 字段重算。如果未来需要给协作者直接分润，应通过 UIP-0006 `get_collab_breakdown` 可验证查询、单独结算/claim 机制，或后续 reward distribution UIP 定义，而不是把全量 collab list 塞进 `header.Extra`。

# 与 USDB Chain Config 的边界

`payload_version` 只描述 `header.Extra` 字节布局。以下字段和规则不属于本文，必须由 USDB chain config / bootstrap UIP 定义：

- ChainID / NetworkId。
- genesis 和 PoW 基础参数。
- USDB reward consensus 是否启用。
- active `payload_version`。
- active `btc_anchor_policy_version` 和正数 `btc_anchor_max_age_blocks`。
- `reward_rule_version`。
- expected `difficulty_policy_version` 及其激活高度。
- 这些版本从 genesis 生效还是在后续 fork 高度生效。

UIP-0008 负责通用版本激活矩阵。USDB chain 具体 chain config、genesis、USDB reward/difficulty policy version 字段应由单独的 USDB Chain Config UIP 定义。

# 错误语义

实现至少需要区分：

| 错误 | 触发条件 |
| --- | --- |
| `MISSING_USDB_PROFILE_SELECTOR` | `header.Extra` 为空或未携带 profile selector。 |
| `PAYLOAD_SIZE_MISMATCH` | `len(header.Extra) != 111`。 |
| `PAYLOAD_VERSION_MISMATCH` | 不支持的 `payload_version`。 |
| `DIFFICULTY_POLICY_VERSION_MISMATCH` | payload `difficulty_policy_version` 与 chain config expected version 不一致。 |
| `BTC_ANCHOR_POLICY_VERSION_UNSUPPORTED` | activation checkpoint 指定了本实现不支持的 anchor policy。 |
| `BTC_ANCHOR_HEIGHT_REGRESSION` | child `btc_height` 小于 parent。 |
| `BTC_ANCHOR_IDENTITY_MISMATCH` | 同高度 child 的 `snapshot_id/system_state_id` 与 parent 不同。 |
| `BTC_ANCHOR_AGE_MISMATCH` | child age 不是父子 transition 唯一派生值。 |
| `BTC_ANCHOR_AGE_EXCEEDED` | child age 超过 activation checkpoint 上限或 counter overflow。 |
| `SNAPSHOT_ID_MISMATCH` | USDB historical state 与 payload `snapshot_id` 不一致。 |
| `SYSTEM_STATE_ID_MISMATCH` | USDB historical state 与 payload `system_state_id` 不一致。 |
| `PASS_NOT_FOUND` | `pass_id` 在该 historical context 下不存在。 |
| `HISTORY_NOT_AVAILABLE` | USDB 无法重放目标历史 context。 |
| `STATE_NOT_RETAINED` | USDB 已不保留目标历史 state。 |
| `REWARD_INPUT_INVALID` | resolved profile 不满足当前 reward rule。 |

# 测试要求

实现 UIP-0007 时，至少需要覆盖：

- `ProfileSelectorPayload` binary roundtrip。
- invalid version。
- invalid payload size。
- miner 生成的 `header.Extra` 长度正好为 111。
- 首个 selector 与 BTC 高度前进时 age 为 0；同 anchor 连续块严格 `+1`。
- BTC height regression、同高 identity replacement、age 跳号、counter overflow 和
  `btc_anchor_max_age_blocks` 边界前后。
- `difficulty_policy_version` 与 chain config expected version 不一致时拒绝。
- validator 使用 payload selectors 查询历史 BTC-side USDB state。
- BTC head 前进后，旧 USDB block 仍按旧 payload 验证通过。
- same-height BTC reorg 后，旧 payload 返回 state mismatch。
- 缺少 BTC-side `usdb-indexer` service 时 fail-closed。
- 篡改 `btc_height` / `snapshot_id` / `system_state_id` / `pass_id` 任一字段会导致验证失败。

# 实现迁移注意

- UIP 仍处于 Draft 且开发链不承诺旧数据兼容，因此当前 111-byte layout 直接取代早期
  107-byte v1 prototype；实现不保留双栈 parser。public network 一旦冻结 v1，
  后续任何 layout 变化都必须升级 `payload_version`。
- 当前 go-ethereum 实现已经移除旧 `RewardPayloadV1`，统一使用 111-byte
  `ProfileSelectorPayload`，并由 USDB chain config 按待处理 block number 提供 expected
  `payload_version / btc_anchor_policy_version / difficulty_policy_version` 和
  `btc_anchor_max_age_blocks`。
- miner/validator CLI 只保留 companion RPC URL、timeout 和 selected pass 等运行参数；
  是否激活本规则及 expected version 只能由 chain config 决定。
- `VerifyHeader` 先执行不访问 RPC 的固定长度、版本和父子 BTC anchor transition
  校验，再按 selector 查询 historical profile 并重算实际 difficulty；畸形或 stale
  transition 不会进入 RPC 路径。
- miner `Prepare`、validator `VerifyHeader` 与 reward state transition 均消费同一 selector
  解析出的 UIP-0006 profile。development chain 在 UIP-0011 激活前仍使用既有 Ethash
  静态奖励，不再使用旧 level/reward multiplier mock。
- expected version 已由 UIP-0008/UIP-0009 的本地 chain-config activation schedule 按 USDB chain
  block number 查询；历史 replay、same-height replacement、服务不可用、字段篡改和
  miner/validator 交叉校验已有 Go 测试及 live E2E 覆盖。

# 后续实现议题

1. collab bonus 的 aggregate 字段是否直接复用 `effective_energy`，还是定义独立 `collab_bonus_energy` / `collab_bonus_bps`。
2. public testnet/mainnet 的 `btc_anchor_max_age_blocks` 必须结合冻结后的 PoW block-time
   实测标定；development 默认 `6650` 只表达约一天的量级，不是 public 参数。
3. 深层 BTC reorg 采用 orphan snapshot archive 还是 deterministic USDB rewind，必须在
   public release 前完成 restart/joiner/live E2E；future SPV policy 单独版本化。
