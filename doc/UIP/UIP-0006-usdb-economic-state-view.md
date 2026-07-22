UIP: UIP-0006
Title: USDB Economic State View
Status: Draft
Type: Standards Track
Layer: BTC-side USDB Indexer RPC / BTC Application Query
Created: 2026-04-25
Requires: UIP-0000, UIP-0002, UIP-0003, UIP-0004, UIP-0005
Activation: USDB index protocol and formula version; development networks activate from height 0 after implementation

# 摘要

本文定义 `usdb-indexer` 对外提供的经济状态视图和审计视图。

它不是 USDB chain 区块头里的链上 payload 编码，而是下游链、validator、浏览器和审计工具在某一 BTC 历史 context 下可查询、可重放、可比对的 BTC-side USDB state view。

核心规则：

- view 必须绑定一个可重放的 BTC-side USDB `external_state`。
- view 可以返回 `raw_energy`、`collab_contribution`、`effective_energy`、`level`、`difficulty_factor_bps` 和 `collab_breakdown_count`。
- 完整 `collab_breakdown` 通过同一历史 context 下的确定性分页查询提供，不要求内联到主 profile。
- energy 类字段必须使用 UIP-0003 的 `uint128` canonical decimal string。
- `level` 和 `difficulty_factor_bps` 是基于 `effective_energy` 的查询时派生值，不要求持久化。
- `leader_eligible`、USDB chain `base_difficulty`、USDB chain `real_difficulty`、reward rule 和 header payload encoding 不属于本文。
- USDB chain 上共识 payload 应消费本文定义的 state view，但不得把本文的完整审计字段集合等同于链上 payload 字节。

# 动机

UIP-0003、UIP-0004 和 UIP-0005 分别定义了：

- raw energy 和继承。
- collab contribution 和 effective energy。
- level 和 difficulty factor。

这些值需要通过 `usdb-indexer` 形成统一的历史查询口径。否则 USDB validator、测试脚本、浏览器和审计工具会各自拼接 RPC 字段，容易产生以下问题：

- current head 查询被误用于历史块验证。
- raw energy、collab contribution、effective energy 混用。
- USDB chain policy 字段反向污染 BTC-side 派生状态。
- 链上 payload 字段和审计明细字段边界不清。

本文把 BTC-side USDB 能提供的完整经济状态视图单独协议化。USDB chain payload 只需要引用其中的最小状态选择器，并在验证时按本文规则重算或查询。

# 当前实现状态

参考实现已经完成 v1 historical context / version 校验基础、`get_pass_economic_profile`、`get_candidate_set_view` 和 `get_collab_breakdown` 的核心派生逻辑，以及 `candidate_set_view` / `collab_breakdown` 的 opaque `cursor + limit` 稳定分页。

`get_pass_economic_profile` 当前已满足：

- 必填 `view_version`，并返回完整历史 `external_state`。
- active standard 的 raw / collab / effective energy、level、difficulty factor 和 breakdown count 运行时派生。
- collab 与 non-active pass 的 effective energy 状态边界。
- invalid pass 无 energy row 时的 canonical 零值合成。
- `PASS_NOT_FOUND` 与 non-invalid 缺 raw energy 的 `INTERNAL_INVARIANT_BROKEN` 错误边界。
- BTC head 前进后按旧 `external_state` 重放同一 profile。
- 派生前后重建并复验完整 historical state ref，同高度 reorg 期间不返回混合状态响应。

`candidate_set_view` / `collab_breakdown` 当前已直接使用本文固定的 `cursor + limit`，不保留旧 `page/page_size` 双栈。参考实现的 `max_limit` 为 500，cursor 绑定完整 external state、resource/query 条件、limit 和 continuation key。

参考实现也已补齐跨组件调用面：Rust typed client、`usdb-indexer-cli` 和 control-plane proxy 都能直接调用 historical state ref、profile、`candidate_set_view` 和 `collab_breakdown`。`get_rpc_info` 现在显式声明 view version、`candidate_set_view` ordering rule、max limit 和四个必需 feature；control-plane 只有在 service/API/version/rule/features 全部匹配时才声明 UIP-0006 economic state view 可用。

happy path、same-height reorg、three-pass `candidate_set_view` 和 three-collab `collab_breakdown` 已通过真实 ord targeted smoke。deterministic live/regtest 与 `100 / 1K / 10K` 规模矩阵也已完成；后续容量工作集中在并发、冷缓存、100K 以上规模和长时 soak。

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

能力字段 `candidate_set_selection_rule` 沿用 v1 RPC 名称，但其规范语义只是 `candidate_set_view` 的 ordering contract，不是 USDB block-selection policy。

# 非目标

本文不定义：

- USDB chain `header.Extra` 二进制编码。
- USDB chain `ProfileSelectorPayload` 字段布局。
- USDB chain `base_difficulty` 来源、PoW target 编码或 chain weight 规则。
- USDB block reward、fee split、uncle reward、CoinBase 或分红池规则。
- pass 铭文 schema、pass 状态机、energy 公式本身。
- Leader eligibility 的报价窗口和 USDB chain 出块历史策略。

# 术语

| 术语 | 含义 |
| --- | --- |
| `query_context` | 调用方请求中的历史查询约束，由 `requested_height` 和可选 `expected_state` 组成；RPC 字段名为 `context`。 |
| `expected_state` | 调用方对目标高度历史 identity 的预期字段集合；服务必须逐字段验证，不能用 current head 或当前二进制常量替代。 |
| `external_state` | 服务完成历史解析和校验后返回的完整、不可变 BTC-side USDB state identity；它是查询结果锚点，不是调用方可自由构造的轻量 selector。 |
| historical context | `query_context`、解析出的目标高度及其 `external_state` 的统称；规范字段必须使用前述精确名称，不能只写裸 `context`。 |
| `economic_state_view` | `usdb-indexer` 在一个 `external_state` 下返回的经济状态视图。 |
| `pass_economic_profile` | 某张 pass 在指定历史 context 下的 pass snapshot + energy profile。 |
| `candidate_pass` | 在同一 `external_state` 下状态为 `Active` 且 `pass_kind = standard` 的 pass；成员资格与 energy 是否为零无关。 |
| `candidate_set_view` | 同一 `external_state` 下全部 `candidate_pass` 的确定性分页审计排序；不等同于 USDB chain 上 payload 或最终出块选择。 |
| `top_ranked_candidate` | 按 `candidate_set_view.selection_rule` 排序后的第一项；只表示审计排序首项。 |
| `collab_breakdown` | 同一 `external_state` 下，向一个 `resolved_leader` 贡献能量的 active collab pass 明细和完整 aggregate。 |
| `selected_pass` | UIP-0007 `ProfileSelectorPayload` 为一个具体 USDB chain 区块声明使用的 pass；不要求等于 `top_ranked_candidate`。 |
| `resolved_profile` | 下游 validator 根据链上 payload 反查本文 state view 后得到的重算结果。 |
| raw energy leaderboard | 面向前端/浏览器的 raw-energy 展示榜单；不是 `candidate_set_view`，也不得作为 USDB chain 选择规则。 |

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
- `candidate_set_view` 排序规则。
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

`external_state` 必须包含足够信息，使实现可以构造等价的 `query_context` 并重放 BTC 历史查询。`ConsensusQueryContext` 和 `HistoricalStateRefInfo` 是参考实现类型名，不是额外的协议概念。

v1 固定字段：

| 字段 | 类型 | 必须 | 说明 |
| --- | --- | --- | --- |
| `btc_height` | integer | 是 | 查询对应的 BTC 高度。 |
| `snapshot_id` | string | 是 | upstream balance-history consensus snapshot id。 |
| `stable_block_hash` | string | 是 | `btc_height` 对应的 stable BTC block hash。 |
| `local_state_commit` | string | 是 | usdb-indexer local durable state commit。 |
| `system_state_id` | string | 是 | 下游链消费的顶层 BTC-side USDB system state id。 |
| `balance_history_api_version` | string | 是 | balance-history 对外 API 版本。 |
| `balance_history_semantics_version` | string | 是 | balance-history 历史查询语义版本。 |
| `activation_registry_id` | string | 是 | 节点为当前 BTC source network 内置的 UIP-0008 registry canonical hash。 |
| `active_version_set` | object | 是 | `btc_height` 对应的完整 UIP-0008 active version set。 |
| `active_version_set_id` | string | 是 | `active_version_set` canonical encoding 的 hash，并由 `local_state_commit` 承诺。 |

`external_state` 必须由目标高度的 durable historical state reference 构造，禁止拿 current-head version set 覆盖历史 identity。`active_version_set_id` 必须可由返回的 `active_version_set` 重算，并与 `local_state_commit` 使用的 id 一致。最小链上 payload 可以只携带 `btc_height`、`snapshot_id`、`system_state_id` 和业务对象 id；所有 UIP-0006 economic view 响应必须返回上表完整字段。只有后续协议明确声明为 selector-only 的轻量接口才可以省略字段。

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
      "activation_registry_id": "...",
      "active_version_set_id": "..."
    }
  }
}
```

RPC JSON 字段 `context` 表示本文的 `query_context`。`block_height` 与 `context.requested_height` 同时存在时必须相等。`context` 可以省略；服务仍必须把最终解析高度的完整历史 identity 返回为 `external_state`。一旦提供 `expected_state`，服务必须逐字段按目标高度的历史 identity 校验，禁止与当前二进制常量或 current head 比较。

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
    "activation_registry_id": "...",
    "active_version_set": {
      "inscription_schema_version": "uip-0001-miner-pass-inscription:v1",
      "pass_state_machine_version": "uip-0002-pass-state-machine:v1",
      "energy_formula_version": "uip-0003-pass-energy-formula:v1",
      "effective_energy_formula_version": "uip-0004-collab-leader-effective-energy:v1",
      "level_formula_version": "uip-0005-level-and-real-difficulty:v1",
      "query_semantics_version": "uip-0006-economic-query-semantics:v1",
      "state_view_version": "uip-0006-usdb-economic-state-view:v1",
      "commit_protocol_version": "uip-0008-usdb-local-state-commit:v1",
      "balance_history_semantics_version": "balance-snapshot-at-or-before:v1"
    },
    "active_version_set_id": "01d1d45f342994690d8ae27ac3d8538ad31e5f81f8e948c838067b3b52f94691"
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
| `pass_id` | string | UIP-0001 | canonical pass inscription id。 |
| `owner_script_hash` | string | UIP-0001 / pass snapshot | 当前 historical context 下的 canonical owner identity，用于比较和索引。 |
| `owner_btc_addr` | string | UIP-0001 / pass snapshot | 当前 historical context 下可展示的 BTC address；当实现能确定 address 时应该返回。 |
| `state` | string | UIP-0002 | `active` / `dormant` / `consumed` / `burned` / `invalid`。 |
| `pass_kind` | string | UIP-0001 | `standard` / `collab`。 |
| `raw_energy` | decimal string | UIP-0003 | pass 自身 raw energy。 |
| `collab_contribution` | decimal string | UIP-0004 | 作为 Leader 获得的协作贡献。 |
| `effective_energy` | decimal string | UIP-0004 | BTC-side nominal 派生值；Active standard pass 返回饱和的 `raw_energy + collab_contribution`，collab 或 non-active pass 返回 `0`。 |
| `level` | integer | UIP-0005 | 从 `effective_energy` 动态派生的 nominal level。 |
| `difficulty_factor_bps` | integer | UIP-0005 | 从 nominal `level` 动态派生的 nominal factor。 |
| `collab_breakdown_count` | integer | UIP-0004 | 当前 `external_state` 下贡献给该 `resolved_leader` 的 collab pass 数量。 |

## Owner 表示

owner 的 canonical 表示必须是 script hash 或等价确定性 owner id。该字段用于：

- 单 owner 单 active pass 校验。
- history query 比对。
- `candidate_set_view` 聚合和索引。

当实现能够从 pass satpoint 或历史输出脚本明确得到 BTC address 时，profile 应该同时返回 `owner_btc_addr`。address 可以推导出 script hash，但 script hash 反查 address 需要额外上下文，因此浏览器和审计视图保留 address display 字段是合理的。

如果存在无法唯一编码为标准 BTC address 的 script，`owner_btc_addr` 可以为空，但 `owner_script_hash` 必须存在。

从 `owner_script_hash` 反查 `owner_btc_addr` 不属于 UIP-0006 的核心要求。若后续需要一等反向查询能力，应通过独立索引器或后续 UIP 定义 script hash -> address 映射、快照语义和历史保留规则。缺少该反向索引不得阻塞 core profile、`candidate_set_view` 或 USDB chain reward replay。

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
- 如果处于 `active`，就是本文定义的 `candidate_pass`；即使 `effective_energy = 0` 也仍在集合中。

collab pass:

- 可以拥有自身 `raw_energy`。
- 永远不能成为独立 `candidate_pass`。
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

BTC-side USDB 应提供 `candidate_set_view`，用于浏览器 overview、审计排序、测试和下游链调试。

在一个固定 `external_state = S` 下，成员资格定义为：

```text
candidate_pass(pass, S)
    = pass.state(S.btc_height) == Active
      and pass.pass_kind == standard
```

因此：

- 所有且仅有 `Active` standard pass 进入 `candidate_set_view`。
- collab、Dormant、Consumed、Burned 和 Invalid pass 必须排除。
- 成员资格不要求 `raw_energy`、`collab_contribution`、`effective_energy` 或 `level` 大于零。
- UIP-0014 quote activity、`leader_eligible` 和 `candidate_energy` 不改变本文集合成员资格；它们属于 USDB-chain policy。
- 同一页及所有 continuation page 的每一项必须来自同一个 `external_state`。

v1 排序规则固定为：

```text
selection_rule = "uip-0006:effective-energy-desc-pass-id-asc:v1"
```

含义：

```text
top_ranked_candidate = first(candidate_set.items)
tie_break = smallest pass_id lexical order
```

其中 `pass_id` 必须使用 UIP-0001 canonical encoding，lexical order 表示 ASCII 字节逐字节升序。

字段名 `selection_rule` 只表示本 view 的 ordering contract。该规则不产生共识 `winner`，也不声明 `top_ranked_candidate` 已赢得 PoW 竞争。某个 USDB chain 区块的 `selected_pass` 由 UIP-0007 `ProfileSelectorPayload` 显式声明；其 quote activity、`candidate_energy` 和 PoW threshold 验证由 USDB-chain UIP 定义。

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
- 资源 identity：`candidate_set_view` 或指定 `leader_pass_id` 的 `collab_breakdown`。
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
| `ACTIVE_VERSION_SET_MISMATCH` | caller 固定的 `active_version_set_id`、返回 set 的重算 id 或 historical lookup 不一致。 |
| `ACTIVATION_RECORD_NOT_FOUND` | 目标 network / height 缺少必需 activation record。 |
| `ACTIVATION_RECORD_CONFLICT` | activation registry 在目标 context 存在冲突。 |
| `VERSION_NOT_SUPPORTED` | active version set 包含本地实现不支持的版本。 |
| `FORMULA_VERSION_MISMATCH` | active formula version 与派生字段实际使用的公式版本不一致。 |
| `COMMIT_PROTOCOL_VERSION_MISMATCH` | active commit protocol 与本地 state commit encoder 不一致。 |
| `VERSION_MISMATCH` | balance-history API 或 query semantics version 不匹配。 |
| `SNAPSHOT_ID_MISMATCH` | `external_state.snapshot_id` 或对应 stable height 与历史 state ref 不匹配。 |
| `BLOCK_HASH_MISMATCH` | `external_state.stable_block_hash` 与历史 state ref 不匹配。 |
| `LOCAL_STATE_COMMIT_MISMATCH` | `local_state_commit` 不匹配。 |
| `SYSTEM_STATE_ID_MISMATCH` | `system_state_id` 不匹配。 |
| `HEIGHT_NOT_SYNCED` | 目标高度高于服务可查询的 durable/stable 高度。 |
| `SNAPSHOT_NOT_READY` | 服务当前没有可用于共识查询的完整状态锚点。 |
| `HISTORY_NOT_AVAILABLE` | 所需历史 context 已不可用。 |
| `STATE_NOT_RETAINED` | 本地 durable state 不再保留目标高度。 |
| `PASS_NOT_FOUND` | 目标 pass 在该 `query_context` 解析出的 `external_state` 下不存在。 |
| `INVALID_PAGINATION` | `limit` 非法、cursor 无法验证或 cursor 与请求绑定字段不一致。 |
| `INTERNAL_INVARIANT_BROKEN` | 服务内部从同一历史状态重算出的字段彼此矛盾。 |
| `ECONOMIC_FIELD_MISMATCH` | 下游 verifier 将外部输入/承诺字段与本文 view 重算结果比较时不一致。 |

查询 RPC 的 mismatch 错误必须带 structured data，至少包含 expected state、actual state、requested height 和 canonical `mismatch_field`。protocol/formula 必须和目标高度的历史 identity 比较；禁止和当前进程常量比较。

`ECONOMIC_FIELD_MISMATCH` 不是 `get_pass_economic_profile`、`get_candidate_set_view` 或 `get_collab_breakdown` 的请求错误：这些查询没有 caller-supplied expected economic fields。它属于 USDB chain validator、审计工具或其他下游 verifier 的本地校验结果，例如链外 validator test envelope 声明的 `effective_energy` 与 profile 重算值不同。若服务自己在一次查询内得到矛盾结果，应返回 `INTERNAL_INVARIANT_BROKEN`，不得冒充 caller mismatch。

# 与 USDB chain 上 Payload 的关系

USDB chain payload 应只携带验证旧块所需的最小 selector。validator 再使用这些 selector 调用本文定义的 BTC-side USDB state view。

当前关系：

```text
USDB chain ProfileSelectorPayload
    -> btc_height
    -> snapshot_id
    -> system_state_id
    -> selected_pass = pass_id
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

因此，本文字段集合是 USDB chain payload 可解析状态的超集，不代表这些字段都应写入 USDB chain 区块头。`selected_pass` 只需是目标 `external_state` 下的 `candidate_pass`；UIP-0007 不要求它等于本文的 `top_ranked_candidate`。

# 测试要求

实现 UIP-0006 时，至少需要覆盖：

- valid profile 按历史 `external_state` 查询通过。
- BTC head 前进后旧 profile 仍按历史 context 查询通过。
- same-height reorg 后旧 `external_state` 返回 state mismatch。
- 查询派生期间 same-height state ref 变化时，post-check 返回 state mismatch。
- `raw_energy`、`collab_contribution`、`effective_energy`、`level`、`difficulty_factor_bps` 可在同一 context 下重算一致。
- collab Leader profile 可通过 breakdown 或审计查询重算 aggregate contribution。
- 所有 Active standard pass 都进入 `candidate_set_view`，包括 `effective_energy = 0` 的 pass。
- collab 和所有 non-Active pass 不进入 `candidate_set_view`。
- `top_ranked_candidate` 按 `effective_energy DESC, canonical pass_id ASC` 得出，但不代表 USDB block-selection 或 PoW 验证结果。
- `view_version` / `protocol_version` / `formula_version` mismatch。
- history retention 不足时返回 `HISTORY_NOT_AVAILABLE` 或 `STATE_NOT_RETAINED`。
- cursor 正常续页在 current head 前进后仍固定原 external state。
- 非法 limit、cursor 篡改、跨资源 cursor、绑定字段变化和旧 `page/page_size` 请求均 fail closed。

参考实现的 Rust/service tests 已覆盖上述 core 规则；deterministic live/regtest 矩阵已覆盖 profile / `candidate_set_view` / `collab_breakdown`、版本 mismatch、head advance、same-height/multi-block reorg、restart 和 historical context。`100 / 1K / 10K` standard + collab 规模矩阵已覆盖 cursor 全分页、两种 breakdown 排序、冻结状态重放和 restart 重放；结果与测量边界见 `doc/usdb-indexer/usdb-indexer-economic-view-scale-evaluation.md`。完整 300-block 随机 world-sim、并发/冷缓存与长时 soak 仍属于后续容量评估。

# 后续实现议题

1. cold cache、并发、多个 historical context 和 100K 以上规模下的物理 I/O/缓存容量评估。
2. script hash -> BTC address 反向索引是否作为后续独立能力实现。
