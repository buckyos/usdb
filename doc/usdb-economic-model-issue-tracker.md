# USDB 经济模型问题与修复跟踪

## 1. 目的

本文档用于持续跟踪 USDB 经济模型从当前实现收敛到正式协议规范的过程。

它不是最终协议正文，而是工作看板：

- 记录当前 review 发现的问题。
- 新发现的问题先进入 `Todo` 状态。
- 开始修复时切换为 `Doing`。
- 代码与文档完成后切换为 `Done`。
- 验证完成后切换为 `Verified`。
- 因协议决议或外部依赖暂时无法推进时切换为 `Blocked`。

目标是让后续每一轮工作都能从本文档判断：

1. 当前经济模型还剩哪些问题。
2. 下一个应优先处理的协议或实现任务是什么。
3. 每个任务的验收标准和关联文档/代码位置是什么。

## 2. 状态字段

| 状态 | 含义 |
| --- | --- |
| `Todo` | 已确认需要处理，但尚未开始。 |
| `Doing` | 正在拆分协议、实现或补测试。 |
| `Blocked` | 需要先完成协议决议、依赖实现或参数选择。 |
| `Done` | 已完成文档/实现更新，但尚未完成最终验证。 |
| `Verified` | 已通过对应测试、review 或协议验收。 |

## 3. 优先级字段

| 优先级 | 含义 |
| --- | --- |
| `P0` | 共识安全、价值继承、状态机确定性相关，优先处理。 |
| `P1` | 影响经济参数、验证路径或后续实现分层，应尽快处理。 |
| `P2` | 目标模型增强项，可在核心规则稳定后推进。 |

## 4. 总体结论

当前实现已经具备可运行的矿工证索引、pass 状态机、raw energy、历史 RPC 和 validator payload 基础。

但 `doc/usdb-economic-model-design.md` 描述的是目标经济模型，已经明显超出当前实现范围。后续不能直接把整份经济模型一次性落到代码里，应先拆分正式 UIP 协议，再按协议版本和激活高度逐步实现。

当前最重要的收敛方向：

1. 先建立 UIP 目录、编号、状态和拆分边界。
2. 先协议化矿工证铭文 schema、pass 状态机、`prev` 继承和 energy formula。
3. 再实现 collab / leader / effective energy / level / difficulty。
4. 最后处理 CoinBase、price / real_price、辅助算力池、收入分配等更大范围经济机制。

## 5. 问题与任务清单

| ID | 优先级 | 状态 | 标题 | 关联范围 |
| --- | --- | --- | --- | --- |
| ECO-001 | P0 | Done | 统一 UIP 命名、目录、编号和流程 | `doc/UIP/` |
| ECO-002 | P0 | Todo | 明确矿工证铭文 schema 与兼容策略 | `doc/矿工证铭文协议.md`, `content.rs` |
| ECO-003 | P0 | Todo | 将 `prev` 继承从 warn/skip 收敛为严格失败 | `pass.rs`, pass tests |
| ECO-004 | P0 | Todo | Burned 状态必须同步写入 energy 终态 | `pass.rs`, `energy.rs`, tests |
| ECO-005 | P0 | Todo | 明确并实现 energy penalty v2 公式 | `energy_formula.rs`, `energy.rs` |
| ECO-006 | P1 | Todo | 明确并实现继承折损规则 | `pass.rs`, `energy.rs` |
| ECO-007 | P1 | Todo | 定义 collab pass 与 leader 绑定协议 | UIP collab/leader |
| ECO-008 | P1 | Todo | 定义并实现 effective_energy / level / real_difficulty | RPC, validator payload |
| ECO-009 | P1 | Todo | 建立经济公式版本与激活高度治理 | `usdb-util`, state ref |
| ECO-010 | P2 | Todo | CoinBase、分账、price / real_price、辅助算力池拆分 | economic UIP 后续 |
| ECO-011 | P1 | Todo | validator payload 补齐经济字段边界 | validator block-body docs/tests |
| ECO-012 | P1 | Todo | 明确 canonical JSON、content-type 和未知字段策略 | inscription source/content parser |

## 6. 详细条目

### ECO-001. 统一 UIP 命名、目录、编号和流程

- 优先级：`P0`
- 状态：`Done`
- 当前现状：
  - `doc/usdb-economic-model-design.md` 头部使用 `UBIP: UBIP-01`。
  - 当前需求中希望采用 `UIP`，并参考 BTC BIP / ETH EIP。
- 目标：
  - 使用统一的 `UIP-NNN` 编号。
  - 明确 `Draft / Review / Last Call / Final / Superseded / Withdrawn` 等状态。
  - 明确标准类、信息类、流程类 UIP 的边界。
- 下一步：
  - review `doc/UIP/UIP-0000-uip-process.md` 中的状态流、激活矩阵和网络标识。
  - 确认诸王 mainnet、ETHW testnet/devnet 等正式 `network_id`。
- 验收：
  - `doc/UIP/` 下有统一目录说明。
  - 后续所有正式协议文档都使用同一头部模板。
  - 已起草 `UIP-0000`，待 review 后可切换为 `Verified`。

### ECO-002. 明确矿工证铭文 schema 与兼容策略

- 优先级：`P0`
- 状态：`Todo`
- 当前现状：
  - `doc/矿工证铭文协议.md` 说明 `prev` 是可选字段。
  - 当前 `USDBMint` 中 `prev` 是必填 `Vec<String>`，缺失会被 serde 判为 schema invalid。
  - 当前协议没有明确 `v` / `protocol_version` 字段，也没有 `leader_ref`。
  - `eth_collab` 目前只进行 EVM 地址格式校验，尚不能表达协作矿工证绑定哪个 leader。
- 目标：
  - 明确必填字段、可选字段、默认值和兼容策略。
  - 明确是否引入 `v`、`leader_pass_id`、`leader_btc_addr` 或其他 `leader_ref` 表达。
  - 明确旧格式在激活高度前后的处理。
- 下一步：
  - 在 UIP inscription schema 文档中定义规范 JSON schema。
  - 再决定实现层是否对 `prev` 增加 `serde(default)` 或按协议版本处理。
- 验收：
  - 有覆盖缺失 `prev`、未知字段、版本字段、collab 字段的单测。
  - 文档和 parser 行为一致。

### ECO-003. 将 `prev` 继承从 warn/skip 收敛为严格失败

- 优先级：`P0`
- 状态：`Todo`
- 当前现状：
  - 当前实现对 `prev` 中 owner 不一致、状态非 `Dormant`、引用不存在等情况采用 warn/skip，并继续 mint。
  - 这适合早期容错，但不适合共识价值继承。
- 目标：
  - 在新协议版本下，任意 `prev` 无效都必须让本次 mint 进入 `Invalid`。
  - 明确所有权一致性是 owner 相同、控制权相同还是 lineage 相同。
  - 同一个 `prev` 在同一列表中重复出现必须 invalid。
- 下一步：
  - 先在 UIP pass state / remint 文档中定义严格规则。
  - 再修改 `MinerPassManager::on_mint_pass` 的处理路径。
- 验收：
  - 增加 owner mismatch、missing prev、already consumed、burned prev、duplicate prev 的严格 invalid 测试。
  - 旧行为如需保留，必须受协议版本或激活高度控制。

### ECO-004. Burned 状态必须同步写入 energy 终态

- 优先级：`P0`
- 状态：`Todo`
- 当前现状：
  - `on_pass_burned` 只更新 pass state。
  - energy 查询命中 burn 后高度时，仍可能从 burn 前的 `Dormant` 或 `Active` checkpoint 投影或返回旧值。
  - 当前测试中已有 burn 后 energy 仍为 `Dormant` 的断言。
- 目标：
  - burn 发生时，energy 状态机必须写入 `Burned` record。
  - `Burned` energy 必须为 `0`。
  - 任意历史查询命中 burn 后高度，不得继续返回 burn 前可用能量。
- 下一步：
  - 在 UIP pass state / energy 文档中定义 burn 终态。
  - 增加 `PassEnergyManager::on_pass_burned` 或等价接口。
  - 更新 burn 相关测试断言。
- 验收：
  - burn 后 pass snapshot 和 energy snapshot 状态一致。
  - validator payload 不会使用 burned pass 的旧能量。

### ECO-005. 明确并实现 energy penalty v2 公式

- 优先级：`P0`
- 状态：`Todo`
- 当前现状：
  - 当前实现增长公式可与抽象单位模型对齐。
  - 当前惩罚是固定窗口近似：`abs(delta_sats) * 43_200_000`。
  - 目标文档提出 `penalty = lost_units * H_now * lambda`，并按损失比例调整 `active_block_height`。
- 目标：
  - 明确定点精度、rounding、saturation、分母为 0 的处理。
  - 明确 `active_block_height'` 的更新公式。
  - 明确公式版本升级和激活高度。
- 下一步：
  - 在 UIP energy formula 文档中把公式写成整数/定点实现规则。
  - 再实现 `calc_penalty_v2` 和对应迁移路径。
- 验收：
  - 有参数化公式单测和 timeline 测试。
  - validator state ref 中公式版本能区分 v1/v2。

### ECO-006. 明确并实现继承折损规则

- 优先级：`P1`
- 状态：`Todo`
- 当前现状：
  - 目标文档建议 `INHERIT_DISCOUNT = 0.05`。
  - 当前实现是全额继承 dormant energy。
- 目标：
  - 明确折损率、rounding 和多 `prev` 累加顺序。
  - 明确旧版本和新版本的差异。
- 下一步：
  - 先在 UIP energy formula 中定义继承折损。
  - 再修改 consume/inherit 路径。
- 验收：
  - 多 prev 继承、单 prev 继承、边界 rounding 都有测试。

### ECO-007. 定义 collab pass 与 leader 绑定协议

- 优先级：`P1`
- 状态：`Todo`
- 当前现状：
  - `eth_collab` 目前只是可选 EVM 地址字段。
  - 目标模型要求 collab pass 创建时绑定有效 leader。
- 目标：
  - 明确 collab pass 如何表达 leader 引用。
  - 明确 leader 失效、collab 退出、collab 转普通 pass 的状态转换。
  - 明确 collab pass 自身是否可独立参与 candidate set。
- 下一步：
  - 在单独 UIP 中定义 `leader_ref` 字段和状态机。
- 验收：
  - collab pass 的基础 energy 与 effective energy 不会双重计数。

### ECO-008. 定义并实现 effective_energy / level / real_difficulty

- 优先级：`P1`
- 状态：`Todo`
- 当前现状：
  - 当前 leaderboard 和 validator 样例主要使用 raw `energy`。
  - 目标模型需要 `effective_energy`、`level`、`real_difficulty`。
- 目标：
  - 定义 `level(effective_energy)` 的整数或定点计算方式。
  - 定义 `real_difficulty` 下界，如 `MAX_LEVEL` 或 `MIN_DIFFICULTY_FACTOR`。
  - 明确 RPC 与 validator payload 返回 raw energy 还是 effective energy。
- 下一步：
  - 先完成 UIP effective energy / difficulty。
  - 再加 RPC 字段和 validator payload 字段。
- 验收：
  - 单元测试覆盖 level 边界、rounding、最大折扣。
  - candidate set 选择规则使用协议指定字段。

### ECO-009. 建立经济公式版本与激活高度治理

- 优先级：`P1`
- 状态：`Todo`
- 当前现状：
  - `USDB_INDEX_FORMULA_VERSION` 当前是全局常量 `pass-energy-formula:v1`。
  - 文档要求公式变更必须通过显式激活高度或治理决议，不应代码发布即生效。
- 目标：
  - 建立公式版本、协议版本、查询语义版本之间的关系。
  - 明确历史高度按当时激活版本重放。
  - 明确 state ref / snapshot id 是否包含激活版本表。
- 下一步：
  - 在 UIP process / versioning 文档中定义激活机制。
  - 再更新共识 identity 和历史查询路径。
- 验收：
  - 同一节点能对不同历史高度按对应公式版本查询。
  - validator payload version mismatch 路径可覆盖经济公式版本。

### ECO-010. CoinBase、分账、price / real_price、辅助算力池拆分

- 优先级：`P2`
- 状态：`Todo`
- 当前现状：
  - 目标经济模型已经写出方向，但大量参数和证明格式仍是 `<TODO>`。
  - 当前代码侧尚未完整实现这些机制。
- 目标：
  - 将发行、分账、价格、辅助算力池拆成独立 UIP。
  - 每个 UIP 必须有确定性输入、整数公式、验证路径和 reorg 语义。
- 下一步：
  - 等 pass / energy / validator 基础协议稳定后推进。
- 验收：
  - 每个机制都有独立协议文档、实现计划和测试计划。

### ECO-011. validator payload 补齐经济字段边界

- 优先级：`P1`
- 状态：`Todo`
- 当前现状：
  - 当前 validator block-body 文档已经覆盖 `external_state`、`miner_selection.energy` 和 candidate set。
  - 经济模型后续会引入 effective energy、level、difficulty、reward、price 等字段。
- 目标：
  - 明确哪些字段必须进入 payload。
  - 明确哪些字段可由 validator 本地重算，不需要 payload 直接携带。
  - 明确 tamper 测试和 mismatch 错误。
- 下一步：
  - 在 UIP validator payload 中补齐经济字段边界。
- 验收：
  - candidate set / reward / difficulty 相关 payload 可在历史 context 下重放。

### ECO-012. 明确 canonical JSON、content-type 和未知字段策略

- 优先级：`P1`
- 状态：`Todo`
- 当前现状：
  - 旧 loader 中存在 content-type 支持判断，但新的 inscription source 路径主要从 text body 直接 classify。
  - 当前 parser 对未知字段依赖 serde 默认忽略行为。
  - 目标协议尚未说明 canonical JSON、字段顺序、大小写、重复字段和未知字段策略。
- 目标：
  - 明确 inscription 内容的 JSON canonical 规则。
  - 明确支持的 content-type。
  - 明确未知字段在不同协议版本下是允许、忽略还是 invalid。
- 下一步：
  - 在 UIP inscription schema 中补齐解析与 canonicalization 章节。
- 验收：
  - ord source、bitcoind source、fixture source 对同一铭文给出一致分类。

## 7. 新问题登记模板

新增问题时复制以下模板，并在 `## 5. 问题与任务清单` 表格中增加一行：

```md
### ECO-XXX. <标题>

- 优先级：`P0|P1|P2`
- 状态：`Todo`
- 当前现状：
  - <现状>
- 目标：
  - <目标行为>
- 下一步：
  - <下一步动作>
- 验收：
  - <验证方式>
```

## 8. 推荐下一步

建议下一轮从 `ECO-001` 和 `ECO-002` 开始：

1. 固化 UIP 目录、模板、编号和拆分边界。
2. 起草正式 `UIP-0001`：Miner Pass inscription schema。
3. 明确旧格式兼容和激活高度。
4. 再进入 `prev` 严格继承与 energy v2 的实现工作。
