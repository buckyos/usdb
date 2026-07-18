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
| ECO-002 | P0 | Done | 明确矿工证铭文 schema 与开发期无兼容策略 | `doc/UIP/UIP-0001-miner-pass-inscription.md`, `content.rs` |
| ECO-003 | P0 | Done | 将 `prev` 继承从 warn/skip 收敛为严格失败 | `doc/UIP/UIP-0002-pass-state-machine.md`, `pass.rs` |
| ECO-004 | P0 | Done | Burned 状态同步写入 energy 终态 | `doc/UIP/UIP-0002-pass-state-machine.md`, `energy.rs` |
| ECO-005 | P0 | Done | 明确并实现 energy penalty v2 公式 | `doc/UIP/UIP-0003-pass-energy-formula.md`, `energy_formula.rs`, `energy.rs` |
| ECO-006 | P1 | Done | 明确并实现继承折损规则 | `doc/UIP/UIP-0003-pass-energy-formula.md`, `pass.rs`, `energy.rs` |
| ECO-007 | P1 | Done | 定义并实现 collab pass 与 leader 绑定协议 | `doc/UIP/UIP-0001-miner-pass-inscription.md`, `doc/UIP/UIP-0004-collab-leader-effective-energy.md` |
| ECO-008 | P1 | In Progress | BTC effective_energy / level 已完成，ETHW real_difficulty 待后续 UIP | `doc/UIP/UIP-0004-collab-leader-effective-energy.md`, `doc/UIP/UIP-0005-level-and-real-difficulty.md`, RPC, state view, ETHW payload |
| ECO-009 | P1 | In Progress | 建立经济公式版本与激活高度治理 | `doc/UIP/UIP-0008-protocol-versioning-and-activation-matrix.md`, `usdb-util`, state ref |
| ECO-010 | P2 | In Progress | CoinBase、K、分账、price / real_price、辅助算力池拆分 | `doc/UIP/UIP-0011-*` 及后续 economic UIP |
| ECO-011 | P1 | In Progress | 拆分 USDB 经济状态视图与 USDB 链上 payload | `doc/UIP/UIP-0006-usdb-economic-state-view.md`, `doc/UIP/UIP-0007-ethw-consensus-profile-selector.md`, validator block-body docs/tests |
| ECO-012 | P1 | Done | 明确 JSON schema、content-type、重复 key 和未知字段策略 | inscription source/content parser |
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

### ECO-002. 明确矿工证铭文 schema 与开发期无兼容策略

- 优先级：`P0`
- 状态：`Done`
- 当前现状：
  - UIP-0001 v1 固定整数 `v: 1`，`prev` 缺省为空数组。
  - standard 与两种 collab Leader binding 使用互斥字段解析，并按当前 BTC network 校验 `leader_btc_addr`。
  - parser 拒绝未知字段、重复 top-level key、重复 `prev`、`usdb_collab` 和开发期旧 payload。
  - storage/RPC/client/control-plane/console 已移除旧字段，不保留旧 DB 或 payload 兼容路径。
- 目标：
  - 明确必填字段、可选字段、默认值，以及开发期旧格式不兼容、不迁移的策略。
  - 明确以 `leader_pass_id` / `leader_btc_addr` 二选一作为 leader 引用，并移除新协议中的 `usdb_collab`。
  - 明确开发期旧格式不作为正式协议版本进入 UIP 版本序列。
- 下一步：
  - 在集中 live/regtest 中复核真实 ord body 的 valid/invalid schema 矩阵。
  - 由 UIP-0008 固定公开网络 activation height 和 stable network id。
- 验收：
  - 有覆盖缺失 `prev`、未知字段、版本字段、collab 字段的单测。
  - 文档和 parser 行为一致。

### ECO-003. 将 `prev` 继承从 warn/skip 收敛为严格失败

- 优先级：`P0`
- 状态：`Done`
- 当前现状：
  - `prev` 在任何 mutation 前完成全量校验；任一引用无效都会让新 mint 进入 `Invalid`。
  - duplicate、missing、owner mismatch、非 Dormant、Consumed/Burned prev 均已覆盖，失败不会部分消费。
- 目标：
  - 在新协议版本下，任意 `prev` 无效都必须让本次 mint 进入 `Invalid`。
  - 明确所有权一致性是 owner 相同、控制权相同还是 lineage 相同。
  - 同一个 `prev` 在同一列表中重复出现必须 invalid。
- 下一步：
  - 在集中 live/regtest 中复核完整 prev invalid 矩阵和 same-block ordering。
- 验收：
  - 增加 owner mismatch、missing prev、already consumed、burned prev、duplicate prev 的严格 invalid 测试。
  - 开发期旧行为不保留兼容入口。

### ECO-004. Burned 状态必须同步写入 energy 终态

- 优先级：`P0`
- 状态：`Done`
- 当前现状：
  - Active / Dormant burn 在同一 event height 写入 `Burned / 0` energy record。
  - Consumed burn 保持 `Consumed / 0` 经济终态，不会恢复或覆盖旧能量。
  - pass snapshot、energy snapshot 和历史 projection 已有状态边界测试。
- 目标：
  - burn 发生时，energy 状态机必须写入 `Burned` record。
  - `Burned` energy 必须为 `0`。
  - 任意历史查询命中 burn 后高度，不得继续返回 burn 前可用能量。
- 下一步：
  - 在集中 live/regtest 中增加真实 ord burn 场景。
- 验收：
  - burn 后 pass snapshot 和 energy snapshot 状态一致。
  - validator payload 不会使用 burned pass 的旧能量。

### ECO-005. 明确并实现 energy penalty v2 公式

- 优先级：`P0`
- 状态：`Done`
- 当前现状：
  - 参考实现采用离散 `0.001 BTC` unit、`ENERGY_PER_UNIT_BLOCK = 1` 和 `penalty = floor(lost_units * age_blocks * 3 / 2)`。
  - unit delta 由 before/after snapshot 计算，部分减仓不按比例折算 `active_block_height`。
  - energy storage 使用 `u128`，跨语言接口使用 canonical decimal string。
  - 相关 GitHub 讨论：[#27](https://github.com/buckyos/usdb/issues/27)。
- 目标：
  - 将公式实现切换到 `uint128` energy 和 unit delta 快照计算。
  - 将 RPC / validator payload / 前端 energy 表示切换为 canonical decimal string。
  - 明确 `active_block_height'` 的更新公式。
  - 当前开发阶段从高度 `0` 激活新公式；未来正式升级再交给 UIP-0008。
- 下一步：
  - 在集中 live/regtest 与大数据运行中交叉验证公式、reorg 和 `u128` 表示。
- 验收：
  - 有参数化公式单测、unit 边界测试和 timeline 测试。
  - RPC、validator payload 和前端都不再用 JSON number 承载 energy。

### ECO-006. 明确并实现继承折损规则

- 优先级：`P1`
- 状态：`Done`
- 当前现状：
  - `INHERIT_DISCOUNT_BPS = 500`，每张 prev 独立执行 `floor(raw_energy * 9500 / 10000)` 后再饱和累加。
  - valid prev 在同一 event height 写入 `Consumed / 0`，新 pass 只继承折损后的 raw energy。
- 目标：
  - 明确折损率、rounding 和多 `prev` 累加顺序。
  - 明确旧版本和新版本的差异。
- 下一步：
  - 在集中 live/regtest 中复核单 prev、多 prev 和边界 rounding。
- 验收：
  - 多 prev 继承、单 prev 继承、边界 rounding 都有测试。

### ECO-007. 定义 collab pass 与 leader 绑定协议

- 优先级：`P1`
- 状态：`Done`
- 当前现状：
  - `usdb_collab` 已从 v1 schema、storage、RPC 和 client surfaces 移除。
  - collab pass 使用 `leader_pass_id` / `leader_btc_addr` 二选一；fixed/address 历史解析和 Leader eligibility 已实现。
  - active collab 自身不进入 candidate set，只按 `COLLAB_WEIGHT_BPS = 5000` 向唯一 Active standard Leader 提供 derived contribution。
- 目标：
  - 明确 collab pass 如何通过 `leader_pass_id` / `leader_btc_addr` 二选一表达 leader 引用。
  - 明确 USDB Leader eligibility 不反向进入 USDB indexer 派生能量。
  - 明确 collab 退出和 collab 转普通 pass 统一走 remint + `prev`。
  - 明确 collab pass 自身是否可独立参与 candidate set。
- 下一步：
  - 在集中 live/regtest 中补齐 address remint、fixed no-follow、consume/remint 和大规模 aggregate 场景。
- 验收：
  - collab pass 的基础 energy 与 effective energy 不会双重计数。
  - old collab consumed 后不再向旧 Leader 贡献 `collab_contribution`。

### ECO-008. 定义并实现 effective_energy / level / real_difficulty

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - UIP-0004 derived resolver、breakdown 和 candidate set 已实现 `effective_energy` 聚合、审计和排序。
  - UIP-0005 公式与 profile/candidate 查询已动态派生 `level` 和 `difficulty_factor_bps`，不写入 energy DB。
  - validator JSON payload 使用 raw/collab/effective 三字段，winner 使用 `effective_energy`；`MAX_LEVEL = 50`，最大 difficulty discount = 50%。
  - BTC/indexer 侧已完成；usdb-indexer 不读取 ETHW `base_difficulty`，ETHW `real_difficulty` policy 尚待后续 UIP。
  - 相关 GitHub 讨论：[#27](https://github.com/buckyos/usdb/issues/27)。
- 目标：
  - 定义 `level(effective_energy)` 的整数或定点计算方式。
  - 定义 `difficulty_factor_bps(level)` 的下界，即 `MIN_DIFFICULTY_FACTOR_BPS = 5000`。
  - 明确 RPC 与 validator payload 同时返回 `raw_energy`、`collab_contribution`、`effective_energy`、`level` 和 `difficulty_factor_bps`。
  - 明确 ETHW 侧基于 `base_difficulty` 和 `difficulty_factor_bps` 计算 `real_difficulty`。
- 下一步：
  - 在 UIP-0007/UIP-0009 或后续 ETHW policy 中固定 `base_difficulty / real_difficulty` 来源、编码和 mismatch 校验。
  - 集中 live/regtest 继续复核 candidate/profile/breakdown 的历史与 reorg 一致性。
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
  - 辅助算力池激活边界、证明纲要和 reward 分配待审计问题已拆到 `doc/UIP/UIP-0015-auxiliary-hashpower-pool.md`。
- 目标：
  - 将发行、分账、价格、辅助算力池拆成独立 UIP。
  - 每个 UIP 必须有确定性输入、整数公式、验证路径和 reorg 语义。
- 下一步：
  - Review UIP-0011 中 `total_miner_btc_sats`、`issued_usdb_atoms`、`K`、fee split、aux pool 和 uncle reward 边界。
  - Review UIP-0012 中 `compute_k_bps`、warmup activation delay 和 `K_LAST_*` 审计 slots。
  - Review UIP-0013 中 price policy range、fixed price 升级、parent state price 和 dynamic source policy 边界。
  - Review UIP-0014 中 quote window、quote subject、candidate energy 和 quote payload encoding。
  - Review UIP-0015 中 BTC reference validation、proof encoding、pass binding、多提交者分配和无有效提交时 aux share 处理。
- 验收：
  - 每个机制都有独立协议文档、实现计划和测试计划。

### ECO-011. 拆分 USDB 经济状态视图与 USDB 链上 payload

- 优先级：`P1`
- 状态：`In Progress`
- 当前现状：
  - UIP-0006 已实现 historical `external_state`、economic profile、candidate set、collab breakdown、opaque cursor、version/rule 校验和 typed client/CLI/control-plane capability。
  - validator JSON 测试 payload 已从 UIP-0006 profile/candidate view 构造，不再使用旧 raw `energy` 拼装路径。
  - USDB-side state view 与 ETHW-side on-chain payload 已明确分层；UIP-0006 indexer scope 完成，UIP-0007/UIP-0009 ETHW scope 仍待对齐。
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
  - 明确哪些字段由 validator 通过 UIP-0006 本地重算，不需要进入 USDB 链上 payload。
  - 明确 tamper 测试和 mismatch 错误。
- 下一步：
  - Review UIP-0007 中 profile selector 二进制布局和 validator replay 流程。
  - Review UIP-0009 中 ETHW chain config、payload version、reward rule version 和 expected difficulty policy version 的激活规则。
  - 执行完整 UIP-0001-0006 live/regtest，并基于大规模数据集评估 `contribution_desc_pass_id_asc` 的数据库索引成本。
- 验收：
  - USDB state view 可在历史 context 下重放。
  - ETHW profile selector 只用最小字段即可重放 reward input，并可供 future difficulty policy 复用。

### ECO-012. 明确 canonical JSON、content-type 和未知字段策略

- 优先级：`P1`
- 状态：`Done`
- 当前现状：
  - UIP-0001 要求 UTF-8 JSON object、精确字段名和 strict schema；字段顺序不参与语义，不要求 byte-level canonical serialization。
  - parser 显式拒绝未知字段、重复 top-level key、类型错误和重复 `prev`。
  - loader 接受 UTF-8 `application/json` / `text/plain` 或无可靠 content-type 的 source，但所有来源都进入同一 strict classifier。
- 目标：
  - 明确 inscription 内容的 JSON canonical 规则。
  - 明确支持的 content-type。
  - 明确未知字段在不同协议版本下是允许、忽略还是 invalid。
- 下一步：
  - 在集中 live/regtest 中复核 ord/bitcoind/fixture source 对同一 body 的分类一致性。
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
