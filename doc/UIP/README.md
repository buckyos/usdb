# USDB Improvement Proposals

`doc/UIP/` 用于存放 USDB 的正式协议改进提案。

UIP 参考 BTC BIP 和 Ethereum EIP 的组织方式，但应保持 USDB 自身的协议边界：

- BTC 侧矿工证铭文与索引规则。
- USDB 经济公式、版本与激活高度。
- BTC-side USDB 经济状态视图、USDB chain payload 与链上验证接口。
- 发行、价格、协作矿工、辅助算力池等经济组件。

当前目录中的文档分两类：

| 类型 | 说明 |
| --- | --- |
| 拆分/规划文档 | 用于规划 UIP 边界，不直接作为最终协议。 |
| 正式 UIP | 后续使用 `UIP-0001-*.md` 形式落地，进入 Draft/Review/Final 流程。 |

## 命名与系统边界

`USDB` 是项目和协议族名称。规范正文描述具体对象时不得仅用裸 `USDB` 推断其所属系统，必须使用下列限定名称：

| 规范名称 | 含义 |
| --- | --- |
| `USDB protocol` | 横跨 BTC 铭文、BTC-side 派生状态和 USDB chain 共识的完整协议族。 |
| `USDB chain` | 从 ETHW/geth 代码基础演进而来的独立 EVM-compatible PoW 链；其 chain id、genesis、block、validator、reward、difficulty 和 chain config 均属于 USDB chain。 |
| `BTC-side USDB services` | `balance-history`、`usdb-indexer` 及其状态视图；它们索引 BTC，但不是 USDB chain 执行层。 |
| `USDB native currency` / `USDB atom` | USDB chain 的原生资产及其最小单位；涉及余额、发行或手续费时不得仅写 `USDB`。 |
| `USDB-chain account address` | USDB chain 上的 EVM account address；矿工证字段 `usdb_main` 属于此类。 |
| `BTC owner address` / `owner_script_hash` | BTC UTXO owner 的地址表示或 script identity；不得称为 USDB address/script hash。 |

`ETHW` 只用于以下非 USDB 协议身份语义：

- 说明 USDB chain 的上游历史或与 legacy ETHW 网络比较。
- 引用继承代码中的既有专名，例如 Ethash、`EthPoWForkBlock`、`ETHWStartDifficulty`。
- 引用尚未改名且确实表示底层执行进程的运维标识，例如 `ethw-node` 或 `ETHW_COMMAND`；这类名称不得扩散为协议字段、chain config 字段或 UIP 术语。

因此，新定义的激活锚点写作 `usdb_block`，新链网络 ID 使用 `usdb-*`。正式 UIP 中的 validator、miner、header、payload、reward 和 difficulty 默认属于 USDB chain，不再称为 ETHW-side 规则。

RPC 和运行参数同样必须显式区分边界：BTC-side 状态服务使用
`usdb-indexer`、`USDB_INDEXER_RPC_*` 和 `--*.usdb-indexer.*`；USDB chain 节点使用
`usdb_chain_url` 或 `USDB_CHAIN_RPC_URL`，其 EVM JSON-RPC method 仍保留标准 `eth_*`
名称。新接口禁止使用无法判断指向链节点还是 indexer 的 `USDB_RPC_*`。

## 当前文档

- [UIP-0000-uip-process.md](./UIP-0000-uip-process.md)：UIP 流程、治理、网络化激活规则与模板。
- [UIP-0001-miner-pass-inscription.md](./UIP-0001-miner-pass-inscription.md)：矿工证铭文 v1 schema、standard/collab pass 字段和校验规则。
- [UIP-0002-pass-state-machine.md](./UIP-0002-pass-state-machine.md)：矿工证状态机、事件排序、`prev` 严格校验与 burn 终态。
- [UIP-0003-pass-energy-formula.md](./UIP-0003-pass-energy-formula.md)：矿工证 raw energy 公式、余额惩罚、继承折损与终态能量。
- [UIP-0004-collab-leader-effective-energy.md](./UIP-0004-collab-leader-effective-energy.md)：协作矿工证 Leader 解析、collab contribution 与 effective energy。
- [UIP-0005-level-and-real-difficulty.md](./UIP-0005-level-and-real-difficulty.md)：基于 effective energy 的 level 阈值表和 real difficulty 折算规则。
- [UIP-0006-usdb-economic-state-view.md](./UIP-0006-usdb-economic-state-view.md)：USDB indexer 提供的经济状态视图、审计字段和历史重放错误语义。
- [UIP-0007-usdb-consensus-profile-selector.md](./UIP-0007-usdb-consensus-profile-selector.md)：USDB chain `header.Extra` 中的最小 consensus profile selector。
- [UIP-0008-protocol-versioning-and-activation-matrix.md](./UIP-0008-protocol-versioning-and-activation-matrix.md)：协议版本族、BTC registry revisions、USDB activation schedule、历史重放和 state commit 版本绑定。
- [UIP-0008-activation-registry-implementation-notes.md](./UIP-0008-activation-registry-implementation-notes.md)：BTC registry 与 USDB activation checkpoints 在多服务实现中的所有权和校验设计备忘。
- [UIP-0009-usdb-chain-config-and-bootstrap.md](./UIP-0009-usdb-chain-config-and-bootstrap.md)：USDB chain config、genesis、PoW bootstrap 和共识版本字段。
- [UIP-0010-source-dao-dividend-bootstrap.md](./UIP-0010-source-dao-dividend-bootstrap.md)：SourceDAO / Dividend system contract 冷启动、genesis predeploy、bootstrap 交易和 fee split activation 边界。
- [UIP-0011-coinbase-emission-and-reward-split.md](./UIP-0011-coinbase-emission-and-reward-split.md)：CoinBase 释放公式、手续费分账、reward recipient 校验和 reward policy 版本边界。
- [UIP-0012-collaboration-efficiency-coefficient.md](./UIP-0012-collaboration-efficiency-coefficient.md)：协作效率系数 `K`、rolling window、warmup 和 reserved system storage 状态。
- [UIP-0013-price-and-real-price-update-rules.md](./UIP-0013-price-and-real-price-update-rules.md)：BTC 算法价格状态、固定价格启动策略和动态 price source 升级边界。
- [UIP-0014-leader-quote-activity-and-candidate-energy.md](./UIP-0014-leader-quote-activity-and-candidate-energy.md)：Leader 主动报价活跃窗口、candidate energy 和 candidate level 策略。
- [UIP-0015-auxiliary-hashpower-pool.md](./UIP-0015-auxiliary-hashpower-pool.md)：辅助算力池激活边界、BTC 算力证明纲要、pass 绑定和 reward 分配待审计问题。
- [uip-split-design.md](./uip-split-design.md)：经济模型拆分与标准化顺序。

## 跨 UIP 术语索引

本节只提供导航，不复制规范定义。术语的规范含义以“定义 UIP”中的术语和规则为准；其他 UIP 使用该术语时必须引用定义 UIP，不得赋予不同含义。

| 术语族 | 定义 UIP | 规范边界 |
| --- | --- | --- |
| `pass`、`pass_id`、`pass_kind`、`owner_script_hash`、`owner_btc_addr` | [UIP-0001](./UIP-0001-miner-pass-inscription.md) | BTC 铭文身份、canonical pass id、pass 类型和 owner 表示。 |
| `Active`、`Dormant`、`Consumed`、`Burned`、`Invalid`、`prev`、mint / remint | [UIP-0002](./UIP-0002-pass-state-machine.md) | pass 生命周期、事件顺序和继承消费状态。 |
| `raw_energy`、`settled_raw_energy`、`projected_raw_energy`、`inheritable_energy` | [UIP-0003](./UIP-0003-pass-energy-formula.md) | BTC-side 可持久化、可继承能量。 |
| `leader_ref`、`resolved_leader`、`leader_eligible`、`collab_contribution`、`effective_energy` | [UIP-0004](./UIP-0004-collab-leader-effective-energy.md) | BTC-side Leader 解析、USDB-chain eligibility 边界和 nominal effective energy。 |
| `level`、`difficulty_factor_bps`、`real_difficulty` | [UIP-0005](./UIP-0005-level-and-real-difficulty.md) | indexer nominal level/factor 和 USDB-chain difficulty 折算公式。 |
| `query_context`、`expected_state`、`external_state`、`candidate_pass`、`candidate_set_view`、`top_ranked_candidate`、`collab_breakdown` | [UIP-0006](./UIP-0006-usdb-economic-state-view.md) | BTC-side USDB 历史查询、经济审计集合和确定性排序。 |
| `ProfileSelectorPayload`、`selected_pass`、USDB miner、USDB validator | [UIP-0007](./UIP-0007-usdb-consensus-profile-selector.md) | USDB block 选择的 pass、链上 selector payload 和验证角色。 |
| `btc_activation_record`、`btc_registry_revision`、`activation_registry_id`、`active_version_set`、`usdb_activation_checkpoint`、`usdb_activation_schedule`、`resolved_usdb_versions` | [UIP-0008](./UIP-0008-protocol-versioning-and-activation-matrix.md) | BTC 单版本族记录、完整 registry 快照，以及 USDB chain 的完整激活检查点和按高度 lookup。 |
| `candidate_energy`、`candidate_level`、`leader_quote_active` | [UIP-0014](./UIP-0014-leader-quote-activity-and-candidate-energy.md) | USDB-chain policy 应用于 selected pass 的实际候选能量和等级。 |

特别地，`candidate_set_view` 的排序首项只是 `top_ranked_candidate`，不自动等于某个 USDB block 的 `selected_pass`，也不表示已经赢得 PoW 出块竞争。正式 UIP 不使用裸 `winner` 指代 pass；需要分别写明审计排序首项、区块声明的 pass 或已经通过 PoW 验证的区块。

`validator test envelope` 是 regtest/E2E 的链外测试容器，不是正式链上 payload。正式 USDB header payload 必须写作 UIP-0007 `ProfileSelectorPayload`。

## 后续建议

后续正式 UIP 建议采用如下文件名：

- `UIP-0000-uip-process.md`
- `UIP-0001-miner-pass-inscription.md`
- `UIP-0002-pass-state-machine.md`
- `UIP-0003-pass-energy-formula.md`
- `UIP-0004-collab-leader-effective-energy.md`
- `UIP-0005-level-and-real-difficulty.md`
- `UIP-0006-usdb-economic-state-view.md`
- `UIP-0007-usdb-consensus-profile-selector.md`
- `UIP-0008-protocol-versioning-and-activation-matrix.md`
- `UIP-0009-usdb-chain-config-and-bootstrap.md`
- `UIP-0010-source-dao-dividend-bootstrap.md`
- `UIP-0011-coinbase-emission-and-reward-split.md`
- `UIP-0012-collaboration-efficiency-coefficient.md`
- `UIP-0013-price-and-real-price-update-rules.md`
- `UIP-0014-leader-quote-activity-and-candidate-energy.md`
- `UIP-0015-auxiliary-hashpower-pool.md`

正式 UIP 的头部字段建议在 `UIP-0000` 或流程文档中统一定义。
