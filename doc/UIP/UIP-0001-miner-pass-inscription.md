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

- 标准矿工证：包含 `eth_main`，不包含 leader 绑定字段，可以作为独立挖矿身份。
- 协作矿工证：包含 `leader_pass_id` 或 `leader_btc_addr` 二选一，不包含 `eth_main`，不能独立参与挖矿。

本文建议在 v1 schema 中移除 `eth_collab` 的新协议语义。协作关系不再由 Leader 主动填写协作者 ETH 地址表达，而由协作者在自己 mint 的矿工证中显式指定 Leader 绑定字段表达。

# 动机

当前 `doc/矿工证铭文协议.md` 和实现中的 `USDBMint` 仍包含：

- `eth_main`
- `eth_collab`
- `prev`

这个设计有三个问题：

1. `eth_collab` 只是一个 EVM 地址，不能唯一指向某一张 Leader 矿工证。
2. 由 Leader 主动指定协作者，无法表达“协作者自愿把自己的矿工证能量委托给 Leader”的链上意图。
3. 只绑定具体 pass id 虽然确定性最好，但 Leader remint 后协作者需要重新绑定；只绑定 BTC 地址虽然体验更好，但会自动跟随该地址的新 active pass。

USDB 经济模型需要的是可重放、可审计、可按历史高度验证的协作绑定关系。因此，协作者必须通过自己的 BTC 铭文显式声明 Leader，并显式选择固定 pass 绑定或地址自动跟随绑定。

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
| standard pass | 标准矿工证，可独立参与挖矿候选集合。 |
| collab pass | 协作矿工证，向指定 Leader 提供能量，不可独立参与挖矿。 |
| Leader | 被协作矿工证引用的标准矿工证。 |
| owner | 当前持有该铭文 UTXO 的 BTC 地址语义。 |
| `eth_main` | 标准矿工证绑定的 EVM 地址，用于 ETHW 侧挖矿身份和收益接收。 |
| `leader_pass_id` | 固定 Leader 矿工证的 BTC inscription id。 |
| `leader_btc_addr` | Leader BTC 地址；按历史高度解析为该地址当前 active standard pass。 |

# 规范关键词

本文中的“必须”、“禁止”、“应该”、“可以”遵循 UIP-0000 的规范关键词含义。

# 版本模型

UIP-0001 定义 USDB 矿工证铭文的第一个标准协议版本：v1。

当前代码和早期文档中的开发期载荷不定义为正式协议版本。开发期字段、临时兼容逻辑或本地测试数据属于实现迁移问题，不进入 UIP-0001 的规范版本序列。

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
- standard pass 必须包含 `eth_main`，且禁止包含 `leader_pass_id` 和 `leader_btc_addr`。
- collab pass 必须在 `leader_pass_id` 和 `leader_btc_addr` 中二选一，且禁止包含 `eth_main`。

# v1 字段定义

| 字段 | 类型 | 必填 | 适用类型 | 说明 |
| --- | --- | --- | --- | --- |
| `p` | string | 是 | all | 固定为 `"usdb"`。 |
| `op` | string | 是 | all | 固定为 `"mint"`。 |
| `v` | integer | 是 | all | 当前为 `1`。 |
| `eth_main` | string | 条件必填 | standard | 标准矿工证的 EVM 地址。 |
| `leader_pass_id` | string | 条件必填 | collab | 固定 Leader 矿工证 inscription id。 |
| `leader_btc_addr` | string | 条件必填 | collab | Leader BTC 地址，必须属于当前 BTC 网络。 |
| `prev` | string[] | 否 | all | 被继承矿工证 inscription id 列表；缺省等价于空数组。 |

## 字段互斥规则

### standard pass

当铭文包含 `eth_main` 且不包含任何 leader 绑定字段时，该铭文是 standard pass：

- `eth_main` 必须存在，且必须是合法 EVM 地址。
- `leader_pass_id` 禁止存在。
- `leader_btc_addr` 禁止存在。
- `eth_collab` 禁止存在。
- 该 pass 可以独立进入后续 validator candidate set。

### collab pass

当铭文包含 `leader_pass_id` 或 `leader_btc_addr`，且不包含 `eth_main` 时，该铭文是 collab pass：

- `leader_pass_id` 与 `leader_btc_addr` 必须二选一，禁止同时存在。
- `leader_pass_id` 存在时，必须是合法 inscription id。
- `leader_btc_addr` 存在时，必须是当前 BTC 网络上的合法地址。
- `eth_main` 禁止存在。
- `eth_collab` 禁止存在。
- 该 pass 禁止独立进入 validator candidate set。
- 该 pass 的有效能量只能归入其 Leader 的 `effective_energy` 计算。

协作矿工证仍然是 BTC owner 持有的 pass 资产，但在绑定有效期间，其挖矿身份与收益接收口径必须使用 Leader 的 `eth_main`。

# JSON 示例

## standard pass

```json
{
  "p": "usdb",
  "op": "mint",
  "v": 1,
  "eth_main": "0x1111111111111111111111111111111111111111",
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
| `leader_eth_main` | 与 ETHW 挖矿身份直接相关 | ETH 地址可被多个 pass 复用，Leader remint 后地址不一定唯一；历史高度上难以反查具体 pass | 不推荐 |
| `leader_pass_id` | inscription id 不可变，唯一、可索引、可历史重放 | Leader remint 后不会自动跟随新 pass | 支持，适合固定 pass 绑定 |
| `leader_btc_addr` | Leader remint 后可自动跟随该地址的新 active standard pass | 协作者会自动接受该地址后续 active pass 和 `eth_main` 变化 | 支持，适合地址身份绑定 |

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

该模式允许 Leader 地址重新铸造新 pass 后自动继承协作者绑定关系。相应地，协作者也显式接受该地址后续 active pass 的 `eth_main` 变化和其他 Leader 侧状态变化。

# `eth_collab` 处理

v1 新铭文禁止使用 `eth_collab`。

原因：

- `eth_collab` 只能表达一个 EVM 地址，不能表达协作矿工证的链上资产身份。
- `eth_collab` 由 Leader 主动填写，缺少协作者主动授权语义。
- `eth_collab` 与 leader 绑定字段并存会产生双重解释路径。

激活后，若 v1 铭文包含 `eth_collab`，索引器必须将该铭文判为 invalid mint。

# `prev` 默认值

v1 中 `prev` 是可选字段。

规则：

- 缺失 `prev` 等价于 `prev: []`。
- `prev` 存在时必须是数组。
- 数组元素必须是合法 inscription id 字符串。
- 同一个 `prev` 数组中禁止出现重复 inscription id。

`prev` 指向对象是否存在、是否可继承、是否已被消费，由 UIP-0002 和 UIP-0003 定义。

# unknown fields 与重复字段

v1 schema 应该采用严格解析。

规则：

- 未定义字段必须导致 invalid mint。
- 重复 JSON key 必须导致 invalid mint。
- 字段类型不匹配必须导致 invalid mint。

严格解析的目标是避免不同 JSON parser 对重复字段或未知字段产生不同解释。

# content-type

索引器必须至少接受 UTF-8 JSON 内容。

推荐 content-type：

```text
application/json;charset=utf-8
```

如果 inscription source 无法提供可靠 content-type，索引器可以基于内容做 JSON 解析，但不得绕过 schema 校验。

# 激活矩阵

UIP-0001 主要影响 BTC 侧铭文解析和由 BTC 派生的 pass 状态。ETHW 侧只消费索引结果，不直接解析 BTC inscription content。

| Chain | Network Type | Network ID | Activation Anchor | Activation Value | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| BTC | regtest | btc-regtest | btc_height | TBD | Planned | 本地回归测试可最先启用 v1 strict schema。 |
| BTC | testnet | btc-testnet4 | btc_height | TBD | Planned | 公开测试网验证 parser 和历史重放。 |
| BTC | mainnet | btc-mainnet | btc_height | TBD | Planned | 主网 v1 schema 激活高度。 |
| ETHW | devnet | ethw-devnet-<name> | governance | TBD | Planned | ETHW 侧切换到消费 v1 pass snapshot。 |
| ETHW | mainnet | 主网-mainnet | governance | TBD | Planned | 主网 mainnet 接受 v1 pass 语义的治理激活点。 |

未列出的网络不得默认激活 UIP-0001。

# 激活与开发期实现

UIP-0001 只定义标准 v1 schema。

在 UIP-0001 激活前，仓库内历史测试数据、开发期 inscription payload 或临时 parser 字段都只属于 pre-standard implementation draft，不构成需要长期兼容的协议版本。

实现可以为了本地开发、regtest 或一次性迁移保留兼容开关，但这些兼容开关必须满足：

- 不得写入 UIP-0001 的标准字段语义。
- 不得在已激活的 mainnet/testnet 上影响共识解析。
- 不得把 `eth_collab` 解释为 v1 collab 绑定。
- 不得把缺失 `v` 的新 mint 解释为 v1。

## 激活后

激活后，新 mint 必须满足 v1 schema。

激活后：

- 缺失 `v` 的新 mint 必须判为 invalid。
- `v != 1` 的新 mint 必须判为 invalid，除非后续 UIP 激活新版本。
- 包含 `eth_collab` 的新 mint 必须判为 invalid。
- 同时包含 `eth_main` 和任一 leader 绑定字段的新 mint 必须判为 invalid。
- 同时包含 `leader_pass_id` 和 `leader_btc_addr` 的新 mint 必须判为 invalid。
- 同时缺失 `eth_main`、`leader_pass_id` 和 `leader_btc_addr` 的新 mint 必须判为 invalid。

历史回放必须按该网络在对应高度的激活状态解释。开发期数据迁移不属于 UIP-0001 的共识规则。

# 协作矿工证的设计约束

协作矿工证的核心语义是：

```text
collab_pass -> leader_pass_id | leader_btc_addr -> leader.eth_main
```

因此：

- 协作者通过自己的 BTC mint 显式选择 Leader。
- Leader 不再通过 `eth_collab` 主动指定协作者。
- 协作关系的链上授权来自 collab pass owner。
- collab pass 不再携带自己的 `eth_main`。
- collab pass 不得作为独立挖矿身份参与 candidate set。
- collab pass 的 raw energy 可以被索引用于审计，但参与挖矿时必须只计入 Leader 的 `effective_energy`。
- `leader_pass_id` 模式绑定具体 pass，`leader_btc_addr` 模式绑定地址在历史高度上的 active standard pass。

这可以避免同一份能量同时作为 collab 加成和独立矿工能量被重复使用。

# 与后续 UIP 的边界

## UIP-0002

UIP-0002 必须定义：

- standard pass 和 collab pass 的状态机差异。
- collab pass 在 Leader 失效时的状态。
- collab pass 是否可以通过 remint 转为 standard pass。
- `leader_pass_id` 不存在或无效时，mint 是 invalid 还是进入 pending 状态。
- `leader_btc_addr` 在 mint 高度没有 active standard pass 时，mint 是 invalid 还是只在解析高度不贡献有效能量。

本文建议：`leader_pass_id` 引用不存在或无效 Leader 时，collab mint 应判为 invalid；`leader_btc_addr` 可以只校验地址格式，具体 Leader 在每个历史高度动态解析。

## UIP-0003

UIP-0003 必须定义：

- collab pass 的 raw energy 如何增长。
- collab pass remint 或退出时 energy 是否折损。
- collab pass 从协作关系退出后是否存在 cooldown。

## UIP-0004

UIP-0004 必须定义：

- Leader 有效性窗口。
- `effective_energy` 公式。
- collab energy 权重。
- leader remint、transfer、burn 后 collab 绑定如何失效或迁移。
- `leader_btc_addr` 自动跟随后，是否需要 cooldown 或延迟生效。
- collab energy 防双计数规则。

# 实现影响

预期需要修改：

- `src/btc/usdb-indexer/src/index/content.rs`
- `src/btc/usdb-indexer/src/inscription/source.rs`
- `src/btc/usdb-indexer/src/index/indexer.rs`
- `src/btc/usdb-indexer/src/index/pass.rs`
- `src/btc/usdb-indexer/src/storage/pass.rs`

建议实现时先只落 schema 解析与存储字段，不提前实现 effective energy。

# 测试要求

最小测试集合：

- v1 standard mint valid。
- v1 collab mint with `leader_pass_id` valid。
- v1 collab mint with `leader_btc_addr` valid。
- v1 missing `prev` 等价于空数组。
- v1 invalid `eth_main`。
- v1 invalid `leader_pass_id`。
- v1 invalid `leader_btc_addr` for active BTC network。
- v1 同时包含 `eth_main` 和任一 leader 绑定字段 invalid。
- v1 同时包含 `leader_pass_id` 和 `leader_btc_addr` invalid。
- v1 同时缺失 `eth_main`、`leader_pass_id` 和 `leader_btc_addr` invalid。
- v1 包含 `eth_collab` invalid。
- v1 unknown field invalid。
- v1 duplicate key invalid。
- pre-standard development payload 不作为正式协议版本参与标准解析。

# 安全考虑

## 协作者授权

协作关系必须由 collab pass owner 自己 mint 表达，避免 Leader 单方面指定他人作为协作者。

## 防双计数

collab pass 不能同时作为独立 candidate 和 Leader 加成来源。

## 引用模式

`leader_pass_id` 是稳定 inscription id，适合固定 pass 绑定。

`leader_btc_addr` 是地址身份绑定，适合 Leader 地址 remint 后自动跟随。该模式必须按历史高度解析，且协作者必须接受该地址后续 active standard pass 的 `eth_main` 变化。

## 历史回放

所有 parser 行为必须按 mint 高度和网络激活状态解释，不能用未来激活规则重算历史。开发期兼容逻辑不得改变已激活网络的标准解析结果。

# 未决问题

- `leader_pass_id` 引用的 Leader 是否必须在同一 BTC 高度之前已经存在，还是允许同一 block 内按事件顺序解析。
- `leader_btc_addr` 在 mint 高度没有 active standard pass 时，是 invalid 还是允许后续高度动态生效。
- `leader_btc_addr` 自动跟随新 active pass 时，是否需要延迟一个 BTC block 或 ETHW epoch 生效。
- collab mint 引用 Dormant Leader 时应 invalid，还是允许暂时无效但保留绑定。
- collab pass 转 standard pass 的退出折损率和 cooldown。
- v1 strict parser 是否需要在 Rust 层实现 duplicate key 检测，而不是只依赖 `serde_json::Value`。
- 主网的稳定 `network_id` 是否最终采用 `主网-mainnet`。

# 下一步

1. Review 本草案中的 v1 字段互斥规则。
2. 在实现中落地 `leader_pass_id` / `leader_btc_addr` 二选一解析与校验。
3. 确认正式 UIP 不为开发期格式分配协议版本，并在实现中移除或隔离 `eth_collab`。
4. 在 UIP-0002 定义 standard/collab 状态机。
5. 在 UIP-0004 定义 collab effective energy 与防双计数规则。
