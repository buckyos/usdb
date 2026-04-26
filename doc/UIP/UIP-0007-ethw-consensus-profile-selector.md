UIP: UIP-0007
Title: ETHW Consensus Profile Selector
Status: Draft
Type: Standards Track
Layer: ETHW Header / Consensus
Created: 2026-04-25
Requires: UIP-0000, UIP-0005, UIP-0006
Activation: ETHW network activation matrix; USDB development chains activate from genesis

# 摘要

本文定义 ETHW / USDB 链在区块头 `header.Extra` 中携带的最小 USDB consensus profile selector。

当前草案定义 `ProfileSelectorPayload`：

- payload 使用固定长度二进制编码。
- payload 写入 `header.Extra`。
- payload 携带历史状态 selector、`pass_id` 和 difficulty policy version commitment。
- payload 不直接携带 `energy`、`level`、`reward`、`owner`、`state` 或 collab 明细。
- validator 必须使用 payload 里的 selector 查询 UIP-0006 定义的 USDB Economic State View，并本地重算 reward input。
- future difficulty policy 如果依赖 pass level，也必须复用同一 selector 查询同一份 resolved profile。

# 动机

ETHW validator 验证旧块时，不能查询 USDB current head。旧块必须携带足够信息，使 validator 能回到该块出块时引用的 BTC / USDB 历史状态。

同时，链上 payload 必须尽量小：

- `header.Extra` 是区块头字段，会进入 PoW seal hash。
- payload 长度影响所有区块。
- 审计字段可以通过 USDB-side view 查询，不需要全部写入区块头。

因此本文只标准化链上最小 profile selector；完整经济状态和审计字段由 UIP-0006 定义。

# 非目标

本文不定义：

- USDB-side economic state view 的完整 JSON 字段。
- raw energy、effective energy、level、difficulty factor 的公式。
- block reward schedule、fee split、uncle reward 或 dividend 规则。
- `level -> difficulty` 的 future PoW difficulty policy；本文只要求其复用同一 profile selector。
- collab bonus、协作者分润或 price / real_price。

# 术语

| 术语 | 含义 |
| --- | --- |
| `ProfileSelectorPayload` | 写入 ETHW `header.Extra` 的固定二进制 profile selector payload。 |
| `profile_selector` | 用于定位某个历史 USDB state 下某张 pass profile 的最小字段集合。 |
| `difficulty_policy_version` | 本区块声明使用的 `level -> difficulty` 算法版本。 |
| `btc_height` | payload 锁定的 BTC 历史高度。 |
| `snapshot_id` | upstream balance-history consensus snapshot id。 |
| `system_state_id` | USDB system state id。 |
| `pass_id` | 被本区块声明为 consensus profile input 的 miner pass inscription id。 |
| `resolved_profile` | validator 根据 payload 查询 UIP-0006 后得到的经济状态。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# Payload Version

当前 payload version：

```text
payload_version = 1
```

该字段是 1 byte unsigned integer，写入 payload 第 0 字节。

payload 编码或字段集合改变时必须升级 `payload_version`。reward 公式或 difficulty policy 公式改变但 payload 字节布局不变时，应通过 ETHW chain config / fork version 管理，不一定升级 `payload_version`。

# Binary Layout

`ProfileSelectorPayload` 固定长度为 107 bytes：

| Offset | Size | 字段 | 类型 | 编码 |
| --- | --- | --- | --- | --- |
| 0 | 1 | `payload_version` | uint8 | 固定为 `1`。 |
| 1 | 2 | `difficulty_policy_version` | uint16 | big-endian。必须匹配 ETHW chain config 在该 block height 下的期望值。 |
| 3 | 4 | `btc_height` | uint32 | big-endian。 |
| 7 | 32 | `snapshot_id` | bytes32 | 32-byte hex id 的原始字节。 |
| 39 | 32 | `system_state_id` | bytes32 | 32-byte hex id 的原始字节。 |
| 71 | 32 | `pass_txid` | bytes32 | inscription outpoint txid。 |
| 103 | 4 | `pass_index` | uint32 | inscription outpoint index，big-endian。 |

等价结构：

```text
ProfileSelectorPayload =
    uint8 payload_version
    uint16 difficulty_policy_version
    uint32 btc_height
    bytes32 snapshot_id
    bytes32 system_state_id
    bytes32 pass_txid
    uint32 pass_index
```

链外展示 `pass_id` 时使用：

```text
pass_id = lowercase_hex(pass_txid) + "i" + decimal(pass_index)
```

# Header Extra 规则

当 ETHW USDB reward consensus rule 激活时：

- `header.Extra` 必须正好等于一个 `ProfileSelectorPayload`。
- `len(header.Extra)` 必须等于 `107`。
- 不得在 payload 前后拼接 vanity bytes、JSON、签名或其它 opaque data。
- `payload_version` 不支持时必须拒绝该区块。
- `difficulty_policy_version` 与 ETHW chain config 在该 block height 下的期望值不一致时必须拒绝该区块。

实现可以将链级 `MaximumExtraDataSize` 设为大于 107 的值以预留后续版本空间，但 v1 验证必须按精确长度解析。

# stable_block_hash

`ProfileSelectorPayload` 不携带 `stable_block_hash`。

原因：

- `snapshot_id` 已由 USDB / balance-history 的 `ConsensusSnapshotIdentity` 派生，identity 中已经包含 `stable_block_hash`。
- `system_state_id` 又绑定 upstream `snapshot_id` 与 usdb-indexer local state commit。
- 因此 `stable_block_hash` 对链上共识约束是冗余字段，只增加诊断直观性。
- 加入该字段会让 v1 payload 从 107 bytes 增至 139 bytes。

验证和审计时需要展示 `stable_block_hash` 的，必须通过 UIP-0006 USDB Economic State View 返回。除非未来发现 `snapshot_id` / `system_state_id` 不能覆盖某类共识安全需求，否则 v2 不应仅为了展示便利加入 `stable_block_hash`。

# 字段语义

## btc_height

`btc_height` 是 validator 构造 `ConsensusQueryContext.requested_height` 的输入。

validator 不得用当前 USDB head 替代该高度。

## snapshot_id

`snapshot_id` 锁定 upstream balance-history consensus snapshot。

validator 必须把它放入 expected state，并要求 USDB 返回同一 snapshot。

## system_state_id

`system_state_id` 锁定 usdb-indexer 暴露给下游链的顶层系统状态。

validator 必须把它放入 expected state，并要求 USDB 返回同一 system state。

## pass_id

`pass_id` 是本块声明使用的 miner pass。

v1 必须显式携带 `pass_id`，不得通过 `coinbase`、`eth_main` 或其它地址字段隐式反查。原因是：

- 当前 USDB 稳定查询主键是 pass id / inscription id。
- 一个 ETH 地址不一定唯一映射到一张 pass。
- 后续 candidate-set 或多 pass 场景需要避免隐式选择歧义。

# Validator Replay

validator 必须按以下顺序验证：

1. 从 `header.Extra` 解析 `ProfileSelectorPayload`。
2. 校验 payload version 和固定长度。
3. 使用 `btc_height`、`snapshot_id`、`system_state_id` 构造 USDB historical query context。
4. 使用 `pass_id` 查询 UIP-0006 定义的 pass economic profile，或使用等价的历史 `get_pass_snapshot` / `get_pass_energy` RPC 组合。
5. 按 ETHW reward rule version 从 resolved profile 重算 reward input。
6. 如果 future ETHW difficulty policy 已激活，并且该 policy 依赖 USDB level，则使用同一个 resolved profile 重算本块应有 difficulty。
7. 在 `Finalize` / state transition 中使用重算 reward 结果发放奖励。

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

这些字段必须通过 UIP-0006 state view 或 ETHW 本地 policy 在验证时重算。

# 与 UIP-0006 的关系

本文是 ETHW-side 链上 payload 规范。UIP-0006 是 USDB-side state view 规范。

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
ETHW reward rule / future difficulty rule
```

因此，`ProfileSelectorPayload` 是 UIP-0006 可查询状态的最小 selector，而不是 UIP-0006 JSON profile 的链上序列化。

# Reward 与 Difficulty 共享 Selector

`ProfileSelectorPayload` 的 selector 是：

```text
btc_height + snapshot_id + system_state_id + pass_id
```

reward rule 和 future difficulty policy 必须消费同一个 selector 得到的同一份 `resolved_profile`。不得定义第二套独立 difficulty payload 来携带另一组 `{btc_height, snapshot_id, system_state_id, pass_id}`。

原因：

- 独立 difficulty payload 会引入 reward 使用 pass A、difficulty 使用 pass B 的歧义。
- 同一区块的 reward、difficulty 和审计视图应引用同一张 miner pass。
- `header.Extra` 字节空间有限，重复 selector 没有必要。

如果 future difficulty policy 需要额外参数，应该优先放入 ETHW chain config / difficulty policy version，而不是在 header 中复制第二套 USDB selector。

# Miner Payload Generation

miner 生成新区块时应该：

1. 从本地 USDB companion service 获取 current system state。
2. 使用配置的 `pass_id` 在该 state 下确认 pass 可查询。
3. 从 ETHW chain config 读取 candidate block height 对应的 expected `difficulty_policy_version`。
4. 将 `difficulty_policy_version`、`btc_height`、`snapshot_id`、`system_state_id` 和 `pass_id` 编码成 `ProfileSelectorPayload`。
5. 写入待挖区块的 `header.Extra`。

miner 不能正确构造 payload 时，不应继续挖 USDB reward-enabled 区块。

# Versioning

本文区分以下版本：

| 版本 | 位置 | 作用 |
| --- | --- | --- |
| `payload_version` | `header.Extra` 第 0 字节 | 描述 payload 字节布局。 |
| `difficulty_policy_version` | `header.Extra` 第 1-2 字节；期望值来自 ETHW chain config / fork policy | 描述 `level -> difficulty` 公式和校验规则。 |
| `reward_rule_version` | ETHW chain config / fork policy | 描述 reward 公式和奖励发放规则。 |

如果未来只改变 reward multiplier、base reward、collab bonus 或 difficulty policy 公式，但 `ProfileSelectorPayload` 字节布局不变，不应强制升级 `payload_version`。

`difficulty_policy_version` 进入 payload 不是为了允许 miner 选择算法，而是为了让区块头显式承诺其声明的 difficulty policy。validator 必须用 ETHW chain config / fork policy 计算该 block height 下的 expected `difficulty_policy_version`，并要求 payload 中的值完全一致。

如果未来确实需要在 header 中新增 selector 字段，则必须定义新的 payload version。仅为展示 `stable_block_hash`、列出 collab pass、或配置 difficulty policy 参数，不应升级 header payload；这些信息应优先来自 UIP-0006 state view 或 UIP-0009 chain config。

# 与 Difficulty 的边界

当前 v1 payload 不直接定义 `level -> difficulty`。

如果后续 ETHW policy 引入 `level` 影响 PoW difficulty，应复用相同 selector 解析 UIP-0006 profile，再由新的 ETHW difficulty policy 决定：

- 是否仍使用 `ProfileSelectorPayload`。
- 是否升级 payload version。
- `base_difficulty` 是否来自 header / parent context。
- `real_difficulty` 是否需要显式承诺。

由于 difficulty 规则可能独立于 reward 规则演进，ETHW chain config 应定义独立的 `difficulty_policy_version` 激活规则。该版本字段同时进入 `header.Extra` 作为显式承诺，但 validator 必须以 chain config 的 expected version 为准，不得让 payload 中的值覆盖本地共识配置。

# 与 Collab Bonus 的边界

collab bonus 若进入 ETHW reward rule，不得要求每个区块在 header 中携带 Leader 的完整 `collab_pass_id` 列表。

原因：

- Leader 的协作者数量可能很大。
- 把所有 collab pass id 放入区块会导致 header 或 block body 大小不可控。
- collab contribution 和 breakdown 已由 UIP-0006 在确定历史 context 下提供。

因此，v1 payload 仍只携带 Leader `pass_id`。collab bonus 的 aggregate input 应通过 UIP-0006 profile 中的 `collab_contribution`、`effective_energy` 或后续明确的 bonus 字段重算。如果未来需要给协作者直接分润，应通过 UIP-0006 `get_collab_breakdown` 可验证查询、单独结算/claim 机制，或后续 reward distribution UIP 定义，而不是把全量 collab list 塞进 `header.Extra`。

# 与 ETHW Chain Config 的边界

`payload_version` 只描述 `header.Extra` 字节布局。以下字段和规则不属于本文，必须由 ETHW chain config / bootstrap UIP 定义：

- ChainID / NetworkId。
- genesis 和 PoW 基础参数。
- USDB reward consensus 是否启用。
- active `payload_version`。
- `reward_rule_version`。
- expected `difficulty_policy_version` 及其激活高度。
- 这些版本从 genesis 生效还是在后续 fork 高度生效。

UIP-0008 负责通用版本激活矩阵。ETHW 具体 chain config、genesis、USDB reward/difficulty policy version 字段应由单独的 ETHW Chain Config UIP 定义。

# 错误语义

实现至少需要区分：

| 错误 | 触发条件 |
| --- | --- |
| `MISSING_USDB_PROFILE_SELECTOR` | `header.Extra` 为空或未携带 profile selector。 |
| `PAYLOAD_SIZE_MISMATCH` | `len(header.Extra) != 107`。 |
| `PAYLOAD_VERSION_MISMATCH` | 不支持的 `payload_version`。 |
| `DIFFICULTY_POLICY_VERSION_MISMATCH` | payload `difficulty_policy_version` 与 chain config expected version 不一致。 |
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
- miner 生成的 `header.Extra` 长度正好为 107。
- `difficulty_policy_version` 与 chain config expected version 不一致时拒绝。
- validator 使用 payload selectors 查询历史 USDB state。
- BTC head 前进后，旧 ETHW block 仍按旧 payload 验证通过。
- same-height BTC reorg 后，旧 payload 返回 state mismatch。
- 缺少 USDB companion service 时 fail-closed。
- 篡改 `btc_height` / `snapshot_id` / `system_state_id` / `pass_id` 任一字段会导致验证失败。

# 实现迁移注意

- 当前 go-ethereum 原型中的 `RewardPayloadV1` 命名应在正式实现前重命名为 `ProfileSelectorPayload`。该重命名需要同步加入 `difficulty_policy_version`，正式 v1 固定布局为 107 bytes。

# 后续实现议题

1. collab bonus 的 aggregate 字段是否直接复用 `effective_energy`，还是定义独立 `collab_bonus_energy` / `collab_bonus_bps`。
