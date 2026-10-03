# testnet-v1 接入资料

[返回手册首页](../README.md) · [测试网版本入口](testnet.md)

配置核对日期：**2026-10-03**。本页记录已冻结的 `usdb-testnet-v1` 网络包，首版目标为 `usdb-testnet-v1-r1`。本页尚未记录该版本的公开下载安装、完整同步及多节点入网验收结果；创建 tag 或进入 CI 不代表这些步骤已经完成。

## 版本与验证范围

v1 是可重置开发测试网的新一代，USDB 从 block 0 开始，不延续 v0 的链上余额、合约运行状态或矿工证索引数据。配套程序只执行 MinerPass V2；保留 v0 配置用于历史核对，不提供旧规则执行或 v0 数据原地迁移。

首次部署时，取得网络运维方确认可用的[完整 Release](https://github.com/buckyos/usdb/releases)及其中的 **Release-bound installer** 命令，再按[安装与入网](../node/install.md)操作。没有已验收的安装入口时等待运营方说明，不使用 v0 安装器代替，也不猜测 v1 制品下载 URL。

## 网络资料

| 项目 | 冻结值或获取方式 |
| --- | --- |
| 网络标识 / rules scope | `usdb-testnet-v1` |
| Chain ID / Network ID | `202610030` |
| USDB genesis hash | `0xb0b6ebc9a6c2e051855c2d61dff9a51635ea1db09c898a3c0126e27b9e779314` |
| Bitcoin 数据源 | **Bitcoin mainnet，铸造和转账使用真实 BTC** |
| BTC 索引 origin | `963800`；USDB 链从 0 开始不等于 BTC 索引也从 0 开始 |
| MinerPass 规则 | 从索引 origin 使用 V2；新铭文 JSON 为整数 `v: 2` |
| 稳定状态滞后 | 10 个 BTC 区块 |
| USDB P2P 默认端口 | `31303/TCP` 和 `31303/UDP` |
| Seed | 使用发布包及运营方确认已切换到 v1 的默认列表；旧服务器地址复用不代表旧节点已升级 |
| 公共 RPC、浏览器、矿工证查询服务 | 本页尚无已验收的地址，向网络运维方取得 |

网络身份取自[源码网络包](../../../docker/networks/testnet-v1/README.md)，正式部署时还需与所选发布清单核对。chain ID、network ID、genesis、规则作用域及 registry 均须配套；不要修改某个 ID 来让旧数据通过检查。

## 从 v0 切换

按运营方的重置公告安排停旧节点和新网安装。保留钱包、密钥及需要留存的资料，新网使用自己的数据目录；不要把这次重置当作保留旧链状态的普通升级。Bitcoin 数据和 Balance History 能否复用，由新包按兼容合同检查，不手工复制旧 indexer 或链数据库。

旧节点可能继续运行旧网，但不能加入 v1；同一服务器的旧服务与新服务不能占用同一端口。完成新网同步、入网和 MinerPass V2 核验后再启用挖矿。旧网上的矿工证状态不能作为 v1 资格证明，也不能用旧版 `v: 1` 模板重新铸造。

首次铸造见[钱包指引](../miner-pass/mint.md)，需要长期持币并减少公钥暴露时见[冷钱包与地址轮换](../miner-pass/cold-wallet.md)。网络创建者的配置说明见[进阶建网](../custom-network/README.md)。
