# USDB testnet-v0 network bundle

第一个三节点测试网使用：

- [testnet-v0 参数冻结清单](./usdb-testnet-v0-parameter-freeze.md)
- [GitHub CI 镜像与跨仓 Release 发布](./github-ci-image-and-release-publishing.md)
- [共享 runtime Compose](../../docker/compose.runtime.yml)
- [testnet-v0 bundle](../../docker/networks/testnet-v0/README.md)
- [启动工具](../../docker/scripts/tools/run_testnet_runtime.sh)

bundle 固定网络共同身份，`node.env` 只保存每台机器的镜像、BTC RPC、bootnode、端口和 miner 参数。
不要把 `node.env`、RPC password、bootstrap private key 或 miner key 提交到 Git。

当前 bundle 是可重置开发测试网。发布三台机器前至少完成：

1. 为三个工程构建并记录不可变 revision/image digest。
2. 完成高度 `963800` signed balance-history snapshot，并从独立空目录安装复检。
3. 在三台机器分别运行 bundle validator 和 Compose render check。
4. 先启动 bootnode，再固定 enode 并启动两个 joiner。
5. 完成 SourceDAO full bootstrap 与只读复检，再越过 fee gate。
6. 记录 genesis SHA-256、chain ID、network ID、BTC registry ID、snapshot ID 和首个共同 checkpoint。

任何 genesis、chain ID、activation、SourceDAO predeploy 或初始难度变更都意味着新网络 bundle；不得把
旧数据目录当作兼容升级继续使用。
