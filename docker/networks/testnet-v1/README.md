# USDB testnet-v1

开发测试网重置代际，chain ID / network ID 为 `202610030`。
BTC 主网 origin `963800`；独立 rules scope `usdb-testnet-v1`，从 origin 使用 MinerPass V2。
USDB 链从 block 0 开始；不导入 v0 区块、余额、运行期 SourceDAO 状态或 indexer 数据。
SourceDAO 的冻结初始分配、委员会、bootstrap admin 与 PoW 参数沿用 v0。

`network.json` 是固定身份输入；`release-bootstrap.json` 复用既有 Bitcoin 主网签名 UTXO 发布材料。
Candidate/Publish 根据 `usdb-testnet-v1-rN` 选择本目录并生成 AssumeUTXO 节点配置。
Bitcoin 基础数据可按兼容合同复用；indexer/链/控制面使用新目录。请勿对 v0 数据目录直接执行 init。

bootnodes 沿用原部署端点，须在运营方完成 v1 重置后验收连通性；文件存在不表示旧节点已经升级。
生成和发布步骤见 `doc/publish/usdb-testnet-v1-network-bundle.md`。
