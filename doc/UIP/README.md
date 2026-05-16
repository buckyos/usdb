# USDB Improvement Proposals

`doc/UIP/` 用于存放 USDB 的正式协议改进提案。

UIP 参考 BTC BIP 和 Ethereum EIP 的组织方式，但应保持 USDB 自身的协议边界：

- BTC 侧矿工证铭文与索引规则。
- USDB 经济公式、版本与激活高度。
- USDB 经济状态视图、ETHW 链上 payload 与下游链验证接口。
- 发行、价格、协作矿工、辅助算力池等经济组件。

当前目录中的文档分两类：

| 类型 | 说明 |
| --- | --- |
| 拆分/规划文档 | 用于规划 UIP 边界，不直接作为最终协议。 |
| 正式 UIP | 后续使用 `UIP-0001-*.md` 形式落地，进入 Draft/Review/Final 流程。 |

## 当前文档

- [UIP-0000-uip-process.md](./UIP-0000-uip-process.md)：UIP 流程、治理、网络化激活规则与模板。
- [UIP-0001-miner-pass-inscription.md](./UIP-0001-miner-pass-inscription.md)：矿工证铭文 v1 schema、standard/collab pass 字段和兼容策略。
- [UIP-0002-pass-state-machine.md](./UIP-0002-pass-state-machine.md)：矿工证状态机、事件排序、`prev` 严格校验与 burn 终态。
- [UIP-0003-pass-energy-formula.md](./UIP-0003-pass-energy-formula.md)：矿工证 raw energy 公式、余额惩罚、继承折损与终态能量。
- [UIP-0004-collab-leader-effective-energy.md](./UIP-0004-collab-leader-effective-energy.md)：协作矿工证 Leader 解析、collab contribution 与 effective energy。
- [UIP-0005-level-and-real-difficulty.md](./UIP-0005-level-and-real-difficulty.md)：基于 effective energy 的 level 阈值表和 real difficulty 折算规则。
- [UIP-0006-usdb-economic-state-view.md](./UIP-0006-usdb-economic-state-view.md)：USDB indexer 提供的经济状态视图、审计字段和历史重放错误语义。
- [UIP-0007-ethw-consensus-profile-selector.md](./UIP-0007-ethw-consensus-profile-selector.md)：ETHW `header.Extra` 中的最小 USDB consensus profile selector。
- [UIP-0008-protocol-versioning-and-activation-matrix.md](./UIP-0008-protocol-versioning-and-activation-matrix.md)：协议版本族、激活矩阵、历史重放和 state commit 版本绑定。
- [UIP-0008-activation-registry-implementation-notes.md](./UIP-0008-activation-registry-implementation-notes.md)：activation registry 在多服务实现中的集中定义、分散校验设计备忘。
- [UIP-0009-ethw-chain-config-and-usdb-bootstrap.md](./UIP-0009-ethw-chain-config-and-usdb-bootstrap.md)：USDB ETHW 链 chain config、genesis、PoW bootstrap 和 USDB 共识版本字段。
- [UIP-0010-source-dao-dividend-bootstrap.md](./UIP-0010-source-dao-dividend-bootstrap.md)：SourceDAO / Dividend system contract 冷启动、genesis predeploy、bootstrap 交易和 fee split activation 边界。
- [UIP-0011-coinbase-emission-and-reward-split.md](./UIP-0011-coinbase-emission-and-reward-split.md)：CoinBase 释放公式、手续费分账、reward recipient 校验和 reward policy 版本边界。
- [UIP-0012-collaboration-efficiency-coefficient.md](./UIP-0012-collaboration-efficiency-coefficient.md)：协作效率系数 `K`、rolling window、warmup 和 reserved system storage 状态。
- [UIP-0013-price-and-real-price-update-rules.md](./UIP-0013-price-and-real-price-update-rules.md)：BTC 算法价格状态、固定价格启动策略和动态 price source 升级边界。
- [UIP-0014-leader-quote-activity-and-candidate-energy.md](./UIP-0014-leader-quote-activity-and-candidate-energy.md)：Leader 主动报价活跃窗口、candidate energy 和 candidate level 策略。
- [UIP-0015-auxiliary-hashpower-pool.md](./UIP-0015-auxiliary-hashpower-pool.md)：辅助算力池激活边界、BTC 算力证明纲要、pass 绑定和 reward 分配待审计问题。
- [uip-split-design.md](./uip-split-design.md)：经济模型拆分与标准化顺序。

## 后续建议

后续正式 UIP 建议采用如下文件名：

- `UIP-0000-uip-process.md`
- `UIP-0001-miner-pass-inscription.md`
- `UIP-0002-pass-state-machine.md`
- `UIP-0003-pass-energy-formula.md`
- `UIP-0004-collab-leader-effective-energy.md`
- `UIP-0005-level-and-real-difficulty.md`
- `UIP-0006-usdb-economic-state-view.md`
- `UIP-0007-ethw-consensus-profile-selector.md`
- `UIP-0008-protocol-versioning-and-activation-matrix.md`
- `UIP-0009-ethw-chain-config-and-usdb-bootstrap.md`
- `UIP-0010-source-dao-dividend-bootstrap.md`
- `UIP-0011-coinbase-emission-and-reward-split.md`
- `UIP-0012-collaboration-efficiency-coefficient.md`
- `UIP-0013-price-and-real-price-update-rules.md`
- `UIP-0014-leader-quote-activity-and-candidate-energy.md`
- `UIP-0015-auxiliary-hashpower-pool.md`

正式 UIP 的头部字段建议在 `UIP-0000` 或流程文档中统一定义。
