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
| 6 | `UIP-0006` | Validator Economic Payload | Standards Track | P1 | Planned |
| 7 | `UIP-0007` | Formula Versioning and Activation | Process / Standards Track | P1 | Planned |
| 8 | `UIP-0008` | CoinBase Emission and Reward Split | Standards Track | P2 | Planned |
| 9 | `UIP-0009` | Price and Real Price Update Rules | Standards Track | P2 | Planned |
| 10 | `UIP-0010` | Auxiliary Hashpower Pool | Standards Track | P2 | Planned |

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
- ETHW payload 是否必须显式携带 `base_difficulty` / `real_difficulty` 留给 UIP-0006。

实现影响：

- RPC。
- validator payload。
- mining difficulty integration。

测试要求：

- level 边界表。
- max level。
- real difficulty lower bound。

## 13. UIP-0006: Validator Economic Payload

目标：

- 定义下游链验证 USDB 经济状态所需 payload。
- 明确哪些字段携带，哪些字段重算。
- 明确 tamper、version mismatch、history unavailable 的错误行为。

需要纳入的候选字段：

- BTC external state。
- pass candidate set。
- raw energy。
- effective energy。
- level。
- real difficulty。
- formula version。
- protocol version。

需要解决：

- candidate set 排序和 tie-break。
- payload 与 historical state ref 的绑定方式。
- reward / price 进入 payload 的阶段边界。

实现影响：

- `doc/usdb-indexer/usdb-indexer-validator-block-body-e2e-design.md`
- `src/btc/usdb-indexer/scripts/regtest_live_ord_validator_*`
- RPC `ConsensusQueryContext`

测试要求：

- tamper winner。
- version mismatch。
- historical context。
- reorg mismatch。

## 14. UIP-0007: Formula Versioning and Activation

目标：

- 定义协议版本、公式版本和查询语义版本。
- 定义激活高度和历史重放规则。
- 定义 state ref / snapshot id 如何承诺公式版本。

需要解决：

- 当前 `USDB_INDEX_FORMULA_VERSION` 是全局常量。
- 历史高度在公式升级后如何按旧公式重放。
- 旧 pass 和新 pass 是否允许跨版本继承。

实现影响：

- `src/btc/usdb-util/src/types.rs`
- `src/btc/usdb-indexer/src/service/rpc.rs`
- `balance-history` state ref identity。

测试要求：

- 不同高度不同公式版本。
- expected formula version mismatch。
- rollback 后版本重放一致。

## 15. UIP-0008: CoinBase Emission and Reward Split

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

建议延后原因：

- 依赖 pass / energy / leader / validator payload 稳定。

## 16. UIP-0009: Price and Real Price Update Rules

目标：

- 定义 `price` 与 `real_price`。
- 定义出块者更新 `real_price` 的挂单证明。
- 定义 `price` 向 `real_price` 收敛。

需要解决：

- 初始值。
- 更新时机。
- 双边挂单证明格式。
- `miner_btc_balance` 统计口径。
- DeFi 合约和 BTC/ETHW 验证边界。

建议延后原因：

- 依赖发行和交易/挂单证明机制。

## 17. UIP-0010: Auxiliary Hashpower Pool

目标：

- 定义辅助算力池提交格式。
- 定义有效算力证明。
- 定义奖励分配和反作弊。

需要解决：

- 最近 2 个 BTC 高度以内的证明如何验证。
- 有效算力大于 BTC 出块难度 75% 的证明格式。
- 多提交者竞争同一奖励如何处理。
- 算力证明和矿工证 owner 如何绑定。

建议延后原因：

- 独立性较强，但实现和验证成本高，应在核心经济模型稳定后推进。

## 18. 推荐实施顺序

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

第四阶段：完整经济系统

1. `UIP-0008`
2. `UIP-0009`
3. `UIP-0010`

## 19. 与当前文档的关系

- `doc/usdb-economic-model-design.md`：目标经济模型总览和讨论稿。
- `doc/usdb-economic-model-issue-tracker.md`：问题、修复状态和下一步工作跟踪。
- `doc/UIP/uip-split-design.md`：正式 UIP 拆分路线图。
- 后续 `doc/UIP/UIP-*.md`：正式协议规范。

原则上，正式 UIP 进入 `Final` 后，其规范性优先级应高于讨论稿。
