UIP: UIP-0000
Title: UIP Process and Governance
Status: Draft
Type: Process
Layer: Process
Created: 2026-04-25
Requires: None
Supersedes: UBIP naming draft usage
Activation: None

# UIP-0000: UIP 流程与治理

## 1. 摘要

本文定义 USDB Improvement Proposal（UIP）的编号、类型、状态流、文档模板、协议优先级和激活规则。

UIP 是 USDB 正式协议规范的承载格式。后续会用 UIP 拆分并标准化：

- BTC 侧矿工证铭文协议。
- pass 状态机与 energy 公式。
- validator payload 与历史状态验证。
- collab / leader / effective energy / difficulty。
- CoinBase、price / real_price、辅助算力池等经济模型组件。

本文是流程类 UIP，不定义具体经济公式，也不改变当前运行时行为。

## 2. 动机

当前 USDB 经济模型相关规则分散在设计文档、实现代码、测试脚本和讨论稿中。

其中 `doc/usdb-economic-model-design.md` 已经形成目标经济模型草案，但其头部仍使用 `UBIP`，且尚未明确：

- 哪些文档属于正式协议。
- 草案、review 和 final 的状态流。
- 已激活协议与实现代码之间的优先级。
- 协议激活高度应锚定 BTC 还是 ETHW。
- 同一协议在 mainnet、testnet、regtest 等不同网络上的激活是否可以不同。

如果不先固定 UIP 流程，后续 `UIP-0001` 铭文 schema、`UIP-0003` energy formula、`UIP-0007` 版本激活等文档会缺少共同解释框架。

## 3. 术语

- **UIP**：USDB Improvement Proposal，用于定义 USDB 协议、流程或说明性设计。
- **正式 UIP**：位于 `doc/UIP/`，并使用本文定义头部字段和状态流的文档。
- **讨论稿**：尚未进入正式 UIP 状态流的设计文档，例如旧经济模型草案、review 记录和规划文档。
- **激活**：某个 UIP 的规范开始约束指定链、指定网络和指定高度之后的协议行为。
- **激活锚点**：用于判断 UIP 是否生效的链上高度或治理决议，例如 BTC height 或 ETHW block number。
- **网络类型**：mainnet、testnet、signet、regtest、devnet、local 等网络类别。
- **网络标识**：具体网络名称或链 ID，例如 `btc-mainnet`、`btc-regtest`、`ethw-mainnet`、`ethw-testnet`、`主网-mainnet`。

## 4. 规范性关键词

本文中的“必须”、“禁止”、“应该”、“可以”分别表示强约束、强禁止、建议约束与可选实现。

正式 UIP 中如果使用这些词，应按本文含义解释。

## 5. UIP 编号

正式 UIP 必须使用如下编号格式：

```text
UIP-0000
UIP-0001
UIP-0002
...
```

规则：

- 编号必须为四位十进制数字。
- 编号一旦分配，禁止复用。
- 被撤回或废弃的 UIP 编号也禁止复用。
- `UIP-0000` 保留给 UIP 流程与治理。
- 后续正式协议从 `UIP-0001` 开始。

旧文档中的 `UBIP` 命名只作为历史草案标记。进入正式流程时，必须迁移到 `UIP` 编号。

## 6. UIP 类型

| 类型 | 用途 |
| --- | --- |
| `Standards Track` | 影响协议、共识、索引、验证、经济结算、RPC 语义或状态承诺的规范。 |
| `Informational` | 背景说明、设计分析、运营说明或非强制建议。 |
| `Process` | UIP 流程、治理、模板、版本和激活规则。 |

影响共识结果的文档必须使用 `Standards Track`，除非它只定义流程本身。

## 7. UIP 状态

| 状态 | 含义 |
| --- | --- |
| `Draft` | 初稿阶段，允许大幅调整，不得作为已激活协议对外宣称。 |
| `Review` | 已进入协议 review，字段、公式和兼容策略应趋于稳定。 |
| `Last Call` | 准备冻结，只接受关键问题修正。 |
| `Final` | 已成为正式协议，但仍需查看激活表判断是否对某个网络生效。 |
| `Living` | 长期维护的流程类文档，例如本文。 |
| `Deferred` | 暂缓推进。 |
| `Withdrawn` | 主动撤回。 |
| `Superseded` | 被后续 UIP 替代。 |

状态流建议：

```text
Draft -> Review -> Last Call -> Final
```

特殊状态可从任意阶段进入：

```text
Draft/Review/Last Call -> Deferred
Draft/Review/Last Call -> Withdrawn
Final -> Superseded
```

## 8. UIP 头部字段

正式 UIP 必须包含以下头部字段：

```text
UIP: UIP-0001
Title: Miner Pass Inscription Schema
Status: Draft
Type: Standards Track
Layer: Application / Consensus
Created: YYYY-MM-DD
Requires: <optional>
Supersedes: <optional>
Activation: <None | See Activation Matrix | TODO>
```

字段说明：

| 字段 | 必填 | 说明 |
| --- | --- | --- |
| `UIP` | 是 | 稳定编号。 |
| `Title` | 是 | 简短标题。 |
| `Status` | 是 | 本文定义的状态之一。 |
| `Type` | 是 | `Standards Track`、`Informational` 或 `Process`。 |
| `Layer` | 是 | 影响层，例如 Process、Application、Consensus、RPC、Validator、Economics。 |
| `Created` | 是 | 初次创建日期。 |
| `Requires` | 否 | 依赖的其他 UIP 或文档。 |
| `Supersedes` | 否 | 被本 UIP 替代的旧 UIP 或旧文档。 |
| `Activation` | 是 | 无激活、待定或引用激活表。 |

影响共识的 Standards Track UIP 应额外提供：

- `Affected-Version-Fields`
- `Activation-Matrix`
- `Backwards-Compatibility`
- `Test-Cases`

## 9. 正文模板

正式 Standards Track UIP 应包含：

1. `摘要`
2. `动机`
3. `术语`
4. `规范`
5. `激活矩阵`
6. `版本影响`
7. `向后兼容`
8. `安全性考虑`
9. `测试要求`
10. `参考实现`
11. `待定问题`

Process 和 Informational UIP 可以裁剪模板，但必须保留头部字段和状态。

## 10. 协议优先级

当文档、实现和讨论稿不一致时，解释优先级如下：

1. 已在对应链、对应网络、对应高度激活的 UIP。
2. `Final` 状态但尚未在目标网络激活的 UIP。
3. `Last Call` / `Review` / `Draft` 状态的 UIP。
4. `doc/usdb-economic-model-design.md` 等讨论稿。
5. 参考实现和测试。
6. issue、聊天记录、临时 review 记录。

注意：

- 当前代码行为可以作为兼容基线，但不得自动等同于正式协议。
- 新代码发布不得隐式激活新的共识规则。
- 已激活 UIP 的历史高度语义必须可重放。

## 11. 激活范围模型

影响共识或经济结果的 UIP 必须定义激活范围。

激活范围必须同时说明：

- 对应链：BTC、ETHW 或跨链组合。
- 网络类型：mainnet、testnet、signet、regtest、devnet、local。
- 网络标识：具体网络名称或链 ID。
- 激活锚点：BTC height、ETHW block number、治理决议或显式 none。
- 激活状态：Planned、Active、Deferred、Superseded。

### 11.1 链字段

| 链字段 | 含义 |
| --- | --- |
| `BTC` | BTC 主链或 BTC 兼容网络上的铭文、余额、UTXO、reorg 和 indexer 派生规则。 |
| `ETHW` | ETHW 链上的 validator、执行、收益合约、治理或价格更新规则。 |
| `CrossChain` | 同时依赖 BTC 与 ETHW 状态的规则。 |

### 11.2 网络类型

| 网络类型 | 含义 |
| --- | --- |
| `mainnet` | 正式生产网络，例如 BTC mainnet、ETHW mainnet、主网。 |
| `testnet` | 公开测试网络。 |
| `signet` | BTC signet 或同类受控测试网络。 |
| `regtest` | 本地可控 BTC regtest。 |
| `devnet` | 项目自建开发网络。 |
| `local` | 单机或临时集成环境。 |

### 11.3 网络标识

`network_id` 必须是具体名称，禁止只写 `mainnet` 或 `testnet`。

示例：

| 链 | 网络类型 | 网络标识 |
| --- | --- | --- |
| `BTC` | `mainnet` | `btc-mainnet` |
| `BTC` | `testnet` | `btc-testnet4` |
| `BTC` | `signet` | `btc-signet` |
| `BTC` | `regtest` | `btc-regtest` |
| `ETHW` | `mainnet` | `ethw-mainnet` |
| `ETHW` | `testnet` | `ethw-testnet` |
| `ETHW` | `mainnet` | `主网-mainnet` |
| `ETHW` | `devnet` | `ethw-devnet-<name>` |

如果某个网络还没有稳定 ID，必须写 `TODO`，不能省略。

## 12. 激活矩阵

每个影响共识的 UIP 必须提供激活矩阵。

推荐格式：

| Chain | Network Type | Network ID | Activation Anchor | Activation Value | Status | Notes |
| --- | --- | --- | --- | --- | --- | --- |
| BTC | regtest | btc-regtest | btc_height | TBD | Planned | 本地测试可先启用。 |
| BTC | mainnet | btc-mainnet | btc_height | TBD | Planned | 正式 BTC 铭文规则激活高度。 |
| ETHW | testnet | ethw-testnet | ethw_block | TBD | Planned | validator 侧测试激活。 |
| ETHW | mainnet | 主网-mainnet | ethw_block | TBD | Planned | 主网执行侧激活。 |
| CrossChain | mainnet | btc-mainnet + 主网-mainnet | governance | TBD | Planned | 同时依赖 BTC 与 ETHW 的规则。 |

规则：

- BTC 侧铭文、余额、pass 状态、energy 派生规则，默认使用 `btc_height` 作为激活锚点。
- ETHW 侧 validator、执行、收益合约、治理和价格更新规则，默认使用 `ethw_block` 或治理决议作为激活锚点。
- 跨链规则必须明确主锚点和辅助锚点。
- 不同网络可以有不同激活高度，但必须在同一 UIP 的激活矩阵中显式列出。
- 未列出的网络不得假设自动激活。

## 13. 激活锚点

| Anchor | 含义 |
| --- | --- |
| `none` | 流程类或说明类文档，不需要链上激活。 |
| `btc_height` | 以 BTC 区块高度作为激活点。 |
| `ethw_block` | 以 ETHW 区块号作为激活点。 |
| `governance` | 以治理决议或链上配置作为激活点。 |
| `manual` | 仅用于 devnet/local，不得用于 mainnet。 |
| `hybrid` | 同时依赖多个锚点，必须在 Notes 中说明逻辑。 |

主网规则不得使用 `manual` 激活。

## 14. 版本字段

UIP 必须声明自己影响哪些版本字段。

当前建议版本字段：

| 字段 | 用途 |
| --- | --- |
| `protocol_version` | 矿工证协议、状态机、validator payload 等外部协议版本。 |
| `formula_version` | energy、effective energy、level、difficulty、CoinBase、price 等公式版本。 |
| `query_semantics_version` | RPC 历史查询、projection、exact / at_or_before 语义版本。 |
| `payload_version` | validator payload 结构版本。 |
| `commit_protocol_version` | local commit / block commit 序列化与哈希规则。 |

版本规则：

- 影响共识结果的公式变更必须升级 `formula_version`。
- 影响铭文 schema 或状态机解释的变更必须升级 `protocol_version`。
- 影响 RPC 查询结果解释但不改变底层状态的变更必须升级 `query_semantics_version`。
- 影响 validator payload 字段或重放规则的变更必须升级 `payload_version`。
- 影响 commit 哈希输入或序列化规则的变更必须升级 `commit_protocol_version`。

## 15. 历史重放规则

已激活 UIP 必须支持历史重放。

规则：

- 查询高度低于某 UIP 激活高度时，必须按旧版本解释。
- 查询高度大于等于激活高度时，必须按新版本解释。
- reorg 后，必须按新 canonical 分支上的高度重新判断激活版本。
- 不得用当前 head 的最新公式覆盖旧高度结果。

如果某个 UIP 无法支持历史双版本重放，必须在 `Backwards Compatibility` 中明确说明，并解释迁移策略。

## 16. 与现有文档的关系

当前关系如下：

- `doc/UIP/`：正式 UIP 存放目录。
- `doc/UIP/uip-split-design.md`：UIP 拆分路线图，不是最终协议。
- `doc/usdb-economic-model-design.md`：目标经济模型讨论稿。
- `doc/usdb-economic-model-issue-tracker.md`：问题与修复跟踪文档。
- `doc/矿工证铭文协议.md`：现有矿工证铭文草案，后续应迁移或被 `UIP-0001` supersede。

正式 UIP 进入 `Final` 并激活后，其规范性优先级高于上述讨论稿和旧草案。

## 17. 参考实现要求

Standards Track UIP 如果包含实现要求，必须列出：

- 影响模块。
- 必须新增或更新的测试。
- 是否需要迁移已有数据。
- 是否需要更新 RPC 文档。
- 是否需要更新 validator payload 或 regtest/world-sim 脚本。

参考实现合并后，不代表 UIP 自动进入 `Final`，也不代表 UIP 自动在任何网络激活。

## 18. 测试要求

Standards Track UIP 必须定义测试要求。

建议分层：

- 公式单元测试。
- 状态机单元测试。
- storage / RPC 历史查询测试。
- regtest 场景测试。
- validator payload tamper / mismatch 测试。
- reorg / rollback 测试。
- network-specific activation 测试。

对有激活矩阵的 UIP，至少应覆盖：

- 激活前行为。
- 激活高度行为。
- 激活后行为。
- 未列出网络不激活行为。
- reorg 跨激活高度行为。

## 19. 安全性考虑

UIP 流程必须避免以下风险：

- 代码发布隐式改变已激活共识规则。
- BTC 网络和 ETHW 网络激活高度混淆。
- mainnet、testnet、regtest 共享未声明的激活状态。
- 查询层字段反向影响 validator 或奖励结算。
- 历史高度被当前 head 的最新协议解释污染。
- 旧草案与正式 UIP 发生冲突时缺少优先级。

## 20. 待定问题

1. 主网的稳定 `network_id` 是否就使用 `主网-mainnet`，还是需要英文/链 ID 格式。
2. ETHW 测试网和 devnet 的正式网络标识列表。
3. 治理决议的编号格式和链上引用方式。
4. 激活矩阵是否需要单独维护成机器可读文件。
5. `protocol_version` 与 `formula_version` 是否继续保持全局常量，还是按高度查询。

## 21. 下一步

建议后续按以下顺序推进：

1. review 本文的状态流、激活矩阵和网络标识。
2. 明确 `主网-mainnet` 等网络 ID。
3. 起草 `UIP-0001-miner-pass-inscription.md`。
4. 将旧 `doc/矿工证铭文协议.md` 标记为被 `UIP-0001` supersede 或迁移到参考资料。
