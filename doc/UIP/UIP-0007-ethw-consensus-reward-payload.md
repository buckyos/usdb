UIP: UIP-0007
Title: ETHW Consensus Reward Payload
Status: Draft
Type: Standards Track
Layer: ETHW Header / Consensus
Created: 2026-04-25
Requires: UIP-0000, UIP-0005, UIP-0006
Activation: ETHW network activation matrix; USDB development chains activate from genesis

# 摘要

本文定义 ETHW / USDB 链在区块头 `header.Extra` 中携带的最小 USDB reward consensus payload。

当前 v1 对齐已落地的 `RewardPayloadV1` 思路：

- payload 使用固定长度二进制编码。
- payload 写入 `header.Extra`。
- payload 只携带历史状态 selector 和 `pass_id`。
- payload 不直接携带 `energy`、`level`、`reward`、`owner`、`state` 或 collab 明细。
- validator 必须使用 payload 里的 selector 查询 UIP-0006 定义的 USDB Economic State View，并本地重算 reward input。

# 动机

ETHW validator 验证旧块时，不能查询 USDB current head。旧块必须携带足够信息，使 validator 能回到该块出块时引用的 BTC / USDB 历史状态。

同时，链上 payload 必须尽量小：

- `header.Extra` 是区块头字段，会进入 PoW seal hash。
- payload 长度影响所有区块。
- 审计字段可以通过 USDB-side view 查询，不需要全部写入区块头。

因此本文只标准化链上最小 selector；完整经济状态和审计字段由 UIP-0006 定义。

# 非目标

本文不定义：

- USDB-side economic state view 的完整 JSON 字段。
- raw energy、effective energy、level、difficulty factor 的公式。
- block reward schedule、fee split、uncle reward 或 dividend 规则。
- `level -> difficulty` 的 future PoW difficulty policy。
- collab bonus、协作者分润或 price / real_price。

# 术语

| 术语 | 含义 |
| --- | --- |
| `RewardPayloadV1` | 写入 ETHW `header.Extra` 的第一版固定二进制 payload。 |
| `btc_height` | payload 锁定的 BTC 历史高度。 |
| `snapshot_id` | upstream balance-history consensus snapshot id。 |
| `system_state_id` | USDB system state id。 |
| `pass_id` | 被本区块声明为 reward input 的 miner pass inscription id。 |
| `resolved_profile` | validator 根据 payload 查询 UIP-0006 后得到的经济状态。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# Payload Version

首版 payload version：

```text
payload_version = 1
```

该字段是 1 byte unsigned integer，写入 payload 第 0 字节。

payload 编码或字段集合改变时必须升级 `payload_version`。reward 公式改变但 payload 字节布局不变时，应通过 ETHW chain config / fork version 管理，不一定升级 `payload_version`。

# Binary Layout

`RewardPayloadV1` 固定长度为 105 bytes：

| Offset | Size | 字段 | 类型 | 编码 |
| --- | --- | --- | --- | --- |
| 0 | 1 | `payload_version` | uint8 | 固定为 `1`。 |
| 1 | 4 | `btc_height` | uint32 | big-endian。 |
| 5 | 32 | `snapshot_id` | bytes32 | 32-byte hex id 的原始字节。 |
| 37 | 32 | `system_state_id` | bytes32 | 32-byte hex id 的原始字节。 |
| 69 | 32 | `pass_txid` | bytes32 | inscription outpoint txid。 |
| 101 | 4 | `pass_index` | uint32 | inscription outpoint index，big-endian。 |

等价结构：

```text
RewardPayloadV1 =
    uint8 payload_version
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

- `header.Extra` 必须正好等于一个 `RewardPayloadV1`。
- `len(header.Extra)` 必须等于 `105`。
- 不得在 payload 前后拼接 vanity bytes、JSON、签名或其它 opaque data。
- `payload_version` 不支持时必须拒绝该区块。

实现可以将链级 `MaximumExtraDataSize` 设为大于 105 的值以预留后续版本空间，但 v1 验证必须按精确长度解析。

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

1. 从 `header.Extra` 解析 `RewardPayloadV1`。
2. 校验 payload version 和固定长度。
3. 使用 `btc_height`、`snapshot_id`、`system_state_id` 构造 USDB historical query context。
4. 使用 `pass_id` 查询 UIP-0006 定义的 pass economic profile，或使用等价的历史 `get_pass_snapshot` / `get_pass_energy` RPC 组合。
5. 按 ETHW reward rule version 从 resolved profile 重算 reward input。
6. 在 `Finalize` / state transition 中使用重算结果发放奖励。

任一步失败都必须 fail-closed。validator 不得因为 USDB 不可用、历史不可用或 mismatch 而继续接受新区块。

# Payload 不携带的字段

`RewardPayloadV1` 禁止直接携带：

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
RewardPayloadV1(header.Extra)
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

因此，`RewardPayloadV1` 是 UIP-0006 可查询状态的最小 selector，而不是 UIP-0006 JSON profile 的链上序列化。

# Miner Payload Generation

miner 生成新区块时应该：

1. 从本地 USDB companion service 获取 current system state。
2. 使用配置的 `pass_id` 在该 state 下确认 pass 可查询。
3. 将 `btc_height`、`snapshot_id`、`system_state_id` 和 `pass_id` 编码成 `RewardPayloadV1`。
4. 写入待挖区块的 `header.Extra`。

miner 不能正确构造 payload 时，不应继续挖 USDB reward-enabled 区块。

# Versioning

本文区分两类版本：

| 版本 | 位置 | 作用 |
| --- | --- | --- |
| `payload_version` | `header.Extra` 第 0 字节 | 描述 payload 字节布局。 |
| `reward_rule_version` | ETHW chain config / fork policy | 描述 reward 公式和奖励发放规则。 |

如果未来只改变 reward multiplier、base reward 或 collab bonus 公式，但 `RewardPayloadV1` 字节布局不变，不应强制升级 `payload_version`。

如果未来需要新增 `stable_block_hash`、candidate set、collab pass id 或 difficulty-specific 字段，则必须定义新的 payload version。

# 与 Difficulty 的边界

当前 v1 payload 是 reward payload，不直接定义 `level -> difficulty`。

如果后续 ETHW policy 引入 `level` 影响 PoW difficulty，应优先复用相同 selector 解析 UIP-0006 profile，再由新的 ETHW difficulty policy 决定：

- 是否仍使用 `RewardPayloadV1`。
- 是否升级 payload version。
- `base_difficulty` 是否来自 header / parent context。
- `real_difficulty` 是否需要显式承诺。

# 错误语义

实现至少需要区分：

| 错误 | 触发条件 |
| --- | --- |
| `MISSING_USDB_REWARD_PAYLOAD` | `header.Extra` 为空或未携带 payload。 |
| `PAYLOAD_SIZE_MISMATCH` | `len(header.Extra) != 105`。 |
| `PAYLOAD_VERSION_MISMATCH` | 不支持的 `payload_version`。 |
| `SNAPSHOT_ID_MISMATCH` | USDB historical state 与 payload `snapshot_id` 不一致。 |
| `SYSTEM_STATE_ID_MISMATCH` | USDB historical state 与 payload `system_state_id` 不一致。 |
| `PASS_NOT_FOUND` | `pass_id` 在该 historical context 下不存在。 |
| `HISTORY_NOT_AVAILABLE` | USDB 无法重放目标历史 context。 |
| `STATE_NOT_RETAINED` | USDB 已不保留目标历史 state。 |
| `REWARD_INPUT_INVALID` | resolved profile 不满足当前 reward rule。 |

# 测试要求

实现 UIP-0007 时，至少需要覆盖：

- `RewardPayloadV1` binary roundtrip。
- invalid version。
- invalid payload size。
- miner 生成的 `header.Extra` 长度正好为 105。
- validator 使用 payload selectors 查询历史 USDB state。
- BTC head 前进后，旧 ETHW block 仍按旧 payload 验证通过。
- same-height BTC reorg 后，旧 payload 返回 state mismatch。
- 缺少 USDB companion service 时 fail-closed。
- 篡改 `btc_height` / `snapshot_id` / `system_state_id` / `pass_id` 任一字段会导致验证失败。

# 待审计问题

1. v2 是否需要加入 `stable_block_hash`，以及是否值得增加 32 bytes。
2. future difficulty policy 是否继续复用 reward payload selector，还是定义独立 difficulty payload。
3. collab bonus 若进入 ETHW reward rule，是否需要显式携带 `collab_pass_id` 或只通过 UIP-0006 解析。
4. `reward_rule_version` 的 fork/version 字段应由 UIP-0008 还是 ETHW chain config 文档统一定义。
