# USDB testnet-v0 network bundle

`testnet-v0` 是第一个三节点联调网络的可重置 bundle。它把网络共同身份与每台机器的运行参数分开：

- Git 中的 `network.json`、`network.env`、genesis 和 bootstrap config 是所有节点共享的网络输入。
- `bootnodes.json` 保存随发布包交付的默认入网地址，独立于链身份。
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

## 默认入网节点

`bootnodes.json` 使用 `usdb-bootnodes:v1`，以 `network_bundle_id` 绑定本测试网，
`bootnodes` 是完整 enode 字符串数组。当前包含 `usdb-testnet.tbudr.top:31303` 的入口。
后续稳定节点直接追加到数组，然后按正常流程发布新 node-kit；完整地址包含各节点自己的公钥。
列表最多 64 条，读取时规范化并去重，支持 IPv4、IPv6、域名及 `?discport=`，校验阶段不查询 DNS。

首次 `setup` 显示列表，回车或输入 `default` 使用它；也可输入自定义逗号分隔列表，或输入 `none`
明确不配置 Seed。`configure` 未传 `--bootnodes` 时采用默认列表；`--bootnodes ''` 明确保存空列表。
这些规则同时适用于 full 和 bootnode 角色。创建一个新网络的真正首节点时显式选择 `none`，
之后仍需独立执行首节点挖矿授权。

默认值只在创建配置时写入 `node.env` 的 `USDB_BOOTNODES`。已有节点再次 setup、启动或升级均保留
自己的列表，包括手动删除后的空列表；需追加入口时使用 `usdb-node peers add`。新版列表只自动用于
新版首次安装，不会远程推送或自动合并到在线节点。旧 bundle 缺少该文件时按无默认入口处理。

文件经公共输入白名单复制到 AssumeUTXO 候选包和最终 node-kit，由安装归档的 SHA-256 校验覆盖。
不要将它加入 `network.json` 的 artifact 哈希或矿工身份绑定：入口变化不应改变 genesis、
`network_json_sha256`、数据兼容 ID 或已有首节点/矿工记录。

验证新增入口时分别检查公网 TCP/UDP 31303、公告地址、实际 peer 与链同步。
当前 Geth 首次解析 Seed 域名失败仍可能阻断启动；本批仅交付默认列表，不包含该重试修复或 DNS 节点目录。

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
usdb-node doctor
usdb-node up
```

`usdb-node setup` 从 release manifest 写入三张 image digest，在本机生成 Bitcoin RPC secret，选择
external firewall 或 managed UFW profile；选择 snapshot 时只冻结批准记录，不在 setup 前台下载或生成 live
RocksDB，并默认安装、enable bundle-scoped systemd unit。`up` 提交该 controller：Bitcoin tip 达到
snapshot/origin stable anchor 加 registry 冻结的 `stable_lag_blocks=10` 后启动 balance-history，使用 snapshot
时另在 anchor 高度校验 active-chain block hash；balance-history 提交 origin 后启动 usdb-indexer；
三者继续流水线追块，全部最终 readiness 通过后才启动 USDB chain。
交互式启动会附加 snapshot、Bitcoin、balance-history、usdb-indexer 和 USDB chain 的固定进度面板；
Snapshot 行会显示 artifact 等待、SQLite 导入阶段和最终 live DB marker；`usdb-node status --watch` 可在
独立终端持续观察。Ctrl+C 或 SSH 断开只退出面板，systemd controller 继续推进；阶段 heartbeat 写入
`usdb-node controller logs --follow`。只有前台调试、CI 或非 systemd 环境才使用 `setup --no-controller`；详细契约见
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

## 可选快照

`snapshots/balance-history-snapshot-release-record.json` 固定本次拆分快照的下载清单：

- BTC height：`963800`；snapshot release：`balance-history-bitcoin-h963800-59e54b88ef118294`。
- Record SHA-256：`56ad21c69c4b02a9bf398f63ab609de91ea35935fd1a7d3af151bd2098678789`。
- Core DB：约 `34.46 GiB`；可选 script registry DB：约 `183.19 GiB`。

Release manifest 从此文件派生公开 record URL、组件 ID、文件大小和可信 catalog hash，node kit
携带同一文件；安装脚本无需另写 DB URL。制作新 release 时必须包含该文件所在的 USDB revision。
`setup` 仍由操作员选择 snapshot 或 full sync；选择 snapshot 后先安装 core，controller 再处理
可选 registry，registry 不阻塞核心服务启动。

上传使用“组件文件在前、record 最后”的顺序；`publish` 运行中 record 可能尚不可访问。发布 candidate
前必须通过工作流中的 `snapshot_distribution.py verify-public`，核验公开 record、全部对象长度和
两个 DB 的 byte range。本次 record 已通过公网检查（8 个文件、2 个 DB Range）；语义审计单独验收。

## 尚未冻结

- 三个发布镜像的 digest 与最终三仓 release manifest；candidate workflow 已具备，但尚待实际 artifact。
- 上述拆分 snapshot 的目标机安装验收；安装时逐文件完成 SHA-256 与签名校验。
- 三台机器的 bootnode enode、外部 IP 和 miner pass。
- 正式 PoW calibration 报告。
- SourceDAO bootstrap 执行记录和完成 checkpoint。

这些内容完成后才能把 bundle 状态提升为 release candidate。
