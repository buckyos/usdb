# USDB UIP 拆分设计

## 1. 目的

本文档用于把当前 `doc/usdb-economic-model-design.md` 中混合在一起的目标经济模型拆分成一组可逐步标准化、实现和验证的 UIP。

它是拆分设计，不是最终协议正文。后续正式协议应独立成文，并使用稳定编号、状态和激活规则。

## 2. 拆分原则

1. **先共识核心，后经济扩展**：先处理会直接影响 pass 状态、energy、validator 选择和历史重放的规则，再处理 CoinBase、分账、价格和辅助算力池。
2. **先 schema，再状态，再公式**：铭文格式必须先固定，否则状态机和能量继承规则没有稳定输入。
3. **每个 UIP 必须可测试**：正式 UIP 不只写目标语义，还要列出实现入口、历史查询语义、reorg 语义和测试矩阵。
4. **版本和激活必须显式**：影响共识结果的 UIP 必须定义激活高度或治理激活方式，不得以代码发布即生效。
5. **查询字段和共识字段分离**：排行榜、UI 缓存、展示格式等不得反向影响 validator 或奖励结算。

## 3. 建议状态流

正式 UIP 建议使用以下状态：

| 状态 | 含义 |
| --- | --- |
| `Draft` | 初稿，允许大幅调整。 |
| `Review` | 已进入协议 review，字段和公式应趋于稳定。 |
| `Last Call` | 准备冻结，只接受关键问题修正。 |
| `Final` | 已成为正式协议。 |
| `Deferred` | 暂缓推进。 |
| `Withdrawn` | 主动撤回。 |
| `Superseded` | 被后续 UIP 替代。 |

## 4. 建议 UIP 类型

| 类型 | 用途 |
| --- | --- |
| `Standards Track` | 影响协议、共识、索引、验证、经济结算的规范。 |
| `Informational` | 背景、设计说明、运营约定，不直接改变共识。 |
| `Process` | UIP 流程、编号、治理和激活规则。 |

## 5. 正式 UIP 头部建议

```text
UIP: UIP-0001
Title: Miner Pass Inscription Schema
Status: Draft
Type: Standards Track
Layer: Application / Consensus
Created: YYYY-MM-DD
Requires: <optional>
Supersedes: <optional>
Activation: <height/governance/TODO>
```

字段说明：

- `UIP`：稳定编号。
- `Status`：状态流中的一个值。
- `Type`：Standards Track / Informational / Process。
- `Layer`：协议影响层，例如 Application、Consensus、RPC、Validator。
- `Activation`：影响共识结果时必须明确。

## 6. 拆分路线图

| 顺序 | 建议编号 | 标题 | 类型 | 优先级 | 当前状态 |
| --- | --- | --- | --- | --- | --- |
| 0 | `UIP-0000` | UIP Process and Governance | Process | P0 | Draft |
| 1 | `UIP-0001` | Miner Pass Inscription Schema | Standards Track | P0 | Draft |
| 2 | `UIP-0002` | Miner Pass State Machine | Standards Track | P0 | Draft |
| 3 | `UIP-0003` | Pass Energy Formula and Inheritance | Standards Track | P0 | Draft |
| 4 | `UIP-0004` | Collab Pass, Leader, and Effective Energy | Standards Track | P1 | Draft |
| 5 | `UIP-0005` | Level and Real Difficulty | Standards Track | P1 | Draft |
| 6 | `UIP-0006` | USDB Economic State View | Standards Track | P1 | Draft |
| 7 | `UIP-0007` | ETHW Consensus Profile Selector | Standards Track | P1 | Draft |
| 8 | `UIP-0008` | Protocol Versioning and Activation Matrix | Process / Standards Track | P1 | Draft |
| 9 | `UIP-0009` | ETHW Chain Config and USDB Bootstrap | Standards Track | P1 | Draft |
| 10 | `UIP-0010` | SourceDAO and Dividend Bootstrap | Standards Track | P1 | Draft |
| 11 | `UIP-0011` | CoinBase Emission and Reward Split | Standards Track | P2 | Draft |
| 12 | `UIP-0012` | Collaboration Efficiency Coefficient K | Standards Track | P2 | Draft |
| 13 | `UIP-0013` | Price and Real Price Update Rules | Standards Track | P2 | Draft |
| 14 | `UIP-0014` | Leader Quote Activity and Candidate Energy Policy | Standards Track | P1 | Draft |
| 15 | `UIP-0015` | Auxiliary Hashpower Pool | Standards Track | P2 | Draft |

## 7. UIP-0000: UIP Process and Governance

目标：

- 定义 UIP 编号、状态、类型、模板。
- 定义协议升级流程。
- 定义 Draft 到 Final 的 review 条件。
- 定义激活高度、治理决议和回滚策略。

需要解决：

- `UBIP` 与 `UIP` 命名统一。
- 正式协议文档与设计讨论稿的优先级。
- 影响共识的变更如何绑定 `protocol_version` / `formula_version`。

当前草案：

- `doc/UIP/UIP-0000-uip-process.md`

## 8. UIP-0001: Miner Pass Inscription Schema

目标：

- 定义矿工证铭文 JSON schema。
- 固定 `p`、`op`、`v`、`eth_main`、`leader_pass_id`、`leader_btc_addr`、`prev` 等字段语义。
- 明确可选字段默认值、未知字段策略、重复字段策略和 content-type。

需要解决：

- `prev` 在文档中可选，但当前实现中缺失会 invalid。
- `eth_collab` 当前只是地址字段，无法表达 leader 绑定。
- 当前草案已采用 `leader_pass_id` / `leader_btc_addr` 二选一作为协作绑定字段，并在激活后禁止新 `eth_collab`。
- 当前草案已将开发期旧格式排除在正式协议版本序列之外。

当前草案：

- `doc/UIP/UIP-0001-miner-pass-inscription.md`

实现影响：

- `src/btc/usdb-indexer/src/index/content.rs`
- `src/btc/usdb-indexer/src/inscription/source.rs`
- `doc/矿工证铭文协议.md`

测试要求：

- valid schema。
- missing optional fields。
- invalid ETH address。
- invalid `prev` inscription id。
- unknown fields。
- version mismatch。

## 9. UIP-0002: Miner Pass State Machine

目标：

- 定义 `Active / Dormant / Consumed / Burned / Invalid` 的正式状态机。
- 定义 mint、transfer、same-owner transfer、burn、remint(prev) 的状态转换。
- 明确单 owner 单 active pass 规则。
- 明确同一 block 内事件排序规则。

需要解决：

- `prev` 无效时是整次 mint invalid，而不是部分继承。
- owner 一致性到底是 BTC owner、控制权、还是 lineage。
- Burned pass 是否可被引用，以及引用时如何 invalid。
- 同 block transfer + mint 的排序必须成为协议规则。

当前草案：

- `doc/UIP/UIP-0002-pass-state-machine.md`

实现影响：

- `src/btc/usdb-indexer/src/index/pass.rs`
- `src/btc/usdb-indexer/src/index/indexer/block_events.rs`
- `src/btc/usdb-indexer/src/storage/pass.rs`

测试要求：

- same owner multi mint。
- passive transfer。
- transfer then remint。
- duplicate prev。
- burned prev。
- same-block ordering。

## 10. UIP-0003: Pass Energy Formula and Inheritance

目标：

- 定义 energy 增长、惩罚、继承和终态语义。
- 明确所有公式使用整数或定点数，不使用浮点非确定性计算。
- 明确 `Dormant / Consumed / Burned` 查询语义。
- 明确 `raw_energy` 是唯一可继承能量，`effective_energy` 和 collab contribution 不得写回 raw energy。

建议公式拆分：

```text
balance_units = floor(owner_balance_sats / 100_000)

growth_delta
    = balance_units * block_delta
```

惩罚目标：

```text
units_before = floor(balance_before_sats / 100_000)
units_after  = floor(balance_after_sats  / 100_000)
lost_units   = max(0, units_before - units_after)

penalty = floor(lost_units * age_blocks * 3 / 2)
```

继承目标：

```text
inherit(prev_i) = floor(raw_energy(prev_i) * 9500 / 10000)
```

需要解决：

- `uint128` energy 在 RocksDB、RPC、validator payload 和前端展示中的迁移。
- decimal string 的 canonical encoding。
- `active_block_height` 在部分减仓时的比例更新。
- Burned energy 终态是否强制为 0。
- 开发期从高度 `0` 激活后，旧公式数据如何重建。

当前草案：

- `doc/UIP/UIP-0003-pass-energy-formula.md`

实现影响：

- `src/btc/usdb-indexer/src/index/energy_formula.rs`
- `src/btc/usdb-indexer/src/index/energy.rs`
- `src/btc/usdb-indexer/src/storage/energy.rs`

测试要求：

- 增长阈值。
- 正向增资。
- 部分减仓。
- 全部减仓。
- 多 prev 继承。
- 继承折损 rounding。
- burn 后 energy 为 0。

## 11. UIP-0004: Collab Pass, Leader, and Effective Energy

目标：

- 定义标准矿工证和协作矿工证的区别。
- 定义 collab pass 创建时如何绑定 leader。
- 定义 `collab_contribution` 与 `effective_energy`。
- 明确 `effective_energy` 是派生值，不可继承，不得进入 raw energy ledger。
- 明确 ETHW Leader eligibility 不反向进入 USDB indexer 派生能量。

候选规则：

```text
collab_contribution
    = floor(raw_energy(collab_i) * 5000 / 10000)

effective_energy
    = raw_energy(leader) + sum(collab_contribution_i)
```

需要解决：

- `leader_btc_addr` 自动跟随 remint 后，如何证明 collab contribution 不会被重复继承。
- collab pass remint 为 standard 或新 collab 后，旧 contribution 如何归零。
- payload / 查询如何同时暴露 `raw_energy`、`collab_contribution`、`effective_energy` 和审计明细。
- ETHW 侧如何在 UIP-0005 / UIP-0006 中基于出块历史判断 Leader eligibility。

当前草案：

- `doc/UIP/UIP-0004-collab-leader-effective-energy.md`

实现影响：

- pass storage schema。
- energy leaderboard。
- validator candidate set。
- RPC pass snapshot / energy snapshot。

测试要求：

- collab energy 不双重计数。
- leader valid / invalid window。
- collab exit。
- leader transfer / burn / remint 后的绑定语义。

## 12. UIP-0005: Level and Real Difficulty

当前草案：

- `doc/UIP/UIP-0005-level-and-real-difficulty.md`

目标：

- 定义 `level(effective_energy)`。
- 定义 `difficulty_factor_bps(level)`。
- 定义 ETHW 侧 `real_difficulty` 折算规则。
- 定义下界约束。

当前公式草案：

```text
level_threshold(0) = 0
level_threshold(L) = ceil(E0 * Σ(i = 0..L-1) q^i)
level = max L where effective_energy >= level_threshold(L)

difficulty_factor_bps = max(5000, 10000 - level * 100)
real_difficulty = ceil(base_difficulty * difficulty_factor_bps / 10000)
```

需要解决：

- 已用整数阈值表替代非确定性 `log`。
- 当前草案已确认采用 `MAX_LEVEL = 50` 和 `MIN_DIFFICULTY_FACTOR_BPS = 5000`。
- UIP-0003 已采用 `ENERGY_PER_UNIT_BLOCK = 1`，与 issue #23 的 `E0 = 1_000_000` 量纲匹配。
- usdb-indexer 只动态派生 `level` 和 `difficulty_factor_bps`，不持久化，也不读取 ETHW `base_difficulty`。
- `real_difficulty` 由 ETHW validator / mining policy 基于当前 `base_difficulty` 计算。
- ETHW `base_difficulty` / `real_difficulty` 的来源、编码和是否显式承诺留给 UIP-0009 或后续 ETHW difficulty policy UIP。

实现影响：

- RPC。
- UIP-0006 state view。
- UIP-0007 profile selector。
- mining difficulty integration。

测试要求：

- level 边界表。
- max level。
- real difficulty lower bound。

## 13. UIP-0006: USDB Economic State View

当前草案：

- `doc/UIP/UIP-0006-usdb-economic-state-view.md`

目标：

- 定义 `usdb-indexer` 对外提供的经济状态视图。
- 明确 USDB-side 能查询和审计的字段集合。
- 明确 historical context、version mismatch、history unavailable 的错误行为。
- 避免把 USDB-side 审计视图与 ETHW 链上 payload 混为一体。

需要纳入的字段：

- BTC external state。
- pass snapshot。
- raw energy。
- collab contribution。
- effective energy。
- level。
- difficulty factor。
- collab breakdown。
- optional candidate set audit view。
- formula version。
- protocol version。
- view version。

需要解决：

- candidate set audit view 作为 usdb-indexer 一等查询后的性能参数、分页 cursor 和 `max_limit`。
- view 与 historical state ref 的绑定方式。
- collab breakdown 通过单独确定性分页查询提供后的可选排序策略和索引成本。
- owner 字段采用 `owner_script_hash` 作为 canonical id，并在可确定时返回 `owner_btc_addr`。
- script hash -> BTC address 反向索引作为后续独立议题，不阻塞 core state view。

实现影响：

- `doc/usdb-indexer/usdb-indexer-validator-block-body-e2e-design.md`
- `src/btc/usdb-indexer/scripts/regtest_live_ord_validator_*`
- RPC `ConsensusQueryContext`

测试要求：

- economic field recompute。
- version mismatch。
- historical context。
- reorg mismatch。

## 14. UIP-0007: ETHW Consensus Profile Selector

当前草案：

- `doc/UIP/UIP-0007-ethw-consensus-profile-selector.md`

目标：

- 定义 ETHW `header.Extra` 中的最小 USDB consensus profile selector。
- 定义正式的 `ProfileSelectorPayload` 固定二进制编码。
- 明确链上 payload 只携带 selector，不携带完整经济审计字段。
- 明确 validator 如何通过 UIP-0006 state view 重算 reward input 和 future difficulty input。

当前 v1 字段：

- `payload_version`。
- `difficulty_policy_version`。
- `btc_height`。
- `snapshot_id`。
- `system_state_id`。
- `pass_id`。

需要解决：

- v1 不携带 `stable_block_hash`，由 UIP-0006 state view 返回审计字段。
- reward rule 与 future difficulty policy 复用同一 selector。
- `difficulty_policy_version` 进入 payload 作为显式承诺，但必须匹配 chain config expected version。
- collab bonus 不在 header 中携带全量 `collab_pass_id` 列表。
- payload version 与 ETHW chain config / reward rule version 的边界。

实现影响：

- `/home/bucky/work/go-ethereum/internal/usdb/payload.go`
- `/home/bucky/work/go-ethereum/internal/usdb/verifier.go`
- `/home/bucky/work/go-ethereum/miner/worker.go`
- `/home/bucky/work/go-ethereum/consensus/ethash/consensus.go`

测试要求：

- binary roundtrip。
- invalid version / invalid size。
- historical USDB replay。
- USDB unavailable fail-closed。
- BTC reorg mismatch。

## 15. UIP-0008: Protocol Versioning and Activation Matrix

当前草案：

- `doc/UIP/UIP-0008-protocol-versioning-and-activation-matrix.md`

目标：

- 定义协议版本、公式版本和查询语义版本。
- 定义激活高度和历史重放规则。
- 定义 state ref / snapshot id 如何承诺公式版本。
- 定义 `active_version_set`、`activation_registry_id` 和 `local_state_commit` 的关系。

需要解决：

- 当前 `USDB_INDEX_FORMULA_VERSION` 是全局常量。
- 历史高度在公式升级后如何按旧公式重放。
- 旧 pass 和新 pass 是否允许跨版本继承。
- `activation_registry_id` / `active_version_set_id` 的 canonical encoding 何时固定。
- 机器可读 activation registry 是否作为纯文档资产先落地。

实现影响：

- `src/btc/usdb-util/src/types.rs`
- `src/btc/usdb-indexer/src/service/rpc.rs`
- `balance-history` state ref identity。

测试要求：

- 不同高度不同公式版本。
- expected formula version mismatch。
- rollback 后版本重放一致。

## 16. UIP-0009: ETHW Chain Config and USDB Bootstrap

当前草案：

- `doc/UIP/UIP-0009-ethw-chain-config-and-usdb-bootstrap.md`

目标：

- 定义 USDB ETHW 链的 chain config 扩展字段。
- 定义 ChainID、NetworkId、genesis、PoW 基础参数和 USDB reward 开关。
- 定义 active payload version、reward rule version 和 expected difficulty policy version。
- 定义这些版本从 genesis 生效还是在后续 fork 高度生效。
- 明确 USDB 新链不复用 ETHW / Merge 迁移语义。

需要解决：

- ETHW fork 遗留字段如何收口。
- `ProfileSelectorPayload` version 如何进入 chain config。
- `reward_rule_version` 与 UIP-0007 `payload_version` 的边界。
- expected `difficulty_policy_version` 的 chain config 表达和激活高度。
- public network 最终 ChainID、NetworkId、genesis difficulty 和 bootnodes。
- `MaximumExtraDataSize = 160` 已作为 UIP-0009 v1 固定上限。
- SourceDAO / Dividend / fee split 冷启动应拆到后续独立 UIP；UIP-0009 只保留 chain config hook 和 genesis/bootstrap 边界。

实现影响：

- `/home/bucky/work/go-ethereum/params/config.go`
- `/home/bucky/work/go-ethereum/core/genesis.go`
- `/home/bucky/work/go-ethereum/consensus/ethash`
- `/home/bucky/work/go-ethereum/docs/usdb/usdb-chain-bootstrap-notes.md`

测试要求：

- USDB genesis / chain config roundtrip。
- USDB reward rule 从 genesis 生效。
- payload version / reward rule version mismatch。

## 17. UIP-0010: SourceDAO and Dividend Bootstrap

目标：

- 定义 SourceDAO / Dividend system contract 的冷启动流程。
- 定义固定系统地址、genesis predeploy runtime code、bootstrap admin、bootstrap 交易顺序和 fee split activation height。
- 定义 canonical genesis artifact、SourceDAO bootstrap config、bootstrap state marker 和后续 joiner 审计方式。
- 明确 fee split activation 的启动条件，但不定义手续费比例和 CoinBase 释放公式。

当前草案：

- `doc/UIP/UIP-0010-source-dao-dividend-bootstrap.md`

需要解决：

- public network 的 `DaoAddress` / `DividendAddress` 最终取值。
- SourceDAO artifact hash / runtime code hash 的 canonical encoding。
- bootstrap admin 的权限生命周期和私钥治理。
- `DividendFeeSplitBlock` 是否必须大于 bootstrap 完成高度，以及最小安全间隔。
- SourceDAO full bootstrap 中其他模块是否进入本 UIP，还是只把 Dao / Dividend 作为 fee split 前置条件。

实现影响：

- `/home/bucky/work/go-ethereum/cmd/geth/usdbbootstrap.go`
- `/home/bucky/work/go-ethereum/core/genesis.go`
- `/home/bucky/work/go-ethereum/params/config.go`
- `/home/bucky/work/SourceDAO/scripts/usdb_bootstrap_full.ts`
- `docker/compose.bootstrap.yml`

测试要求：

- canonical genesis 可复现。
- genesis predeploy code hash 与 manifest 一致。
- bootstrap tx 顺序正确。
- activation block 前后 fee split 状态可验证。
- 新 joiner 能通过 genesis、manifest 和链上状态审计 bootstrap 完成状态。

## 18. UIP-0011: CoinBase Emission and Reward Split

目标：

- 定义 CoinBase 释放公式。
- 定义矿工、辅助算力池、DAO 分红池、手续费分配。
- 定义叔块奖励兼容边界。

需要解决：

- `TOTAL_MINER_BTC` 统计口径。
- `ISSUED_USDB` 来源和 reorg 回滚。
- `K` 函数。
- 叔块奖励采用哪一版规则。
- 收入合约是否进入共识验证。

当前草案：

- `doc/UIP/UIP-0011-coinbase-emission-and-reward-split.md`

当前草案倾向：

- 使用整数 `atoms` / `sats` 公式，不使用浮点数。
- `reward_recipient` 来自 standard pass 的 `eth_main`，且必须等于 `header.Coinbase`。
- `CoinBase` 使用 `target_supply_atoms - issued_usdb_atoms` 的剩余目标供应量计算。
- fee split 激活后手续费按 `miner=60%`、`DAO/Dividend=40%` 分配，整除余数归矿工。
- 动态 `K` 已拆到 `UIP-0012`，UIP-0011 只消费 `k_bps`。
- 在 UIP-0015 Final 前不启用 aux pool split。
- uncle / ommer reward 在规则未固定前建议禁用或置 `0`。

建议延后原因：

- 依赖 pass / energy / leader / validator payload 稳定。
- 依赖 UIP-0010 先确定分红池地址、bootstrap 状态和 fee split activation 边界。

## 19. UIP-0012: Collaboration Efficiency Coefficient K

目标：

- 定义 CoinBase 公式中的协作效率系数 `K`。
- 定义 `CE_N`、`AE_N`、rolling window 和 warmup 规则。
- 定义 `K` state 如何存入 ETHW reserved system storage 并由 `stateRoot` 承诺。
- 固定 v1 `CE_N = collab_contribution` 的样本口径。

需要解决：

- `compute_k_bps` 整数公式。
- warmup 阶段是否需要 activation delay。
- optional `K_LAST_*` 审计 slots 是否进入 v1。

当前草案：

- `doc/UIP/UIP-0012-collaboration-efficiency-coefficient.md`

当前草案倾向：

- v1 使用 UIP-0006 的 `collab_contribution` 作为 `CE_N`。
- `AE_N` 使用过去固定 ETHW block 数量的 rolling window，不使用 wall-clock。
- `K_WINDOW_BLOCKS = 50400`，按 12 秒平均出块间隔对应 1 周。
- 窗口未填满或 `AE_N == 0` 时，`k_bps = 10000`。
- rolling window 使用 reserved system storage ring buffer，并随 `stateRoot` / reorg 自动回滚。

## 20. UIP-0013: Price and Real Price Update Rules

目标：

- 定义 `price` 与 `real_price`。
- 定义 `price_atoms_per_btc` 的本链 stateRoot 承诺边界。
- 定义按高度生效的 price policy range。
- 固定启动期 `FixedPrice` 策略。
- 预留外部以太坊 DeFi 和 USDB 自有 DeFi 两类后续 price source policy。

需要解决：

- dynamic price source 的独立 UIP 拆分。
- `PRICE_POLICY_RANGE_ID_SLOT` canonical encoding。
- parent state price 与 activation 边界块的精确执行顺序。
- 固定价格升级的治理/公告窗口。

当前草案：

- `doc/UIP/UIP-0013-price-and-real-price-update-rules.md`

当前草案倾向：

- v1 使用 `FixedPrice`，从 genesis 固定 `100_000 USDB / BTC`。
- 不预设 `PRICE_REPORT_START_HEIGHT`，后续由 activation range 决定。
- 必要时可以通过新增 `FixedPrice` range 调整启动期固定价格。
- 外部以太坊 DeFi 和 USDB 自有 DeFi 都是可选后续 policy，不强制按阶段顺序启用。
- UIP-0011 reward 只读取 parent state 中已经承诺的 `price_atoms_per_btc`。

## 21. UIP-0014: Leader Quote Activity and Candidate Energy Policy

目标：

- 定义 Leader 主动报价活跃窗口。
- 定义 stale Leader 如何从 `effective_energy` 回落到 `raw_energy`。
- 定义 `candidate_energy`、`candidate_level` 和 `candidate_difficulty_factor_bps`。
- 明确 FixedPrice v1 下 quote 只是 heartbeat，不更新 price。
- 明确 quote activity 是 ETHW 侧 state，不反向写入 USDB indexer。

需要解决：

- quote payload canonical encoding。
- quote subject 使用 pass id 还是 owner / BTC address。
- stale 后是否仅失去 collab energy，还是完全失去 Leader 资格。
- quote activity state 的 reserved storage key encoding。

当前草案：

- `doc/UIP/UIP-0014-leader-quote-activity-and-candidate-energy.md`

当前草案倾向：

- `LEADER_QUOTE_WINDOW_BLOCKS = 50400`，按 12 秒平均出块间隔对应 1 周。
- `candidate_energy = effective_energy` if leader quote active，否则 `candidate_energy = raw_energy`。
- `candidate_level` 从 `candidate_energy` 派生，ETHW difficulty policy 使用 `candidate_level`。
- block `N` 的有效 quote 最早影响 block `N+1`。
- FixedPrice v1 中 quote 必须等于 parent price，仅作为 heartbeat。
- v1 使用 active standard pass `pass_id` 作为 quote subject，不按 BTC owner / address 继承 quote activity。

## 22. UIP-0015: Auxiliary Hashpower Pool

目标：

- 定义辅助算力池提交格式。
- 定义有效算力证明。
- 定义奖励分配和反作弊。

需要解决：

- 最近 2 个 BTC 高度以内的证明如何验证。
- 有效算力大于 BTC 出块难度 75% 的证明格式。
- 多提交者竞争同一奖励如何处理。
- 算力证明和矿工证 owner 如何绑定。

当前草案：

- `doc/UIP/UIP-0015-auxiliary-hashpower-pool.md`

当前草案倾向：

- v1 public network 不默认启用辅助算力池，初始 `aux_pool_policy_version = 0`。
- 后续启用必须通过 UIP-0008 activation matrix 在指定 ETHW block height 激活 `aux_pool_policy_version > 0`。
- aux pool 不使用独立本地 `enabled` boolean；是否 active 由 policy version、recipient 和 verifier code hash 共同派生。
- UIP-0015 Final 前，UIP-0011 必须保持 `aux_pool_coinbase_atoms = 0`，CoinBase 100% 归 miner。
- 辅助算力证明不进入 UIP-0007 `header.Extra`，而是通过 system transaction / system contract 进入 ETHW state。
- accepted submissions 必须由 `stateRoot` 承诺，并支持 reorg 回滚。
- BTC reference validation 不得依赖 live BTC RPC，必须选择可历史重放的 BTC header / state commitment / proof segment 方案。
- 当前倾向使用 active miner pass `pass_id` 作为辅助算力提交绑定 subject。
- 75% 门槛、最近 2 个 BTC 高度窗口、多提交者竞争和无有效提交时 aux share 处理仍是待审计问题。

建议延后原因：

- 独立性较强，但实现和验证成本高，应在核心经济模型稳定后推进。

## 23. 推荐实施顺序

第一阶段：协议骨架

1. `UIP-0000`
2. `UIP-0001`
3. `UIP-0002`

第二阶段：当前实现收敛

1. `UIP-0003`
2. `UIP-0004`
3. 成对审计 raw energy、collab contribution、effective energy 的边界。
4. 更新 `doc/usdb-economic-model-issue-tracker.md` 中 ECO-005、ECO-006、ECO-007、ECO-008 状态。

第三阶段：validator 与挖矿选择

1. `UIP-0005`
2. `UIP-0006`
3. `UIP-0007`
4. `UIP-0008`
5. `UIP-0009`

第四阶段：完整经济系统

1. `UIP-0010`
2. `UIP-0011`
3. `UIP-0012`
4. `UIP-0013`
5. `UIP-0014`
6. `UIP-0015`

## 24. 与当前文档的关系

- `doc/usdb-economic-model-design.md`：目标经济模型总览和讨论稿。
- `doc/usdb-economic-model-issue-tracker.md`：问题、修复状态和下一步工作跟踪。
- `doc/UIP/uip-split-design.md`：正式 UIP 拆分路线图。
- 后续 `doc/UIP/UIP-*.md`：正式协议规范。

原则上，正式 UIP 进入 `Final` 后，其规范性优先级应高于讨论稿。
