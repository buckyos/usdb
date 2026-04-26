UIP: UIP-0006
Title: USDB Economic State View
Status: Draft
Type: Standards Track
Layer: USDB Indexer RPC / BTC Application Query
Created: 2026-04-25
Requires: UIP-0000, UIP-0002, UIP-0003, UIP-0004, UIP-0005
Activation: USDB index protocol and formula version; development networks activate from height 0 after implementation

# 摘要

本文定义 `usdb-indexer` 对外提供的经济状态视图和审计视图。

它不是 ETHW 区块头里的链上 payload 编码，而是下游链、validator、浏览器和审计工具在某一 BTC 历史 context 下可查询、可重放、可比对的 USDB-side state view。

核心规则：

- view 必须绑定一个可重放的 BTC / USDB `external_state`。
- view 可以返回 `raw_energy`、`collab_contribution`、`effective_energy`、`level`、`difficulty_factor_bps` 和 `collab_breakdown`。
- energy 类字段必须使用 UIP-0003 的 `uint128` canonical decimal string。
- `level` 和 `difficulty_factor_bps` 是基于 `effective_energy` 的查询时派生值，不要求持久化。
- `leader_eligible`、ETHW `base_difficulty`、ETHW `real_difficulty`、reward rule 和 header payload encoding 不属于本文。
- ETHW 链上共识 payload 应消费本文定义的 state view，但不得把本文的完整审计字段集合等同于链上 payload 字节。

# 动机

UIP-0003、UIP-0004 和 UIP-0005 分别定义了：

- raw energy 和继承。
- collab contribution 和 effective energy。
- level 和 difficulty factor。

这些值需要通过 `usdb-indexer` 形成统一的历史查询口径。否则 ETHW validator、测试脚本、浏览器和审计工具会各自拼接 RPC 字段，容易产生以下问题：

- current head 查询被误用于历史块验证。
- raw energy、collab contribution、effective energy 混用。
- ETHW policy 字段反向污染 BTC-side 派生状态。
- 链上 payload 字段和审计明细字段边界不清。

本文把 USDB-side 能提供的完整经济状态视图单独协议化。ETHW 链上 payload 只需要引用其中的最小状态选择器，并在验证时按本文规则重算或查询。

# 非目标

本文不定义：

- ETHW `header.Extra` 二进制编码。
- ETHW `RewardPayloadV1` 字段布局。
- ETHW `base_difficulty` 来源、PoW target 编码或 chain weight 规则。
- ETHW block reward、fee split、uncle reward、CoinBase 或分红池规则。
- pass 铭文 schema、pass 状态机、energy 公式本身。
- Leader eligibility 的报价窗口和 ETHW 出块历史策略。

# 术语

| 术语 | 含义 |
| --- | --- |
| `external_state` | 绑定一次历史查询的 BTC / USDB 状态选择器。 |
| `economic_state_view` | `usdb-indexer` 在一个 `external_state` 下返回的经济状态视图。 |
| `pass_economic_profile` | 某张 pass 在指定历史 context 下的 pass snapshot + energy profile。 |
| `candidate_set_view` | 多张 candidate pass 的排序/审计查询结果；不等同于 ETHW 链上 payload。 |
| `resolved_profile` | 下游 validator 根据链上 payload 反查本文 state view 后得到的重算结果。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# View 版本

首版 view 版本建议：

```text
view_version = "uip-0006-usdb-economic-state-view:v1"
```

影响以下内容时必须升级 `view_version`：

- JSON 字段集合。
- 字段 canonical encoding。
- 历史查询语义。
- candidate set 排序规则。
- mismatch / history unavailable 错误语义。

影响公式参数但不改变 view 结构时，应升级 `formula_version`，不一定升级 `view_version`。

# External State

`external_state` 必须足够构造 `ConsensusQueryContext` 并重放 BTC 历史查询。

建议字段：

| 字段 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `btc_height` | integer | 是 | 查询对应的 BTC 高度。 |
| `snapshot_id` | string | 是 | upstream balance-history consensus snapshot id。 |
| `system_state_id` | string | 是 | 下游链消费的顶层 USDB system state id。 |
| `stable_block_hash` | string | 建议 | `btc_height` 对应的 stable BTC block hash。 |
| `local_state_commit` | string | 建议 | usdb-indexer local durable state commit。 |
| `balance_history_semantics_version` | string | 建议 | balance-history 历史查询语义版本。 |
| `usdb_index_protocol_version` | string | 是 | usdb-indexer 外部协议版本。 |
| `usdb_index_formula_version` | string | 是 | energy / effective energy / level 公式版本。 |

最小链上 payload 可以只携带 `btc_height`、`snapshot_id`、`system_state_id` 和业务对象 id。USDB-side view 查询返回时应该尽量补齐完整 `external_state`，便于审计和排错。

# Pass Economic Profile

单张 pass 的经济状态视图建议结构：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "external_state": {
    "btc_height": 900123,
    "snapshot_id": "...",
    "stable_block_hash": "000000...",
    "local_state_commit": "...",
    "system_state_id": "...",
    "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
    "usdb_index_protocol_version": "1.0.0",
    "usdb_index_formula_version": "pass-energy-formula:v1"
  },
  "pass": {
    "pass_id": "txidi0",
    "owner": "...",
    "state": "active",
    "pass_kind": "standard",
    "raw_energy": "1000000",
    "collab_contribution": "500000",
    "effective_energy": "1500000",
    "level": 1,
    "difficulty_factor_bps": 9900,
    "collab_breakdown": []
  }
}
```

字段语义：

| 字段 | 类型 | 来源 | 说明 |
| --- | --- | --- | --- |
| `pass_id` | string | UIP-0001 / UIP-0002 | inscription id。 |
| `owner` | string | pass snapshot | 当前历史 context 下的 BTC owner。 |
| `state` | string | UIP-0002 | `active` / `dormant` / `consumed` / `burned` / `invalid`。 |
| `pass_kind` | string | UIP-0001 | `standard` / `collab`。 |
| `raw_energy` | decimal string | UIP-0003 | pass 自身 raw energy。 |
| `collab_contribution` | decimal string | UIP-0004 | 作为 Leader 获得的协作贡献。 |
| `effective_energy` | decimal string | UIP-0004 | `raw_energy + collab_contribution`，仅 standard active pass 可用于 candidate。 |
| `level` | integer | UIP-0005 | 从 `effective_energy` 动态派生。 |
| `difficulty_factor_bps` | integer | UIP-0005 | 从 `level` 动态派生。 |
| `collab_breakdown` | array | UIP-0004 | 协作贡献明细，可内联或通过审计查询分页获取。 |

## Energy 字段编码

`raw_energy`、`collab_contribution`、`effective_energy` 必须使用 UIP-0003 的 canonical decimal string。

禁止使用 JSON number 表示 energy。

## Standard 与 Collab Pass

standard pass:

- 可以拥有 `raw_energy`。
- 可以作为 Leader 接收 `collab_contribution`。
- `effective_energy = raw_energy + collab_contribution`。
- 如果处于 `active`，可以成为下游链 candidate。

collab pass:

- 可以拥有自身 `raw_energy`。
- 不得直接作为下游链 independent candidate。
- 对自身查询时 `collab_contribution = 0`。
- 对自身查询时 `effective_energy = 0`，除非后续 UIP 明确引入新的用途。
- 其贡献必须通过 Leader 的 `collab_breakdown` 进入 Leader 的 `collab_contribution`。

# Collab Breakdown

`collab_breakdown` 建议 item：

```json
{
  "collab_pass_id": "txidi1",
  "collab_owner": "...",
  "collab_raw_energy": "1000000",
  "collab_weight_bps": 5000,
  "collab_contribution": "500000",
  "leader_ref_kind": "leader_btc_addr",
  "leader_ref_value": "bc1..."
}
```

如果主 profile 不内联完整 `collab_breakdown`，必须提供等价的审计查询，使下游能在同一 `external_state` 下重算 aggregate `collab_contribution`。

aggregate `collab_contribution` 不得被视为不可验证黑盒。

# Candidate Set View

USDB-side 可以提供 candidate set audit view，用于浏览器、测试或下游链调试。

建议排序规则：

```text
selection_rule = "uip-0006:effective-energy-desc-pass-id-asc:v1"
```

含义：

```text
winner = max(candidate_set.items, by effective_energy)
tie_break = smallest pass_id lexical order
```

该规则只定义 USDB-side audit view 的确定性排序。ETHW 链上 payload 是否携带 candidate set、是否只携带 selected `pass_id`、是否使用 PoW threshold 验证，由 ETHW-side UIP 定义。

# 查询语义

实现可以将本文映射为一个或多个 RPC，例如：

- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`

无论 RPC 如何拆分，必须满足：

- 同一 `external_state` 下返回确定结果。
- 不得在历史查询失败时自动退回 current head。
- BTC head 前进后，旧 `external_state` 仍按历史 context 重放。
- same-height reorg 后，若 `external_state` 不再匹配 canonical history，必须返回 mismatch。
- history retention 不足时必须返回 `HISTORY_NOT_AVAILABLE` 或 `STATE_NOT_RETAINED`。

# 错误语义

实现至少需要区分：

| 错误 | 触发条件 |
| --- | --- |
| `VIEW_VERSION_MISMATCH` | 不支持的 `view_version`。 |
| `PROTOCOL_VERSION_MISMATCH` | `usdb_index_protocol_version` 不匹配。 |
| `FORMULA_VERSION_MISMATCH` | `usdb_index_formula_version` 不匹配。 |
| `SNAPSHOT_ID_MISMATCH` | `external_state.snapshot_id` 与历史 state ref 不匹配。 |
| `LOCAL_STATE_COMMIT_MISMATCH` | `local_state_commit` 不匹配。 |
| `SYSTEM_STATE_ID_MISMATCH` | `system_state_id` 不匹配。 |
| `HISTORY_NOT_AVAILABLE` | 所需历史 context 已不可用。 |
| `STATE_NOT_RETAINED` | 本地 durable state 不再保留目标高度。 |
| `PASS_NOT_FOUND` | 目标 pass 在该 context 下不存在。 |
| `ECONOMIC_FIELD_MISMATCH` | 调试/审计输入中的字段与重算结果不一致。 |

错误响应应该带 structured data，至少包含 expected state、actual state、requested height 和 mismatch 字段名。

# 与 ETHW 链上 Payload 的关系

ETHW 链上 payload 应只携带验证旧块所需的最小 selector。validator 再使用这些 selector 调用本文定义的 USDB-side state view。

当前关系：

```text
ETHW RewardPayloadV1
    -> btc_height
    -> snapshot_id
    -> system_state_id
    -> pass_id
        |
        v
USDB Economic State View
    -> pass snapshot
    -> raw_energy
    -> collab_contribution
    -> effective_energy
    -> level
    -> difficulty_factor_bps
    -> collab_breakdown
```

因此，本文字段集合是 ETHW 链上 payload 可解析状态的超集，不代表这些字段都应写入 ETHW 区块头。

# 测试要求

实现 UIP-0006 时，至少需要覆盖：

- valid profile 按历史 `external_state` 查询通过。
- BTC head 前进后旧 profile 仍按历史 context 查询通过。
- same-height reorg 后旧 `external_state` 返回 state mismatch。
- `raw_energy`、`collab_contribution`、`effective_energy`、`level`、`difficulty_factor_bps` 可在同一 context 下重算一致。
- collab Leader profile 可通过 breakdown 或审计查询重算 aggregate contribution。
- collab pass 不直接进入 candidate set view。
- `view_version` / `protocol_version` / `formula_version` mismatch。
- history retention 不足时返回 `HISTORY_NOT_AVAILABLE` 或 `STATE_NOT_RETAINED`。

# 待审计问题

1. `collab_breakdown` 是否在主 profile 中必填，还是允许通过分页审计查询按需获取。
2. candidate set view 是否由 usdb-indexer 一等提供，还是只提供单 pass profile 后由下游自行排序。
3. owner 字段的 canonical 表示是否统一为 script hash，还是保留 BTC address display 字段。
4. `stable_block_hash` 和 `local_state_commit` 是否应在所有 profile 响应中强制返回。
