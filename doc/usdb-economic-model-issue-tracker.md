# USDB 经济模型问题与修复跟踪

## 1. 目的

本文档用于持续跟踪 USDB 经济模型从当前实现收敛到正式协议规范的过程。

它不是最终协议正文，而是工作看板：

- 记录当前 review 发现的问题。
- 新发现的问题先进入 `Todo` 状态。
- 开始协议拆分、实现或补测试时切换为 `In Progress`。
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
| `In Progress` | 正在拆分协议、实现或补测试。 |
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
| ECO-002 | P0 | In Progress | 明确矿工证铭文 schema 与兼容策略 | `doc/UIP/UIP-0001-miner-pass-inscription.md`, `content.rs` |
| ECO-003 | P0 | In Progress | 将 `prev` 继承从 warn/skip 收敛为严格失败 | `doc/UIP/UIP-0002-pass-state-machine.md`, `pass.rs` |
| ECO-004 | P0 | In Progress | Burned 状态必须同步写入 energy 终态 | `doc/UIP/UIP-0002-pass-state-machine.md`, `energy.rs` |
| ECO-005 | P0 | In Progress | 明确并实现 energy penalty v2 公式 | `doc/UIP/UIP-0003-pass-energy-formula.md`, `energy_formula.rs`, `energy.rs` |
| ECO-006 | P1 | In Progress | 明确并实现继承折损规则 | `doc/UIP/UIP-0003-pass-energy-formula.md`, `pass.rs`, `energy.rs` |
| ECO-007 | P1 | In Progress | 定义 collab pass 与 leader 绑定协议 | `doc/UIP/UIP-0001-miner-pass-inscription.md`, `doc/UIP/UIP-0004-collab-leader-effective-energy.md` |
| ECO-008 | P1 | In Progress | 定义并实现 effective_energy / level / real_difficulty | `doc/UIP/UIP-0004-collab-leader-effective-energy.md`, `doc/UIP/UIP-0005-level-and-real-difficulty.md`, RPC, state view, ETHW payload |
| ECO-009 | P1 | In Progress | 建立经济公式版本与激活高度治理 | `doc/UIP/UIP-0008-protocol-versioning-and-activation-matrix.md`, `usdb-util`, state ref |
| ECO-010 | P2 | In Progress | CoinBase、K、分账、price / real_price、辅助算力池拆分 | `doc/UIP/UIP-0011-*` 及后续 economic UIP |
| ECO-011 | P1 | In Progress | 拆分 USDB 经济状态视图与 ETHW 链上 payload | `doc/UIP/UIP-0006-usdb-economic-state-view.md`, `doc/UIP/UIP-0007-ethw-consensus-profile-selector.md`, validator block-body docs/tests |
| ECO-012 | P1 | Todo | 明确 canonical JSON、content-type 和未知字段策略 | inscription source/content parser |
| ECO-013 | P1 | In Progress | 标准化 SourceDAO / Dividend / fee split 冷启动流程 | `doc/UIP/UIP-0010-source-dao-dividend-bootstrap.md`, `doc/UIP/UIP-0009-ethw-chain-config-and-usdb-bootstrap.md` |

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
  - 确认主网、ETHW testnet/devnet 等正式 `network_id`。
- 验收：
  - `doc/UIP/` 下有统一目录说明。
  - 后续所有正式协议文档都使用同一头部模板。
  - 已起草 `UIP-0000`，待 review 后可切换为 `Verified`。

### ECO-002. 明确矿工证铭文 schema 与兼容策略

- 优先级：`P0`
- 状态：`In Progress`
- 当前现状：
  - `doc/矿工证铭文协议.md` 说明 `prev` 是可选字段。
  - 当前 `USDBMint` 中 `prev` 是必填 `Vec<String>`，缺失会被 serde 判为 schema invalid。
  - 当前协议没有明确 `v` / `protocol_version` 字段，也没有 `leader_pass_id` / `leader_btc_addr`。
  - `eth_collab` 目前只进行 EVM 地址格式校验，尚不能表达协作矿工证绑定哪个 leader。
- 目标：
  - 明确必填字段、可选字段、默认值和兼容策略。
  - 明确以 `leader_pass_id` / `leader_btc_addr` 二选一作为 leader 引用，并移除新协议中的 `eth_collab`。
  - 明确开发期旧格式不作为正式协议版本进入 UIP 版本序列。
- 下一步：
  - Review `doc/UIP/UIP-0001-miner-pass-inscription.md` 中的 v1 schema。
  - 再决定实现层是否对 `prev` 增加 `serde(default)` 或按协议版本处理。
- 验收：
  - 有覆盖缺失 `prev`、未知字段、版本字段、collab 字段的单测。
  - 文档和 parser 行为一致。

### ECO-003. 将 `prev` 继承从 warn/skip 收敛为严格失败

- 优先级：`P0`
- 状态：`In Progress`
- 当前现状：
  - 当前实现对 `prev` 中 owner 不一致、状态非 `Dormant`、引用不存在等情况采用 warn/skip，并继续 mint。
  - 这适合早期容错，但不适合共识价值继承。
- 目标：
  - 在新协议版本下，任意 `prev` 无效都必须让本次 mint 进入 `Invalid`。
  - 明确所有权一致性是 owner 相同、控制权相同还是 lineage 相同。
  - 同一个 `prev` 在同一列表中重复出现必须 invalid。
- 下一步：
  - Review `doc/UIP/UIP-0002-pass-state-machine.md` 中的 `prev` strict invalid 规则。
  - 再修改 `MinerPassManager::on_mint_pass` 的处理路径。
- 验收：
  - 增加 owner mismatch、missing prev、already consumed、burned prev、duplicate prev 的严格 invalid 测试。
  - 旧行为如需保留，必须受协议版本或激活高度控制。

### ECO-004. Burned 状态必须同步写入 energy 终态

- 优先级：`P0`
- 状态：`In Progress`
- 当前现状：
  - `on_pass_burned` 只更新 pass state。
  - energy 查询命中 burn 后高度时，仍可能从 burn 前的 `Dormant` 或 `Active` checkpoint 投影或返回旧值。
  - 当前测试中已有 burn 后 energy 仍为 `Dormant` 的断言。
- 目标：
  - burn 发生时，energy 状态机必须写入 `Burned` record。
  - `Burned` energy 必须为 `0`。
  - 任意历史查询命中 burn 后高度，不得继续返回 burn 前可用能量。
- 下一步：
  - Review `doc/UIP/UIP-0002-pass-state-machine.md` 中的 burn 终态规则。
  - 增加 `PassEnergyManager::on_pass_burned` 或等价接口。
  - 更新 burn 相关测试断言。
- 验收：
  - burn 后 pass snapshot 和 energy snapshot 状态一致。
  - validator payload 不会使用 burned pass 的旧能量。

### ECO-005. 明确并实现 energy penalty v2 公式

- 优先级：`P0`
- 状态：`In Progress`
- 当前现状：
  - UIP-0003 已确认采用离散 `0.001 BTC` unit 增长模型。
  - 当前实现仍是达到阈值后的 sat 级线性增长，需要调整为 `balance_units`。
  - 当前惩罚是固定窗口近似：`abs(delta_sats) * 43_200_000`。
  - UIP-0003 已确认 `ENERGY_PER_UNIT_BLOCK = 1`，并采用 `penalty = floor(lost_units * age_blocks * 3 / 2)`。
  - 余额减少后按剩余 units 比例调整 `active_block_height`。
  - 相关 GitHub 讨论：[#27](https://github.com/buckyos/usdb/issues/27)。
- 目标：
  - 将公式实现切换到 `uint128` energy 和 unit delta 快照计算。
  - 将 RPC / validator payload / 前端 energy 表示切换为 canonical decimal string。
  - 明确 `active_block_height'` 的更新公式。
  - 当前开发阶段从高度 `0` 激活新公式；未来正式升级再交给 UIP-0008。
- 下一步：
  - 基于 `doc/UIP/UIP-0003-pass-energy-formula.md` 修改 `energy_formula.rs`、`energy.rs` 和 energy storage/RPC 类型。
  - 增加 unit 边界、正向增资 settlement、部分减仓和 `uint128` decimal string 测试。
- 验收：
  - 有参数化公式单测、unit 边界测试和 timeline 测试。
  - RPC、validator payload 和前端都不再用 JSON number 承载 energy。

### ECO-006. 明确并实现继承折损规则

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - 目标文档建议 `INHERIT_DISCOUNT = 0.05`。
  - 当前实现是全额继承 dormant energy。
- 目标：
  - 明确折损率、rounding 和多 `prev` 累加顺序。
  - 明确旧版本和新版本的差异。
- 下一步：
  - Review `doc/UIP/UIP-0003-pass-energy-formula.md` 中的 `INHERIT_DISCOUNT_BPS`、逐项 rounding 和多 `prev` 累加规则。
  - 再修改 consume/inherit 路径。
- 验收：
  - 多 prev 继承、单 prev 继承、边界 rounding 都有测试。

### ECO-007. 定义 collab pass 与 leader 绑定协议

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - `eth_collab` 目前只是可选 EVM 地址字段。
  - 目标模型要求 collab pass 创建时绑定 Leader 引用。
- 目标：
  - 明确 collab pass 如何通过 `leader_pass_id` / `leader_btc_addr` 二选一表达 leader 引用。
  - 明确 ETHW Leader eligibility 不反向进入 USDB indexer 派生能量。
  - 明确 collab 退出和 collab 转普通 pass 统一走 remint + `prev`。
  - 明确 collab pass 自身是否可独立参与 candidate set。
- 下一步：
  - Review UIP-0001 中 standard/collab 互斥字段规则。
  - 基于 `doc/UIP/UIP-0004-collab-leader-effective-energy.md` 实现 Leader 解析、地址跟随和 collab contribution 聚合。
- 验收：
  - collab pass 的基础 energy 与 effective energy 不会双重计数。
  - old collab consumed 后不再向旧 Leader 贡献 `collab_contribution`。

### ECO-008. 定义并实现 effective_energy / level / real_difficulty

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - 当前 leaderboard 和 validator 样例主要使用 raw `energy`。
  - 目标模型需要 `effective_energy`、`level`、`real_difficulty`。
  - `doc/UIP/UIP-0004-collab-leader-effective-energy.md` 已定义 `effective_energy`。
  - `doc/UIP/UIP-0005-level-and-real-difficulty.md` 已进入 Draft，并与 UIP-0003 unit-block energy 量纲对齐。
  - 已确认 `MAX_LEVEL = 50`，最大 difficulty discount = 50%。
  - 已确认 usdb-indexer 不读取 ETHW `base_difficulty`，只动态派生 `level` 和 `difficulty_factor_bps`。
  - 相关 GitHub 讨论：[#27](https://github.com/buckyos/usdb/issues/27)。
- 目标：
  - 定义 `level(effective_energy)` 的整数或定点计算方式。
  - 定义 `difficulty_factor_bps(level)` 的下界，即 `MIN_DIFFICULTY_FACTOR_BPS = 5000`。
  - 明确 RPC 与 validator payload 同时返回 `raw_energy`、`collab_contribution`、`effective_energy`、`level` 和 `difficulty_factor_bps`。
  - 明确 ETHW 侧基于 `base_difficulty` 和 `difficulty_factor_bps` 计算 `real_difficulty`。
- 下一步：
  - 基于 UIP-0004 增加可审计的 `collab_contribution` 明细查询或 payload 字段。
  - 增加 RPC 动态派生字段和 validator payload 字段。
  - 在 UIP-0006 中定义 USDB-side state view，并在 UIP-0007 或后续 ETHW policy UIP 中定义链上 payload / difficulty 编码。
- 验收：
  - 单元测试覆盖 level 边界、rounding、最大折扣。
  - candidate set 选择规则使用协议指定字段。

### ECO-009. 建立经济公式版本与激活高度治理

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - `USDB_INDEX_FORMULA_VERSION` 当前是全局常量 `pass-energy-formula:v1`。
  - 文档要求公式变更必须通过显式激活高度或治理决议，不应代码发布即生效。
  - `doc/UIP/UIP-0008-protocol-versioning-and-activation-matrix.md` 已进入 Draft。
  - 已确认 `local_state_commit` 只需要承诺 `active_version_set_id`，不需要内联完整 `active_version_set`。
  - 已确认首个正式 ETHW 网络必须启用 level-based difficulty policy，不保留 `difficulty_policy_version = 0` 语义。
- 目标：
  - 建立公式版本、协议版本、查询语义版本之间的关系。
  - 明确历史高度按当时激活版本重放。
  - 明确 state ref / snapshot id 是否包含激活版本表。
  - 明确 `active_version_set`、`activation_registry_id` 和 `local_state_commit` 的关系。
- 下一步：
  - Review UIP-0008 中 version family、activation record、state commit 绑定和跨版本 `prev` 继承规则。
  - 再更新共识 identity 和历史查询路径。
- 验收：
  - 同一节点能对不同历史高度按对应公式版本查询。
  - validator payload version mismatch 路径可覆盖经济公式版本。

### ECO-010. CoinBase、K、分账、price / real_price、辅助算力池拆分

- 优先级：`P2`
- 状态：`In Progress`
- 当前现状：
  - 目标经济模型已经写出方向，但大量参数和证明格式仍是 `<TODO>`。
  - 当前代码侧尚未完整实现这些机制。
  - SourceDAO / Dividend bootstrap 已拆到 `UIP-0010` 优先处理。
  - CoinBase emission 与 reward / fee split 公式后移到 `UIP-0011` 及后续 economic UIP。
  - `doc/UIP/UIP-0011-coinbase-emission-and-reward-split.md` 已进入 Draft。
  - 动态 `K` 已拆到 `doc/UIP/UIP-0012-collaboration-efficiency-coefficient.md`。
  - `price` / `real_price` 顶层状态与 fixed price 启动策略已拆到 `doc/UIP/UIP-0013-price-and-real-price-update-rules.md`。
  - Leader 主动报价活跃窗口和 candidate energy 策略已拆到 `doc/UIP/UIP-0014-leader-quote-activity-and-candidate-energy.md`。
- 目标：
  - 将发行、分账、价格、辅助算力池拆成独立 UIP。
  - 每个 UIP 必须有确定性输入、整数公式、验证路径和 reorg 语义。
- 下一步：
  - Review UIP-0011 中 `total_miner_btc_sats`、`issued_usdb_atoms`、`K`、fee split、aux pool 和 uncle reward 边界。
  - Review UIP-0012 中 `compute_k_bps`、warmup activation delay 和 `K_LAST_*` 审计 slots。
  - Review UIP-0013 中 price policy range、fixed price 升级、parent state price 和 dynamic source policy 边界。
  - Review UIP-0014 中 quote window、quote subject、candidate energy 和 quote payload encoding。
  - 继续起草 UIP-0015，固定辅助算力池证明和 75% / 25% CoinBase split 的激活条件。
- 验收：
  - 每个机制都有独立协议文档、实现计划和测试计划。

### ECO-011. 拆分 USDB 经济状态视图与 ETHW 链上 payload

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - 当前 validator block-body 文档已经覆盖 `external_state`、`miner_selection.energy` 和 candidate set。
  - 经济模型后续会引入 effective energy、level、difficulty、reward、price 等字段。
  - 已决定拆分为 USDB-side state view 和 ETHW-side on-chain payload 两层，避免把审计字段和区块头字段混在一个 UIP 中。
  - `doc/UIP/UIP-0006-usdb-economic-state-view.md` 已进入 Draft。
  - `doc/UIP/UIP-0007-ethw-consensus-profile-selector.md` 已进入 Draft。
  - 已确认 `stable_block_hash` 不进入 UIP-0007 v1 header payload，由 UIP-0006 state view 返回。
  - 已确认 reward rule 与 future difficulty policy 复用同一 profile selector。
  - 已确认 `ProfileSelectorPayload` 是正式 payload 命名；当前 go-ethereum 原型中的 `RewardPayloadV1` 应在正式实现前重命名。
  - 已确认 future difficulty policy 使用独立 `difficulty_policy_version`；该字段进入 UIP-0007 payload 作为显式承诺，但必须匹配 ETHW chain config / fork policy 的 expected version。
  - 已确认 collab bonus 不在 header 中携带全量 `collab_pass_id`。
  - `doc/UIP/UIP-0009-ethw-chain-config-and-usdb-bootstrap.md` 已进入 Draft，用于承接 ETHW chain config、genesis 和 USDB consensus version 激活。
- 目标：
  - 明确 USDB indexer 可以提供的完整经济状态 / 审计视图。
  - 明确 ETHW `header.Extra` 只携带最小历史 selector。
  - 明确哪些字段由 validator 通过 UIP-0006 本地重算，不需要进入 ETHW 链上 payload。
  - 明确 tamper 测试和 mismatch 错误。
- 下一步：
  - Review UIP-0006 中 state view 字段、collab breakdown 和错误语义。
  - Review UIP-0007 中 profile selector 二进制布局和 validator replay 流程。
  - Review UIP-0009 中 ETHW chain config、payload version、reward rule version 和 expected difficulty policy version 的激活规则。
  - 在实现阶段基于数据库索引和性能测试确定 `get_collab_breakdown` 排序策略、分页 cursor 和最大 `limit`。
- 验收：
  - USDB state view 可在历史 context 下重放。
  - ETHW profile selector 只用最小字段即可重放 reward input，并可供 future difficulty policy 复用。

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

### ECO-013. 标准化 SourceDAO / Dividend / fee split 冷启动流程

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - 当前 docker bootstrap 已有开发期流程：复制 canonical ETHW genesis artifact，执行 `geth init`，启动 ETHW 节点，再由 `sourcedao-bootstrap` 调用 SourceDAO 工作区脚本完成 Dao / Dividend 初始化。
  - go-ethereum 原型已有 `USDBBootstrapGenesisConfig`、`DividendAddress`、`DividendFeeSplitBlock` 等实现入口。
  - 该流程目前依赖本地 SourceDAO workspace、外部 bootstrap config 和开发期 manifest，还不是正式协议标准。
  - 已决定将 SourceDAO / Dividend bootstrap 提前作为 `UIP-0010`，原 CoinBase / reward split 后移到 `UIP-0011`。
- 目标：
  - 单独起草 UIP，定义固定系统地址、SourceDAO / Dividend runtime code 来源、bootstrap admin、初始化交易顺序、fee split activation height 和 release artifact。
  - 明确后续 joiner 如何验证 canonical genesis、SourceDAO bootstrap 状态和 fee split 激活状态。
  - 明确 UIP-0009 只负责 chain config / genesis 边界，不直接定义 SourceDAO 业务初始化细节。
- 下一步：
  - Review `doc/UIP/UIP-0010-source-dao-dividend-bootstrap.md` 中的 artifact、bootstrap state、activation height 和 joiner validation 章节。
  - 确认 public testnet / mainnet 的 `DaoAddress`、`DividendAddress`、`DividendFeeSplitBlock` 和 bootstrap admin 治理方式。
- 验收：
  - 有独立协议文档覆盖 genesis predeploy、post-start bootstrap tx、activation height 和 joiner audit。
  - docker 本地 bootstrap 流程能映射到协议中的每个 artifact 和状态 marker。

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

建议下一轮继续 review `UIP-0010` 的待审计问题：

1. 确认 public testnet / mainnet 的 SourceDAO 系统地址。
2. 确认 SourceDAO artifact hash / runtime code hash 的 canonical encoding。
3. 确认 bootstrap admin 使用临时账户、多签还是治理合约。
4. 确认 `DividendFeeSplitBlock` 与 bootstrap 完成高度之间的安全间隔。
