# USDB Improvement Proposals

`doc/UIP/` 用于存放 USDB 的正式协议改进提案。

UIP 参考 BTC BIP 和 Ethereum EIP 的组织方式，但应保持 USDB 自身的协议边界：

- BTC 侧矿工证铭文与索引规则。
- USDB 经济公式、版本与激活高度。
- validator payload 与下游链验证接口。
- 发行、价格、协作矿工、辅助算力池等经济组件。

当前目录中的文档分两类：

| 类型 | 说明 |
| --- | --- |
| 拆分/规划文档 | 用于规划 UIP 边界，不直接作为最终协议。 |
| 正式 UIP | 后续使用 `UIP-0001-*.md` 形式落地，进入 Draft/Review/Final 流程。 |

## 当前文档

- [UIP-0000-uip-process.md](./UIP-0000-uip-process.md)：UIP 流程、治理、网络化激活规则与模板。
- [uip-split-design.md](./uip-split-design.md)：经济模型拆分与标准化顺序。

## 后续建议

后续正式 UIP 建议采用如下文件名：

- `UIP-0000-uip-process.md`
- `UIP-0001-miner-pass-inscription.md`
- `UIP-0002-pass-state-machine.md`
- `UIP-0003-pass-energy-formula.md`
- `UIP-0004-collab-leader-effective-energy.md`
- `UIP-0005-validator-economic-payload.md`

正式 UIP 的头部字段建议在 `UIP-0000` 或流程文档中统一定义。
