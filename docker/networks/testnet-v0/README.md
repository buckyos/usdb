# USDB testnet-v0 network bundle

`testnet-v0` 是第一个三节点联调网络的可重置 bundle。它把网络共同身份与每台机器的运行参数分开：

- Git 中的 `network.json`、`network.env`、genesis 和 bootstrap config 是所有节点共享的网络输入。
- 未提交的 `node.env` 保存镜像引用、BTC RPC 凭据、节点角色、bootnodes 和 miner 参数。
- `docker/compose.runtime.yml` 是 testnet/mainnet 共用的 image-only 运行基座。

当前状态是 `development-resettable`，不是 public release 或未来 mainnet 参数。发生不兼容重置时必须发布
新的 bundle/chain ID，不能在已有 `testnet-v0` 数据目录上原地替换 genesis。

完整参数所有权、重置边界和上线签字项见
[`doc/publish/usdb-testnet-v0-parameter-freeze.md`](../../../doc/publish/usdb-testnet-v0-parameter-freeze.md)。

## 冻结值

| 项目 | testnet-v0 |
| --- | --- |
| chain ID | `202608250` |
| devp2p network ID | `202608250` |
| P2P | `31303/TCP+UDP` |
| BTC source | `btc-mainnet` |
| BTC index origin | `963800` |
| BTC registry ID | `cc47923f4cdff1875f89771d08e1b89fa22295c92bb816073c3271dc53c54c1c` |
| quote / aux | `0 / 0` |
| genesis SHA-256 | `c40bc1f7e907701d8fe61c25d0386bce86db6768ca1f583614781a732c45ea3e` |

`0x180000 / 0x100000` 仍是当前目标硬件 bring-up 难度，不是最终 PoW calibration 结果。

## 启动前输入

1. 通过 GitHub image workflows 发布 candidate，并取得 digest-only `USDB_SERVICES_IMAGE` 与
   `USDB_CHAIN_IMAGE`。`latest`、`local`、普通 tag 和占位引用不能进入跨仓 release manifest。
2. 在本机 Bitcoin Core 中创建仅供 USDB 使用的 RPC 账户，并允许 Docker bridge 访问；不要对公网发布 `8332`。
3. 准备 `snapshot_963800.db`、manifest 和 detached signature。默认信任 bundle 中的
   `usdb-mainnet-snapshot-v1` public key catalog。
4. 确认本机至少 32 GiB 内存，并为 balance-history 保留 20 GiB cgroup 上限。

初始化私有节点配置：

```bash
cd /home/bucky/work/usdb
docker/scripts/tools/run_testnet_runtime.sh init-env
```

编辑 `docker/networks/testnet-v0/node.env` 后执行：

```bash
docker/scripts/tools/run_testnet_runtime.sh validate-node
docker/scripts/tools/run_testnet_runtime.sh up
docker/scripts/tools/run_testnet_runtime.sh ps
```

`up` 会进一步要求 snapshot 三件套真实存在，并在启动前执行 `docker compose config --quiet`。
共享 runtime 默认把每个容器的 JSON log 限制为 `5 x 100 MiB`，并给长服务 2 分钟优雅停止时间；
这些是节点运行参数，不进入链共识身份。

## 三节点顺序

1. 第一台以 `USDB_NODE_ROLE=bootnode` 启动，HTTP RPC 只通过 SSH tunnel 或本机访问。
2. 通过 `admin_nodeInfo` 读取第一台 enode，把它写入另外两台 `USDB_BOOTNODES`。
3. 第二、三台先以 `full` 加入，确认 genesis hash、chain ID、peer 和同步高度一致。
4. BTC-side active standard pass 就绪后，再把选定节点改为 `miner`，同时配置
   `USDB_MINER_ADDRESS` 和 `USDB_PASS_ID`。

SourceDAO full bootstrap config 已随 bundle 冻结，但 bootstrap private key 不进入 Compose 或 Git。
首次启动应在独立受控步骤中执行 `usdb_bootstrap_full.ts`，并在区块 `8192` fee gate 前完成
`Dividend.finalizeBootstrap()`。runtime Compose 不自动消费管理员密钥。

## 尚未冻结

- 两个发布镜像的 digest 与最终三仓 release manifest；candidate workflow 已具备，但尚待实际 artifact。
- snapshot artifact 自身的 hash/signature；当前只冻结 signer public key。
- 三台机器的 bootnode enode、外部 IP 和 miner pass。
- 正式 PoW calibration 报告。
- SourceDAO bootstrap 执行记录和完成 checkpoint。

这些内容完成后才能把 bundle 状态提升为 release candidate。
