# USDB testnet-v0 network bundle

`testnet-v0` 是第一个三节点联调网络的可重置 bundle。它把网络共同身份与每台机器的运行参数分开：

- Git 中的 `network.json`、`network.env`、genesis 和 bootstrap config 是所有节点共享的网络输入。
- 未提交的 `node.env` 保存镜像引用、BTC RPC、snapshot 模式、节点角色、bootnodes 和 miner 参数。
- `docker/compose.bitcoin.yml` 是独立 Bitcoin full-node project；`docker/compose.runtime.yml` 是 USDB image-only 运行基座。

当前状态是 `development-resettable`，不是 public release 或未来 mainnet 参数。发生不兼容重置时必须发布
新的 bundle/chain ID，不能在已有 `testnet-v0` 数据目录上原地替换 genesis。

完整参数所有权、重置边界和上线签字项见
[`doc/publish/usdb-testnet-v0-parameter-freeze.md`](../../../doc/publish/usdb-testnet-v0-parameter-freeze.md)。

## 冻结值

| 项目 | testnet-v0 |
| --- | --- |
| chain ID | `202608250` |
| deployment tier | `testnet` |
| devp2p network ID | `202608250` |
| P2P | `31303/TCP+UDP` |
| BTC source | `btc-mainnet` |
| BTC index origin | `963800` |
| BTC registry ID | `a6350cd6a68755ea64edf537f35c1eca4421a970e2ecfd67aaa29075aae57224` |
| quote / aux | `0 / 0` |
| bootstrap admin | `0x0b5223FD31cDc1536f31b3627e6D7025b52310c9` |
| genesis SHA-256 | `da5d9062d26a75c7ec4d6f3f2b567ffd627c53b5482f1bc702ce37026b06e2e5` |
| genesis block hash | `0x12a1baed070d1521d791b73956a8b5cf1613fc9504636f215390c1f839992a23` |

`0x180000 / 0x100000` 仍是当前目标硬件 bring-up 难度，不是最终 PoW calibration 结果。

`963800` 只属于 testnet-v0 bundle。其他测试网或未来 mainnet bundle 可以冻结不同的
`index_origin_height`。Snapshot 不属于网络身份：全新 indexer 可使用任意通过签名和身份校验、且高度
不高于本网络 origin 的 snapshot；官方推荐 artifact 使用与 origin 相同的高度。

本次 bootstrap admin 隔离已经改变 genesis。任何曾用旧 block hash
`0xac89ddec...70e560` 初始化的 USDB-chain datadir 都必须丢弃并用本 bundle 重新 `geth init`；
仅因此变化不要求重建 Bitcoin Core 或 BTC-side index 数据。

## 启动前输入

1. 通过 GitHub image workflows 发布 candidate，并取得 digest-only `USDB_SERVICES_IMAGE`、
   `USDB_CHAIN_IMAGE` 与 `USDB_BITCOIN_IMAGE`。`latest`、`local`、普通 tag 和占位引用不能进入跨仓 release manifest。
2. 准备独立 Bitcoin 数据目录和 rpcauth；release Compose 默认把 `8333/TCP` 绑定到 loopback，
   可显式改为公网 Bitcoin P2P，但始终不发布 `8332`。
3. 默认使用 `SNAPSHOT_MODE=none`，balance-history 从 BTC 创世全量同步；signed snapshot 是以后可选的节点加速路径。
4. 确认本机至少 32 GiB 内存；共机模板为 Bitcoin `5g`、balance-history `12g`，全部服务 hard limit 合计 `27g`。

发布节点优先从 GitHub Release node kit 安装，不再 clone 仓库或手工填写 image digest：

```bash
bash <(curl -fsSL \
  "https://github.com/buckyos/usdb/releases/download/usdb-testnet-v0-r1/install-usdb-testnet-v0-r1.sh")
export PATH="${HOME}/.local/bin:${PATH}"
usdb-node prepare-host
usdb-node setup
usdb-node controller install
usdb-node doctor
usdb-node resume
```

`usdb-node setup` 从 release manifest 写入三张 image digest，在本机生成 Bitcoin RPC secret，并选择
external firewall 或 managed UFW profile；选择 snapshot 时只冻结批准记录，不在 setup 前台下载或生成 live
RocksDB。`controller install` 安装并启用 bundle-scoped systemd unit；`resume` 提交该 controller，依次等待
Bitcoin、balance-history 和 indexer readiness，最后启动 USDB chain。
交互式启动会附加 snapshot、Bitcoin、balance-history、usdb-indexer 和 USDB chain 的固定进度面板；
Snapshot 行会显示 artifact 等待、SQLite 导入阶段和最终 live DB marker；`usdb-node status --watch` 可在
独立终端持续观察。Ctrl+C 或 SSH 断开只退出面板，systemd controller 继续推进；阶段 heartbeat 写入
`usdb-node controller logs --follow`。详细契约见
[`doc/publish/usdb-release-node-kit-and-deployment.md`](../../../doc/publish/usdb-release-node-kit-and-deployment.md)。
共享 runtime 默认把每个容器的 JSON log 限制为 `5 x 100 MiB`，并给长服务 2 分钟优雅停止时间；
这些是节点运行参数，不进入链共识身份。

首节点从零部署的完整命令和验收项见
[`doc/publish/usdb-testnet-v0-first-node-operations.md`](../../../doc/publish/usdb-testnet-v0-first-node-operations.md)。

## 三节点顺序

1. 第一台以 `USDB_NODE_ROLE=bootnode` 启动，HTTP RPC 只通过 SSH tunnel 或本机访问。
2. 通过 `admin_nodeInfo` 读取第一台 enode，把它写入另外两台 `USDB_BOOTNODES`。
3. 第二、三台先以 `full` 加入，确认 genesis hash、chain ID、peer 和同步高度一致。
4. BTC-side active standard pass 就绪后，再把选定节点改为 `miner`，配置
   `USDB_MINER_ADDRESS`。indexer 会在冻结 external state 下按该 `usdb_main` 原子选择具体 pass。

SourceDAO full bootstrap config 已随 bundle 冻结，但 bootstrap private key 不进入 Compose 或 Git。
首次启动应在独立受控步骤中执行 `usdb_bootstrap_full.ts`，并在区块 `8192` fee gate 前完成
`Dividend.finalizeBootstrap()`。runtime Compose 不自动消费管理员密钥。

仓库内开发 fixture 使用的 `0xabCd35AfbB4561213fEAfF01B5F91e18F8Df7c37` 已知对应公开私钥，
只允许 local/world-sim。bundle validator 会拒绝 testnet/mainnet 使用该地址；未来 mainnet 还必须
生成与本 testnet 地址不同的 signer。Git 和 bundle 只记录公开地址。

## 尚未冻结

- 三个发布镜像的 digest 与最终三仓 release manifest；candidate workflow 已具备，但尚待实际 artifact。
- release-approved snapshot record 已冻结为 BTC height `963800` 的 content-addressed artifact；candidate
  和 publish 必须确认公开 record、对象长度和 DB byte-range 可用。节点仍可在 `setup` 中选择 full sync，
  选择 snapshot 时再逐文件完成 SHA-256 与签名校验。
- 三台机器的 bootnode enode、外部 IP 和 miner pass。
- 正式 PoW calibration 报告。
- SourceDAO bootstrap 执行记录和完成 checkpoint。

这些内容完成后才能把 bundle 状态提升为 release candidate。
