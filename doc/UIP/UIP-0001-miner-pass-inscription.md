UIP: UIP-0001
Title: Miner Pass Inscription Schema
Status: Draft
Type: Standards Track
Layer: BTC Application / Consensus Input
Created: 2026-04-25
Requires: UIP-0000
Supersedes: doc/矿工证铭文协议.md field draft after activation
Activation: BTC network activation matrix

# 摘要

本文定义 USDB 矿工证铭文的标准 JSON schema。

本文把矿工证 mint 明确拆成两种互斥形态，类型由字段直接推导：

- 标准矿工证：包含 `usdb_main`，不包含 leader 绑定字段，可以作为独立挖矿身份。
- 协作矿工证：包含 `leader_pass_id` 或 `leader_btc_addr` 二选一，不包含 `usdb_main`，不能独立参与挖矿。

v1 schema 移除 `usdb_collab` 的协议语义。协作关系不再由 Leader 主动填写协作者 USDB-chain account address 表达，而由协作者在自己 mint 的矿工证中显式指定 Leader 绑定字段表达。

# 动机

早期 `doc/矿工证铭文协议.md` 草案和开发期 `USDBMint` 曾包含：

- `usdb_main`
- `usdb_collab`
- `prev`

这个设计有三个问题：

1. `usdb_collab` 只是一个 EVM 地址，不能唯一指向某一张 Leader 矿工证。
2. 由 Leader 主动指定协作者，无法表达“协作者自愿把自己的矿工证能量委托给 Leader”的链上意图。
3. 只绑定具体 pass id 虽然确定性最好，但 Leader remint 后协作者需要重新绑定；只绑定 BTC 地址虽然体验更好，但会自动跟随该地址的新 active pass。

USDB 经济模型需要的是可重放、可审计、可按历史高度验证的协作绑定关系。因此，协作者必须通过自己的 BTC 铭文显式声明 Leader，并显式选择固定 pass 绑定或地址自动跟随绑定。

# 当前实现状态

参考实现已完成 UIP-0001 v1 core 对齐：

- parser 必须接收整数 `v: 1`，`prev` 缺省为空数组，并拒绝未知字段、重复 top-level key、重复 `prev` 和开发期旧 payload。
- standard / `leader_pass_id` collab / `leader_btc_addr` collab 三种合法形态已使用互斥字段解析；BTC 地址按当前 indexer network 校验。
- `usdb_collab` 只作为 invalid schema 检测项存在，不进入 pass storage、RPC、commit mutation、control-plane mint 或前端类型。
- pass storage、查询、Rust client、CLI、control-plane 和浏览器类型均暴露 `mint_version`、`pass_kind` 与 Leader 绑定字段。
- ord、bitcoind 与 fixture source 将历史字段 `inscription_number` 统一解释为
  `pass_id` 中的 reveal-envelope index，不再使用 ord source-local global number。
- 隔离 live/regtest 已用真实 ord 铭文交叉验证 `application/json`、
  `text/plain;charset=utf-8`、raw/USDB source comparison，以及 ord/bitcoind
  primary indexer 的 canonical state/commit 一致性。

当前剩余工作是由 UIP-0008 固定公开网络 activation matrix，并单独冻结
“reveal 当下没有可用 owner”的 invalid/ignore/fail-closed 口径；纯 parser 负向组合
继续由确定性单元测试覆盖，不要求为每个无链上状态语义的 JSON 变体重复构造 live
场景。不再保留开发期兼容或迁移任务。

# 非目标

本文只定义铭文内容 schema，不完整定义：

- pass 状态机转换。
- `prev` 继承严格失败规则。
- energy 增长、继承折损和终态规则。
- collab pass 的有效性窗口、退出规则和 `effective_energy` 公式。
- reward split、分润合约或协作者收益分配。

这些内容分别由后续 UIP 定义。

# 术语

| 术语 | 含义 |
| --- | --- |
| pass | USDB 矿工证，由 BTC 铭文表达。 |
| `pass_id` | pass inscription id 的 canonical 文本表示，也是跨 UIP、RPC 和 USDB chain selector 使用的唯一 pass 标识。 |
| `pass_kind` | 由 v1 互斥字段确定的 schema 类型，只能是 `standard` 或 `collab`。 |
| standard pass | 包含 `usdb_main` 的 pass kind；具备成为 UIP-0006 `candidate_pass` 的类型资格，但仍须满足 UIP-0002 状态条件。 |
| collab pass | 包含一种 Leader 绑定字段的 pass kind；向成功解析的 Leader 提供能量，永远不能成为独立 `candidate_pass`。 |
| Leader | collab pass 所声明的协作目标角色；该名称本身不表示目标高度已经解析成功或具备 USDB chain 出块资格。 |
| `owner_script_hash` | 当前持有铭文 UTXO 的输出脚本所对应的 canonical owner identity，用于状态比较、余额和索引。 |
| `owner_btc_addr` | 能从对应输出脚本确定时使用的网络相关 BTC 地址表示，只用于输入或展示，不替代 `owner_script_hash`。 |
| owner | 未带字段名时表示 `owner_script_hash`；文档需要表达地址时必须显式写 `owner_btc_addr`。 |
| `usdb_main` | 标准矿工证绑定的 USDB-chain account address，用于 USDB chain 挖矿身份和收益接收。 |
| `leader_pass_id` | 固定 Leader 矿工证的 canonical `pass_id`。 |
| `leader_btc_addr` | Leader BTC 地址；按历史高度解析为该地址当前 active standard pass。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 版本模型

UIP-0001 定义 USDB 矿工证铭文的第一个标准协议版本：v1。

当前代码和早期文档中的开发期载荷不定义为正式协议版本。开发期字段和本地测试数据不进入 UIP-0001 的规范版本序列；在当前 dev 阶段，旧载荷和旧数据库应直接删除或重建，不设计兼容解析或迁移路径。

# Canonical Pass ID

所有 pass inscription id 必须使用同一种 canonical 文本表示：

```text
pass_id = lowercase_hex(inscription_txid) + "i" + decimal(inscription_index)
```

其中：

- `inscription_txid` 必须是 64 个 lowercase hex 字符。
- `inscription_index` 必须是 `uint32` 的十进制表示；除数值 `0` 本身外禁止前导零。
- `leader_pass_id`、`prev[]`、RPC `pass_id`、`candidate_set_view` tie-break 和 USDB chain `ProfileSelectorPayload` 的链外表示必须使用同一 canonical encoding。
- pass identity 的相等比较必须比较解析后的 inscription id；规范实现必须拒绝非 canonical 文本，不能让大小写或前导零别名绕过 duplicate 检测。
- 任何按 `pass_id` 的 lexical ordering 都表示按 canonical ASCII 字节逐字节升序。

## v1 schema

v1 schema 必须包含：

- `p`
- `op`
- `v`

并且必须满足 standard pass 或 collab pass 的字段互斥规则。

其中：

- `p` 必须为 `"usdb"`。
- `op` 必须为 `"mint"`。
- `v` 必须为整数 `1`。
- standard pass 必须包含 `usdb_main`，且禁止包含 `leader_pass_id` 和 `leader_btc_addr`。
- collab pass 必须在 `leader_pass_id` 和 `leader_btc_addr` 中二选一，且禁止包含 `usdb_main`。

# v1 字段定义

| 字段 | 类型 | 必填 | 适用类型 | 说明 |
| --- | --- | --- | --- | --- |
| `p` | string | 是 | all | 固定为 `"usdb"`。 |
| `op` | string | 是 | all | 固定为 `"mint"`。 |
| `v` | integer | 是 | all | 当前为 `1`。 |
| `usdb_main` | string | 条件必填 | standard | 标准矿工证的 EVM 地址。 |
| `leader_pass_id` | string | 条件必填 | collab | 固定 Leader 矿工证 canonical `pass_id`。 |
| `leader_btc_addr` | string | 条件必填 | collab | Leader BTC 地址，必须属于当前 BTC 网络。 |
| `prev` | string[] | 否 | all | 被继承矿工证 canonical `pass_id` 列表；缺省等价于空数组。 |

## 字段互斥规则

### standard pass

当铭文包含 `usdb_main` 且不包含任何 leader 绑定字段时，该铭文是 standard pass：

- `usdb_main` 必须存在，且必须是合法 EVM 地址。
- `leader_pass_id` 禁止存在。
- `leader_btc_addr` 禁止存在。
- `usdb_collab` 禁止存在。
- 该 pass 具备成为 UIP-0006 `candidate_pass` 的 `pass_kind` 资格；只有在目标 `external_state` 下处于 UIP-0002 `Active` 状态时才实际进入 `candidate_set_view`。

### collab pass

当铭文包含 `leader_pass_id` 或 `leader_btc_addr`，且不包含 `usdb_main` 时，该铭文是 collab pass：

- `leader_pass_id` 与 `leader_btc_addr` 必须二选一，禁止同时存在。
- `leader_pass_id` 存在时，必须是合法且 canonical 的 `pass_id`。
- `leader_btc_addr` 存在时，必须是当前 BTC 网络上的合法地址。
- `usdb_main` 禁止存在。
- `usdb_collab` 禁止存在。
- 该 pass 永远禁止成为独立 `candidate_pass`，即使自身状态为 `Active` 且拥有非零 `raw_energy`。
- 该 pass 的有效能量只能归入其 Leader 的 `effective_energy` 计算。

协作矿工证仍然是 BTC owner 持有的 pass 资产，但在绑定有效期间，其挖矿身份与收益接收口径必须使用 Leader 的 `usdb_main`。

# JSON 示例

## standard pass

```json
{
  "p": "usdb",
  "op": "mint",
  "v": 1,
  "usdb_main": "0x1111111111111111111111111111111111111111",
  "prev": []
}
```

## collab pass with fixed Leader pass

```json
{
  "p": "usdb",
  "op": "mint",
  "v": 1,
  "leader_pass_id": "1111111111111111111111111111111111111111111111111111111111111111i0",
  "prev": []
}
```

## collab pass with Leader BTC address

```json
{
  "p": "usdb",
  "op": "mint",
  "v": 1,
  "leader_btc_addr": "bc1qexampleleaderaddressxxxxxxxxxxxxxxxxxxxxxx",
  "prev": []
}
```

# Leader 绑定模式

协作绑定支持两类规范字段：

| 候选字段 | 优点 | 问题 | 结论 |
| --- | --- | --- | --- |
| `leader_usdb_main` | 与 USDB chain 挖矿身份直接相关 | USDB-chain account address 可被多个 pass 复用，Leader remint 后地址不一定唯一；历史高度上难以反查具体 pass | 不推荐 |
| `leader_pass_id` | inscription id 不可变，唯一、可索引、可历史重放 | Leader remint 后不会自动跟随新 pass | 支持，适合固定 pass 绑定 |
| `leader_btc_addr` | Leader remint 后可自动跟随该地址的新 active standard pass | 协作者会自动接受该地址后续 active pass 和 `usdb_main` 变化 | 支持，适合地址身份绑定 |

## `leader_pass_id` 绑定

`leader_pass_id` 表示协作者绑定一张具体 Leader pass。

在高度 `h` 解析时：

```text
leader = pass_by_inscription_id(leader_pass_id, h)
```

只有当该 pass 在高度 `h` 是 active standard pass 时，collab pass 才能向其贡献有效能量。Leader remint 后不会自动跟随新 pass；协作者如需切换，必须重新 mint 或 remint 自己的 collab pass。

## `leader_btc_addr` 绑定

`leader_btc_addr` 表示协作者绑定一个 BTC 地址在历史高度 `h` 的 active standard pass。

在高度 `h` 解析时：

```text
leader = active_standard_pass_by_owner(normalize_btc_addr(leader_btc_addr), h)
```

只有当该地址在高度 `h` 能解析到唯一 active standard pass 时，collab pass 才能向其贡献有效能量。如果该地址暂时没有 active standard pass，该 collab pass 在该高度不贡献有效能量。

该模式允许 Leader 地址重新铸造新 pass 后自动继承协作者绑定关系。相应地，协作者也显式接受该地址后续 active pass 的 `usdb_main` 变化和其他 Leader 侧状态变化。

# `usdb_collab` 处理

v1 新铭文禁止使用 `usdb_collab`。

原因：

- `usdb_collab` 只能表达一个 EVM 地址，不能表达协作矿工证的链上资产身份。
- `usdb_collab` 由 Leader 主动填写，缺少协作者主动授权语义。
- `usdb_collab` 与 leader 绑定字段并存会产生双重解释路径。

激活后，若 v1 铭文包含 `usdb_collab`，索引器必须将该铭文判为 invalid mint。

# `prev` 默认值

v1 中 `prev` 是可选字段。

规则：

- 缺失 `prev` 等价于 `prev: []`。
- `prev` 存在时必须是数组。
- 数组元素必须是合法且 canonical 的 `pass_id` 字符串。
- 同一个 `prev` 数组中禁止出现重复 pass identity；非 canonical 别名必须直接判 invalid，不能参与去重。

`prev` 指向对象是否存在、是否可继承、是否已被消费，由 UIP-0002 和 UIP-0003 定义。

# unknown fields 与重复字段

v1 schema 应该采用严格解析。

规则：

- 未定义字段必须导致 invalid mint。
- 重复 JSON key 必须导致 invalid mint。
- 字段类型不匹配必须导致 invalid mint。
- 实现必须在依据 `p` / `op` 进行协议分类前扫描顶层 key。duplicate `p` / `op`
  中只要任一 `p` 为 `usdb` 且任一 `op` 为 `mint`，该 payload 就属于 USDB
  mint candidate，并必须因重复字段判 invalid；禁止使用 first-value 或
  last-value 结果将其降级为非 USDB inscription。

严格解析的目标是避免不同 JSON parser 对重复字段或未知字段产生不同解释。

# content-type

索引器必须至少接受 UTF-8 JSON 内容。

推荐 content-type：

```text
application/json;charset=utf-8
```

content-type 只作为内容提示，不参与 mint 共识分类。对同一 UTF-8 JSON body，source
报告 `application/json`、`text/plain` 或无法提供可靠 content-type 时，索引器必须进入
同一 strict schema classifier 并得到相同结果。content-type 不得绕过 schema 校验。

# 激活矩阵

UIP-0001 主要影响 BTC 侧铭文解析和由 BTC 派生的 pass 状态。USDB chain 侧只消费索引结果，不直接解析 BTC inscription content。

| Chain | Network Type | Network ID | Activation Anchor | Activation Value | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| BTC | regtest | btc-regtest | btc_height | TBD | Planned | 本地回归测试可最先启用 v1 strict schema。 |
| BTC | testnet | btc-testnet4 | btc_height | TBD | Planned | 公开测试网验证 parser 和历史重放。 |
| BTC | mainnet | btc-mainnet | btc_height | TBD | Planned | 主网 v1 schema 激活高度。 |
| USDB | devnet | usdb-devnet-<name> | governance | TBD | Planned | USDB chain 侧切换到消费 v1 pass snapshot。 |
| USDB | mainnet | usdb-mainnet | governance | TBD | Planned | USDB 主网接受 v1 pass 语义的治理激活点。 |

未列出的网络不得默认激活 UIP-0001。

# 激活与开发期实现

UIP-0001 只定义标准 v1 schema。

当前 dev 实现直接按 v1 strict schema 解析 USDB mint。仓库内历史测试数据、开发期 inscription payload、旧数据库或临时 parser 字段都只属于 pre-standard implementation draft，不构成需要兼容的协议版本。

dev 阶段实现要求：

- 不为缺失 `v`、旧字段或旧互斥规则的 payload 提供兼容解析路径。
- 不为旧 `miner_passes` schema 提供数据库迁移路径；本地开发和测试环境需要时应删除旧数据库并按 v1 schema 重建。
- `usdb_collab` 仅作为 v1 invalid 条件保留在 parser，不进入 pass 状态、RPC 响应或 commit mutation schema。
- regtest、live test、fixture 和 simulator 生成的新 mint payload 必须直接使用 v1 schema。

## 激活后

激活后，新 mint 必须满足 v1 schema。

激活后：

- 缺失 `v` 的新 mint 必须判为 invalid。
- `v != 1` 的新 mint 必须判为 invalid，除非后续 UIP 激活新版本。
- 包含 `usdb_collab` 的新 mint 必须判为 invalid。
- 同时包含 `usdb_main` 和任一 leader 绑定字段的新 mint 必须判为 invalid。
- 同时包含 `leader_pass_id` 和 `leader_btc_addr` 的新 mint 必须判为 invalid。
- 同时缺失 `usdb_main`、`leader_pass_id` 和 `leader_btc_addr` 的新 mint 必须判为 invalid。

历史回放必须按该网络在对应高度的激活状态解释。旧开发期数据不参与标准历史回放；dev/test 环境需要以 v1 数据重建。

# 协作矿工证的设计约束

协作矿工证的核心语义是：

```text
collab_pass -> leader_pass_id | leader_btc_addr -> leader.usdb_main
```

因此：

- 协作者通过自己的 BTC mint 显式选择 Leader。
- Leader 不再通过 `usdb_collab` 主动指定协作者。
- 协作关系的链上授权来自 collab pass owner。
- collab pass 不再携带自己的 `usdb_main`。
- collab pass 不得成为独立 `candidate_pass`。
- collab pass 的 raw energy 可以被索引用于审计，但参与挖矿时必须只计入 Leader 的 `effective_energy`。
- `leader_pass_id` 模式绑定具体 pass，`leader_btc_addr` 模式绑定地址在历史高度上的 active standard pass。

这可以避免同一份能量同时作为 collab 加成和独立矿工能量被重复使用。

# 与后续 UIP 的边界

## UIP-0002

UIP-0002 定义：

- standard pass 和 collab pass 的状态机差异。
- Leader 失效不直接改变 collab pass 状态，只影响 UIP-0004 contribution。
- collab pass 可以通过新 mint + `prev` remint 为 standard 或新 collab pass。
- `leader_pass_id` 不存在、非 Active 或非 standard 时，该 collab mint 为 `Invalid`。
- `leader_btc_addr` 在 mint 时只校验当前 BTC network；每个历史高度动态解析，没有 active standard pass 时 contribution 为 `0`。

上述规则分别由 UIP-0002 状态机和 UIP-0004 derived view 执行。

## UIP-0003

UIP-0003 定义：

- collab pass 的 raw energy 如何增长。
- collab pass remint 或退出使用统一 `INHERIT_DISCOUNT_BPS = 500` 折损。
- collab pass 退出不增加额外 cooldown 或专用退出 penalty。

## UIP-0004

UIP-0004 定义：

- Leader 在目标历史高度必须是 Active standard pass。
- `effective_energy` 公式。
- collab energy 权重。
- fixed pass 与 address binding 在 leader remint、transfer、burn 后的不同解析结果。
- `leader_btc_addr` 自动跟随不增加 cooldown 或延迟生效。
- collab energy 防双计数规则。

# 实现影响

参考实现已对齐以下入口：

- `src/btc/usdb-indexer/src/index/content.rs`
- `src/btc/usdb-indexer/src/inscription/source.rs`
- `src/btc/usdb-indexer/src/index/indexer.rs`
- `src/btc/usdb-indexer/src/index/pass.rs`
- `src/btc/usdb-indexer/src/storage/pass.rs`

effective energy 不写入本 schema 或 pass mint storage，由 UIP-0004 / UIP-0006 在查询时派生。

# 测试要求

最小测试集合：

- v1 standard mint valid。
- v1 collab mint with `leader_pass_id` valid。
- v1 collab mint with `leader_btc_addr` valid。
- v1 missing `prev` 等价于空数组。
- v1 invalid `usdb_main`。
- v1 invalid `leader_pass_id`。
- v1 non-canonical `leader_pass_id` invalid。
- v1 invalid `leader_btc_addr` for active BTC network。
- v1 同时包含 `usdb_main` 和任一 leader 绑定字段 invalid。
- v1 同时包含 `leader_pass_id` 和 `leader_btc_addr` invalid。
- v1 同时缺失 `usdb_main`、`leader_pass_id` 和 `leader_btc_addr` invalid。
- v1 包含 `usdb_collab` invalid。
- v1 unknown field invalid。
- v1 duplicate key invalid。
- v1 non-canonical `prev` pass id invalid。
- pre-standard development payload 不作为正式协议版本参与标准解析。

参考实现的 parser、source comparison、indexer behavior 和 control-plane mint 测试已覆盖上述 core 规则；隔离 live/regtest 已复核真实 ord body、不同 content-type 及 ord/bitcoind source 的一致性。

# 安全考虑

## 协作者授权

协作关系必须由 collab pass owner 自己 mint 表达，避免 Leader 单方面指定他人作为协作者。

## 防双计数

collab pass 不能同时作为 `candidate_pass` 和 Leader 加成来源。

## 引用模式

`leader_pass_id` 是稳定 inscription id，适合固定 pass 绑定。

`leader_btc_addr` 是地址身份绑定，适合 Leader 地址 remint 后自动跟随。该模式必须按历史高度解析，且协作者必须接受该地址后续 active standard pass 的 `usdb_main` 变化。

## 历史回放

所有 parser 行为必须按 mint 高度和网络激活状态解释，不能用未来激活规则重算历史。当前 dev 实现不保留开发期兼容逻辑；旧 dev 数据应在启用 v1 前丢弃或重建。

# 后续 UIP 依赖

- `leader_pass_id` 的 mint-time Leader 有效性、同 block ordering 口径，以及 `leader_btc_addr` 的动态解析规则由 UIP-0002 定义。
- collab pass 的 `effective_energy` 归属、防双计数和转换后的 derived energy 影响由 UIP-0004 定义；`candidate_pass` 和 `candidate_set_view` 由 UIP-0006 定义。
- collab pass 转 standard pass 的继承折损使用 UIP-0003 的通用 `prev` 继承规则，不在 UIP-0001 分配额外退出折损率。
- 正式 mainnet 的具体 `network_id` 由 UIP-0008/UIP-0009 冻结，且必须使用 `usdb-*` 命名空间。

# 下一步

1. 在 UIP-0008 activation matrix 中确认正式激活高度和稳定 `network_id`。
2. 后续只为新增的链上解析边界增加 live 场景；纯 JSON schema 组合保留在确定性 parser 测试中。
