# USDB testnet-v0 network bundle

第一个三节点测试网使用：

- [testnet-v0 参数冻结清单](./usdb-testnet-v0-parameter-freeze.md)
- [GitHub CI 镜像与跨仓 Release 发布](./github-ci-image-and-release-publishing.md)
- [共享 runtime Compose](../../docker/compose.runtime.yml)
- [独立 Bitcoin Compose](../../docker/compose.bitcoin.yml)
- [testnet-v0 bundle](../../docker/networks/testnet-v0/README.md)
- [Bitcoin 启动工具](../../docker/scripts/tools/run_testnet_bitcoin.sh)
- [USDB runtime 启动工具](../../docker/scripts/tools/run_testnet_runtime.sh)
- [首节点发布与部署操作手册](./usdb-testnet-v0-first-node-operations.md)

bundle 固定网络共同身份，`node.env` 只保存每台机器的镜像、BTC RPC、bootnode、端口和 miner 参数。
不要把 `node.env`、RPC password、bootstrap private key 或 miner key 提交到 Git。

当前 bundle 是可重置开发测试网。发布三台机器前至少完成：

1. 为三个工程构建并记录不可变 revision，以及 services/chain/Bitcoin Core 三个 image digest。
2. 在三台机器分别运行 bundle validator 和 Compose render check。
3. 每台机器先完成 Bitcoin full sync/txindex readiness；没有 snapshot 时，balance-history 从创世全量同步。
4. 启动 bootnode，固定 enode 后启动两个 joiner。
5. 完成 SourceDAO full bootstrap 与只读复检，再越过 fee gate。
6. 记录 genesis SHA-256、chain ID、network ID、BTC registry ID、数据层 state-ref 和首个共同 checkpoint。

任何 genesis、chain ID、activation、SourceDAO predeploy 或初始难度变更都意味着新网络 bundle；不得把
旧数据目录当作兼容升级继续使用。
