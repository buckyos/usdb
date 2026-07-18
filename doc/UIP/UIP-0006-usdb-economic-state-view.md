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
- view 可以返回 `raw_energy`、`collab_contribution`、`effective_energy`、`level`、`difficulty_factor_bps` 和 `collab_breakdown_count`。
- 完整 `collab_breakdown` 通过同一历史 context 下的确定性分页查询提供，不要求内联到主 profile。
- energy 类字段必须使用 UIP-0003 的 `uint128` canonical decimal string。
- `level` 和 `difficulty_factor_bps` 是基于 `effective_energy` 的查询时派生值，不要求持久化。
- `leader_eligible`、ETHW `base_difficulty`、ETHW `real_difficulty`、reward rule 和 header payload encoding 不属于本文。
- ETHW 链上共识 payload 应消费本文定义的 state view，但不得把本文的完整审计字段集合等同于链上 payload 字节。

# 动机

UIP-0003、UIP-0004 和 UIP-0005 分别定义了：

- raw energy 和继承。
- collab contribution 和 effective energy。
- level 和 difficulty factor。

这些值需要通过 `usdb-indexer` 形成统一的历史查询口径。否则 USDB validator、测试脚本、浏览器和审计工具会各自拼接 RPC 字段，容易产生以下问题：

- current head 查询被误用于历史块验证。
- raw energy、collab contribution、effective energy 混用。
- ETHW policy 字段反向污染 BTC-side 派生状态。
- 链上 payload 字段和审计明细字段边界不清。

本文把 USDB-side 能提供的完整经济状态视图单独协议化。USDB 链上 payload 只需要引用其中的最小状态选择器，并在验证时按本文规则重算或查询。

# 当前实现状态

参考实现已经完成 v1 历史 context / version 校验基础、`get_pass_economic_profile`、`get_candidate_set_view` 和 `get_collab_breakdown` 的核心派生逻辑，以及 candidate/breakdown 的 opaque `cursor + limit` 稳定分页。

`get_pass_economic_profile` 当前已满足：

- 必填 `view_version`，并返回完整历史 `external_state`。
- active standard 的 raw / collab / effective energy、level、difficulty factor 和 breakdown count 运行时派生。
- collab 与 non-active pass 的 effective energy 状态边界。
- invalid pass 无 energy row 时的 canonical 零值合成。
- `PASS_NOT_FOUND` 与 non-invalid 缺 raw energy 的 `INTERNAL_INVARIANT_BROKEN` 错误边界。
- BTC head 前进后按旧 `external_state` 重放同一 profile。
- 派生前后重建并复验完整 historical state ref，同高度 reorg 期间不返回混合状态响应。

candidate/breakdown 当前已直接使用本文固定的 `cursor + limit`，不保留旧 `page/page_size` 双栈。参考实现的 `max_limit` 为 500，cursor 绑定完整 external state、resource/query 条件、limit 和 continuation key。

参考实现也已补齐跨组件调用面：Rust typed client、`usdb-indexer-cli` 和 control-plane proxy 都能直接调用 historical state ref/profile/candidate/breakdown。`get_rpc_info` 现在显式声明 view version、candidate selection rule、max limit 和四个必需 feature；control-plane 只有在 service/API/version/rule/features 全部匹配时才声明 UIP-0006 economic state view 可用。

happy path、same-height reorg、three-pass candidate 和 three-collab breakdown 已通过真实 ord targeted smoke。集中回归入口现已切换到 canonical profile/candidate view，并补入 formula-version mismatch 与 three-collab 场景；完整 live/regtest 矩阵和大数据量查询/索引性能评估仍需在下一阶段执行。

# 能力发现

UIP-0006 consumer 必须通过 `get_rpc_info` 判断服务能力，不得把 RPC 可达或 `query_ready` 等价为协议兼容。

当前 v1 的必需声明为：

```text
economic_state_view_version = "uip-0006-usdb-economic-state-view:v1"
candidate_set_selection_rule = "uip-0006:effective-energy-desc-pass-id-asc:v1"
economic_page_max_limit > 0
features contains:
  historical_state_ref
  pass_economic_profile
  candidate_set_view
  collab_breakdown
```

`query_ready / consensus_ready` 描述当前运行状态；上述字段描述实现能力和契约版本。consumer 需要同时满足自身所需的 readiness 与协议能力条件。

# 非目标

本文不定义：

- ETHW `header.Extra` 二进制编码。
- ETHW `ProfileSelectorPayload` 字段布局。
- ETHW `base_difficulty` 来源、PoW target 编码或 chain weight 规则。
- ETHW block reward、fee split、uncle reward、CoinBase 或分红池规则。
- pass 铭文 schema、pass 状态机、energy 公式本身。
- Leader eligibility 的报价窗口和 USDB 链出块历史策略。

# 术语

| 术语 | 含义 |
| --- | --- |
| `external_state` | 绑定一次历史查询的 BTC / USDB 状态选择器。 |
| `economic_state_view` | `usdb-indexer` 在一个 `external_state` 下返回的经济状态视图。 |
| `pass_economic_profile` | 某张 pass 在指定历史 context 下的 pass snapshot + energy profile。 |
| `candidate_set_view` | 多张 candidate pass 的排序/审计查询结果；不等同于 USDB 链上 payload。 |
| `resolved_profile` | 下游 validator 根据链上 payload 反查本文 state view 后得到的重算结果。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# View 版本

首版 view 版本固定为：

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

所有 UIP-0006 v1 查询必须在请求顶层显式携带：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1"
}
```

`view_version` 是查询/响应结构与语义 selector，不属于历史状态 identity，因此禁止放入 `ConsensusStateReference.expected_state`。字段缺失属于无效请求；字段存在但服务不支持时必须返回 `VIEW_VERSION_MISMATCH`。当前项目尚未公开激活旧协议，参考实现不保留省略该字段的兼容入口。

# External State

`external_state` 必须足够构造 `ConsensusQueryContext` 并重放 BTC 历史查询。

v1 固定字段：

| 字段 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `btc_height` | integer | 是 | 查询对应的 BTC 高度。 |
| `snapshot_id` | string | 是 | upstream balance-history consensus snapshot id。 |
| `stable_block_hash` | string | 是 | `btc_height` 对应的 stable BTC block hash。 |
| `local_state_commit` | string | 是 | usdb-indexer local durable state commit。 |
| `system_state_id` | string | 是 | 下游链消费的顶层 USDB system state id。 |
| `balance_history_api_version` | string | 是 | balance-history 对外 API 版本。 |
| `balance_history_semantics_version` | string | 是 | balance-history 历史查询语义版本。 |
| `usdb_index_protocol_version` | string | 是 | usdb-indexer 外部协议版本。 |
| `usdb_index_formula_version` | string | 是 | energy / effective energy / level 公式版本。 |

`external_state` 必须由目标高度的历史 `HistoricalStateRefInfo` 构造，禁止拿当前二进制常量覆盖历史 identity 中的 protocol/formula version。最小链上 payload 可以只携带 `btc_height`、`snapshot_id`、`system_state_id` 和业务对象 id；所有 UIP-0006 economic view 响应必须返回上表完整字段。只有后续协议明确声明为 selector-only 的轻量接口才可以省略字段。

# Pass Economic Profile

`get_pass_economic_profile` v1 请求字段固定为：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "pass_id": "txidi0",
  "block_height": 900123,
  "context": {
    "requested_height": 900123,
    "expected_state": {
      "snapshot_id": "...",
      "stable_height": 900123,
      "stable_block_hash": "000000...",
      "local_state_commit": "...",
      "system_state_id": "...",
      "balance_history_api_version": "1.0.0",
      "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
      "usdb_index_protocol_version": "1.0.0",
      "usdb_index_formula_version": "pass-energy-formula:v1"
    }
  }
}
```

`block_height` 与 `context.requested_height` 同时存在时必须相等。`context` 可以省略；服务仍必须把最终解析高度的完整历史 identity 返回为 `external_state`。一旦提供 `expected_state`，服务必须逐字段按目标高度的历史 identity 校验，禁止与当前二进制常量或 current head 比较。

单张 pass 的经济状态视图 v1 结构：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "external_state": {
    "btc_height": 900123,
    "snapshot_id": "...",
    "stable_block_hash": "000000...",
    "local_state_commit": "...",
    "system_state_id": "...",
    "balance_history_api_version": "1.0.0",
    "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
    "usdb_index_protocol_version": "1.0.0",
    "usdb_index_formula_version": "pass-energy-formula:v1"
  },
  "pass": {
    "pass_id": "txidi0",
    "owner_script_hash": "...",
    "owner_btc_addr": "bc1...",
    "state": "active",
    "pass_kind": "standard",
    "raw_energy": "1000000",
    "collab_contribution": "500000",
    "effective_energy": "1500000",
    "level": 1,
    "difficulty_factor_bps": 9900,
    "collab_breakdown_count": 3
  }
}
```

字段语义：

| 字段 | 类型 | 来源 | 说明 |
| --- | --- | --- | --- |
| `pass_id` | string | UIP-0001 / UIP-0002 | inscription id。 |
| `owner_script_hash` | string | pass snapshot | 当前历史 context 下的 canonical owner id，用于比较和索引。 |
| `owner_btc_addr` | string | pass snapshot | 当前历史 context 下可展示的 BTC address；当实现能确定 address 时应该返回。 |
| `state` | string | UIP-0002 | `active` / `dormant` / `consumed` / `burned` / `invalid`。 |
| `pass_kind` | string | UIP-0001 | `standard` / `collab`。 |
| `raw_energy` | decimal string | UIP-0003 | pass 自身 raw energy。 |
| `collab_contribution` | decimal string | UIP-0004 | 作为 Leader 获得的协作贡献。 |
| `effective_energy` | decimal string | UIP-0004 | `raw_energy + collab_contribution`，仅 standard active pass 可用于 candidate。 |
| `level` | integer | UIP-0005 | 从 `effective_energy` 动态派生。 |
| `difficulty_factor_bps` | integer | UIP-0005 | 从 `level` 动态派生。 |
| `collab_breakdown_count` | integer | UIP-0004 | 当前 context 下贡献给该 Leader 的 collab pass 数量。 |

## Owner 表示

owner 的 canonical 表示必须是 script hash 或等价确定性 owner id。该字段用于：

- 单 owner 单 active pass 校验。
- history query 比对。
- candidate set 聚合和索引。

当实现能够从 pass satpoint 或历史输出脚本明确得到 BTC address 时，profile 应该同时返回 `owner_btc_addr`。address 可以推导出 script hash，但 script hash 反查 address 需要额外上下文，因此浏览器和审计视图保留 address display 字段是合理的。

如果存在无法唯一编码为标准 BTC address 的 script，`owner_btc_addr` 可以为空，但 `owner_script_hash` 必须存在。

从 `owner_script_hash` 反查 `owner_btc_addr` 不属于 UIP-0006 的核心要求。若后续需要一等反向查询能力，应通过独立索引器或后续 UIP 定义 script hash -> address 映射、快照语义和历史保留规则。缺少该反向索引不得阻塞 core profile、candidate set 或 ETHW reward replay。

## Energy 字段编码

`raw_energy`、`collab_contribution`、`effective_energy` 必须使用 UIP-0003 的 canonical decimal string。

禁止使用 JSON number 表示 energy。

## Invalid Pass 零值

`invalid` pass 的 profile 必须可查询，并返回以下 canonical 派生值：

```text
raw_energy = "0"
collab_contribution = "0"
effective_energy = "0"
level = 0
difficulty_factor_bps = 10000
collab_breakdown_count = 0
```

该规则是查询层语义，不要求为 invalid mint 在 energy DB 中伪造一条记录。参考实现应从 pass history 识别 `invalid`，在 profile resolver 中合成零值；不得因为底层没有 energy row 而返回 `ENERGY_NOT_FOUND`。底层 record-oriented energy RPC 不属于该 profile 规则。

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
- 其贡献必须通过 Leader 的 collab breakdown 查询进入 Leader 的 `collab_contribution`。

# Collab Breakdown

`collab_breakdown` 不要求内联在主 profile 中。原因是一个 Leader 可能拥有大量 collab pass，直接在主 profile 中返回完整数组会影响浏览器 overview、USDB validator replay 和普通单 pass 查询的响应大小。

实现必须提供确定历史状态下的额外 list 查询，例如：

```text
get_collab_breakdown(view_version, leader_pass_id, context, cursor, limit, sort)
```

该查询必须：

- 请求 `context` 必须可由主 profile 的 `external_state` 无损构造，响应必须返回同一个 `external_state`。
- 支持稳定分页。
- 返回 deterministic ordering，并在请求或响应中显式声明 `sort`。
- 允许下游通过所有分页结果重算主 profile 中的 `collab_contribution`。

v1 定义以下排序值：

| sort | 语义 | 典型用途 |
| --- | --- | --- |
| `collab_pass_id_asc` | 按 RPC 输出的 canonical `collab_pass_id` 文本逐字节升序；v1 默认值，必须支持。 | 稳定全量审计、分页简单。 |
| `contribution_desc_pass_id_asc` | 按 `collab_contribution` 降序，以 canonical `collab_pass_id` 文本逐字节升序打破平局；可以支持。 | 浏览器展示最大贡献者、Leader 贡献分析。 |

无论提供哪种排序，cursor 都必须绑定 `external_state`、`leader_pass_id`、`sort` 和分页边界，不得跨历史 context 或跨排序策略复用。
实现不得使用内部 txid byte order 代替 canonical inscription-id 文本顺序。

建议 item：

```json
{
  "collab_pass_id": "txidi1",
  "collab_owner_script_hash": "...",
  "collab_owner_btc_addr": "bc1...",
  "collab_raw_energy": "1000000",
  "collab_weight_bps": 5000,
  "collab_contribution": "500000",
  "leader_ref_kind": "leader_btc_addr",
  "leader_ref_value": "bc1..."
}
```

aggregate `collab_contribution` 不得被视为不可验证黑盒。

breakdown v1 页响应必须包含 `view_version`、完整 `external_state`、`leader_pass_id`、`leader_state`、`leader_pass_kind`、`sort`、`total`、`aggregate_collab_contribution`、`limit`、`max_limit`、`next_cursor` 和 `items`。其中 aggregate 是完整结果集的总和，不是当前页小计。

# Candidate Set View

USDB-side 应提供 candidate set audit view，用于浏览器 overview、排行榜、测试和下游链调试。

v1 排序规则固定为：

```text
selection_rule = "uip-0006:effective-energy-desc-pass-id-asc:v1"
```

含义：

```text
winner = max(candidate_set.items, by effective_energy)
tie_break = smallest pass_id lexical order
```

该规则只定义 USDB-side audit view 的确定性排序。USDB 链上 payload 是否携带 candidate set、是否只携带 selected `pass_id`、是否使用 PoW threshold 验证，由 ETHW-side UIP 定义。

Candidate set view 是一等查询，不要求下游先逐个读取所有 pass profile 后自行排序。实现可以按分页返回：

```json
{
  "view_version": "uip-0006-usdb-economic-state-view:v1",
  "external_state": {},
  "selection_rule": "uip-0006:effective-energy-desc-pass-id-asc:v1",
  "total": 6000,
  "items": [],
  "limit": 100,
  "max_limit": 500,
  "next_cursor": "..."
}
```

## v1 稳定分页契约

`candidate_set_view` 和 `collab_breakdown` v1 使用 `cursor + limit`，不使用数字 `page/page_size`：

- 首次请求不携带 `cursor`，必须携带正整数 `limit`。
- 后续请求原样携带上页返回的 opaque `next_cursor`。
- 响应必须返回实际生效的 `limit`、服务上限 `max_limit` 和可空的 `next_cursor`。
- `next_cursor = null` 表示没有下一页。
- `limit > max_limit` 必须返回 `INVALID_PAGINATION`，禁止静默截断。

cursor 必须完整绑定：

- `view_version`。
- 完整 `external_state`，包括 protocol/formula/query semantics versions。
- 资源 identity：candidate set 或指定 `leader_pass_id` 的 breakdown。
- `selection_rule` 或 `sort`、全部 filter、`limit`。
- 最后一条已返回记录的确定性排序 key。

任一绑定字段变化、cursor 无法验证或 cursor 来自另一节点不兼容实现时，服务必须返回 `INVALID_PAGINATION`，禁止退回 current head 或从第一页静默重启。cursor 的字节编码、签名方式和 `max_limit` 数值属于实现细节；调用方不得解析或构造 cursor。

参考实现使用 base64url 编码的版本化 envelope，并以 domain-separated SHA-256 checksum 检测损坏、篡改和 schema drift。该编码不是协议字段，也不是授权边界；调用方只能把 cursor 当作 opaque string。参考实现固定 `max_limit = 500`。

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
- 每次查询必须在业务派生前后重建并比较同一个完整 historical state ref；若查询期间发生 reorg，必须返回对应 state mismatch，禁止把 reorg 前的 `external_state` 与 reorg 后的业务字段组合成响应。
- history retention 不足时必须返回 `HISTORY_NOT_AVAILABLE` 或 `STATE_NOT_RETAINED`。
- 所有响应必须回显已验证的 `view_version` 并返回完整 `external_state`，不得只返回裸 `resolved_height`。

# 错误语义

实现至少需要区分：

| 错误 | 触发条件 |
| --- | --- |
| `VIEW_VERSION_MISMATCH` | 不支持的 `view_version`。 |
| `PROTOCOL_VERSION_MISMATCH` | `usdb_index_protocol_version` 不匹配。 |
| `FORMULA_VERSION_MISMATCH` | `usdb_index_formula_version` 不匹配。 |
| `VERSION_MISMATCH` | balance-history API 或 query semantics version 不匹配。 |
| `SNAPSHOT_ID_MISMATCH` | `external_state.snapshot_id` 或对应 stable height 与历史 state ref 不匹配。 |
| `BLOCK_HASH_MISMATCH` | `external_state.stable_block_hash` 与历史 state ref 不匹配。 |
| `LOCAL_STATE_COMMIT_MISMATCH` | `local_state_commit` 不匹配。 |
| `SYSTEM_STATE_ID_MISMATCH` | `system_state_id` 不匹配。 |
| `HEIGHT_NOT_SYNCED` | 目标高度高于服务可查询的 durable/stable 高度。 |
| `SNAPSHOT_NOT_READY` | 服务当前没有可用于共识查询的完整状态锚点。 |
| `HISTORY_NOT_AVAILABLE` | 所需历史 context 已不可用。 |
| `STATE_NOT_RETAINED` | 本地 durable state 不再保留目标高度。 |
| `PASS_NOT_FOUND` | 目标 pass 在该 context 下不存在。 |
| `INVALID_PAGINATION` | `limit` 非法、cursor 无法验证或 cursor 与请求绑定字段不一致。 |
| `INTERNAL_INVARIANT_BROKEN` | 服务内部从同一历史状态重算出的字段彼此矛盾。 |
| `ECONOMIC_FIELD_MISMATCH` | 下游 verifier 将外部输入/承诺字段与本文 view 重算结果比较时不一致。 |

查询 RPC 的 mismatch 错误必须带 structured data，至少包含 expected state、actual state、requested height 和 canonical `mismatch_field`。protocol/formula 必须和目标高度的历史 identity 比较；禁止和当前进程常量比较。

`ECONOMIC_FIELD_MISMATCH` 不是 `get_pass_economic_profile`、`get_candidate_set_view` 或 `get_collab_breakdown` 的请求错误：这些查询没有 caller-supplied expected economic fields。它属于 ETHW validator、审计工具或其他下游 verifier 的本地校验结果，例如外部 payload 声明的 `effective_energy` 与 profile 重算值不同。若服务自己在一次查询内得到矛盾结果，应返回 `INTERNAL_INVARIANT_BROKEN`，不得冒充 caller mismatch。

# 与 ETHW 链上 Payload 的关系

USDB 链上 payload 应只携带验证旧块所需的最小 selector。validator 再使用这些 selector 调用本文定义的 USDB-side state view。

当前关系：

```text
ETHW ProfileSelectorPayload
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
    -> collab_breakdown_count / collab_breakdown query
```

因此，本文字段集合是 USDB 链上 payload 可解析状态的超集，不代表这些字段都应写入 ETHW 区块头。

# 测试要求

实现 UIP-0006 时，至少需要覆盖：

- valid profile 按历史 `external_state` 查询通过。
- BTC head 前进后旧 profile 仍按历史 context 查询通过。
- same-height reorg 后旧 `external_state` 返回 state mismatch。
- 查询派生期间 same-height state ref 变化时，post-check 返回 state mismatch。
- `raw_energy`、`collab_contribution`、`effective_energy`、`level`、`difficulty_factor_bps` 可在同一 context 下重算一致。
- collab Leader profile 可通过 breakdown 或审计查询重算 aggregate contribution。
- collab pass 不直接进入 candidate set view。
- `view_version` / `protocol_version` / `formula_version` mismatch。
- history retention 不足时返回 `HISTORY_NOT_AVAILABLE` 或 `STATE_NOT_RETAINED`。
- cursor 正常续页在 current head 前进后仍固定原 external state。
- 非法 limit、cursor 篡改、跨资源 cursor、绑定字段变化和旧 `page/page_size` 请求均 fail closed。

参考实现的 Rust/service tests 已覆盖上述 core 规则；targeted live 已覆盖 profile/candidate/breakdown、formula-version mismatch、head advance 和 same-height reorg。完整 runner 仍需执行全场景 reorg/restart 和大数据 cursor 压测后，才能声明 UIP-0006 live regression 完成。

# 后续实现议题

1. `contribution_desc_pass_id_asc` 在大数据量 cursor 分页下的数据库索引成本。
2. script hash -> BTC address 反向索引是否作为后续独立能力实现。
