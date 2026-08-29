# USDB 发布与上线文档

## 1. 目录职责

本目录集中维护 USDB 跨工程的打包、发布、部署和上线编排。这里回答：

- 哪些源代码 revision 和协议参数组成一次 release；
- 需要构建、签名和发布哪些 artifact；
- 各组件按什么顺序 bootstrap、安装、启动和验收；
- public release 前有哪些阻断门禁、人工审批和回滚条件。

组件内部机制仍由各自文档维护。例如 snapshot 文件格式、签名算法和安装校验属于
`doc/balance-history`；本目录只规定它们如何进入 USDB release bundle 和上线流程。

## 2. 当前手册

- [USDB Release 总体流程](./usdb-release-process.md)
- [USDB 节点主机软件基线与准备工具](./usdb-node-host-prerequisites.md)
- [USDB testnet-v0 参数冻结清单](./usdb-testnet-v0-parameter-freeze.md)
- [GitHub CI 镜像与跨仓 Release 发布](./github-ci-image-and-release-publishing.md)
- [Bitcoin Core Release Image 与同步操作](./bitcoin-core-release-and-sync-operations.md)
- [USDB testnet-v0 首节点发布与部署操作](./usdb-testnet-v0-first-node-operations.md)
- [Balance-History 发布与 Snapshot 分发](./balance-history-release-and-snapshot-distribution.md)
- [Balance-history Snapshot 与 Indexer Checkpoint 兼容规则](./balance-history-indexer-checkpoint-compatibility.md)
- [USDB testnet-v0 Network Bundle](./usdb-testnet-v0-network-bundle.md)

## 3. 关联文档

- [Balance-History Snapshot 签名与信任说明](../balance-history/balance-history-snapshot-signing.md)
- [Balance-History 主网 Snapshot 操作指南](../balance-history/balance-history-mainnet-exact-height-snapshot-operations.md)
- [USDB Docker Deployment Plan](../usdb-docker-deployment-plan.md)
- [World-Sim Release Image Plan](../world-sim-release-image-plan.md)
- [UIP README](../UIP/README.md)

配套仓库还包括：

- `go-ethereum/docs/usdb`：USDB chain node、genesis、activation 和共识 E2E；
- `SourceDAO`：合约 artifact、bootstrap 参数和链上初始化验收。

这些文档仍是组件事实来源。后续可以逐步迁移发布编排内容，但不应把协议规范或组件实现文档
复制到本目录。

## 4. 文档状态约定

发布手册中的事项应标记为：

- `implemented`：代码和本地自动化已存在并验证；
- `manual`：已有确定流程，但仍需人工执行或审批；
- `planned`：设计方向明确，尚无可执行自动化；
- `blocked`：缺少冻结参数、外部依赖或安全决策，不能用于 public release。

开发期 smoke 成功不等同于 public release ready。手册必须分别记录本地验证、目标硬件验证、
跨进程 E2E、发布 artifact 验证和实际上线结果。
