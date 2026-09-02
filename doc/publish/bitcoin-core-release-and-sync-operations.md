# Bitcoin Core Release Image 与同步操作

## 1. 目标与边界

testnet-v0 的每个节点运行独立 Bitcoin Core 28.1 mainnet full node。Bitcoin Core 不是 USDB 链数据，
也不随 testnet reset、genesis 替换或 USDB runtime 停止而清理。

发布路径由以下文件组成：

- `docker/Dockerfile.bitcoin-core`：验证并打包官方 Bitcoin Core 28.1 binary；
- `.github/workflows/usdb-bitcoin-image.yml`：发布 GHCR candidate、SBOM 和 provenance attestation；
- `docker/compose.bitcoin.yml`：独立 Bitcoin Compose project；
- `docker/scripts/tools/run_testnet_bitcoin.sh`：节点侧生命周期入口；
- `docker/scripts/tools/check_bitcoin_readiness.py`：USDB 启动前同步门禁。

开发期 `compose.base.yml` 中的第三方 `latest` 镜像不属于 release 路径。

## 2. 上游 Artifact 信任

release image 只构建 `linux/amd64`，固定：

| 项目 | 值 |
| --- | --- |
| Bitcoin Core | `28.1` |
| archive | `bitcoin-28.1-x86_64-linux-gnu.tar.gz` |
| archive SHA-256 | `07f77afd326639145b9ba9562912b2ad2ccec47b8a305bd075b4f4cb127b7ed7` |
| guix.sigs revision | `3b667ee3ebb3dcd9e1990cf03e38a0935eec1683` |
| required checksum signers | `achow101`、`laanwj`、`0xb10c` |

Docker build 会校验三个固定 key artifact 的 SHA-256，要求三个 fingerprint 都对官方
`SHA256SUMS.asc` 产生有效签名，再用已签名 checksum 验证 archive。镜像自身随后由 GitHub-hosted
workflow 生成 provenance attestation。Bitcoin Core 官方下载与验证方法见：

- <https://bitcoincore.org/en/releases/28.1/>
- <https://bitcoincore.org/en/download/>

节点只使用 manifest 中的 digest reference：

```text
ghcr.io/buckyos/usdb-bitcoin-core@sha256:<64-char-digest>
```

## 3. 节点拓扑与数据

Bitcoin Compose 和 USDB runtime 是两个 Compose project，共享 bundle 固定的 external Docker
network。默认 endpoint 为 `http://btc-node:8332`：

- `8333/TCP` 默认只绑定宿主机 loopback；可按节点角色选择发布到公网，用于 Bitcoin 入站 P2P；
- `8332/TCP` 不映射到宿主机，也不能暴露公网；
- Bitcoin 数据使用 `BTC_NODE_DATA_HOST_DIR` bind mount；
- `run_testnet_runtime.sh down` 不停止 Bitcoin、不删除 shared network、不触碰 Bitcoin 数据；
- `run_testnet_bitcoin.sh down` 只停止容器，不删除 bind-mounted data directory。

新节点的默认部署目录由 `usdb-node` 生成，例如 testnet-v0：

```text
/home/usdb/.usdb/datasets/bitcoin/btc-mainnet
/home/usdb/.usdb/networks/usdb-testnet-v0/secure/bitcoin-mainnet-rpcauth
```

实际路径以 bundle-scoped `node.env` 为准。已有非裁剪、`txindex=1` 的 mainnet 数据目录不能由新工具
自动认领；需先按兼容契约完成显式审核和 marker 建立。同一目录任意时刻只能由一个 bitcoind 进程打开。
切换前必须等待旧进程完整退出，并确认容器 UID/GID 对目录有读写权限。

关闭公网 `8333/TCP` 不影响 Bitcoin Core 主动建立出站 peer 和同步主链，只是不接收入站 peer。
如需公开 `8333`，必须同时将 `BTC_P2P_BIND_ADDRESS=0.0.0.0` 并采用 firewall public profile；仅增加
UFW allow 规则不足以改变 loopback-only Docker bind。具体见
[USDB 节点防火墙与端口暴露操作](./usdb-node-firewall-operations.md)。

## 4. 初始化 RPC 身份

先创建私有 node env：

```bash
docker/scripts/tools/run_testnet_runtime.sh init-env
```

编辑 `BTC_RPCAUTH_HOST_FILE` 路径后生成专用账户：

```bash
docker/scripts/tools/run_testnet_bitcoin.sh init-rpc-auth usdb-testnet
```

该命令原子创建 mode `0600` 的 rpcauth 文件，并只在终端输出一次随机 password。把输出的 username 和
password 写入未提交的 `node.env`。validator 会复算 HMAC，拒绝用户名、password、rpcauth 不一致或
权限过宽的配置。不要把输出写入 Git、普通日志或 release manifest。

## 5. 启动与同步

填入 digest-pinned `USDB_BITCOIN_IMAGE` 后执行：

```bash
docker/scripts/tools/run_testnet_bitcoin.sh validate
docker/scripts/tools/run_testnet_bitcoin.sh pull
docker/scripts/tools/run_testnet_bitcoin.sh up
```

`up` 会创建共享 Docker network、启动 Bitcoin Core，然后持续等待 readiness。首次主网同步可能需要很
长时间；可以中断等待命令，容器会继续同步，之后使用：

```bash
docker/scripts/tools/run_testnet_bitcoin.sh ps
docker/scripts/tools/run_testnet_bitcoin.sh logs
docker/scripts/tools/run_testnet_bitcoin.sh progress
docker/scripts/tools/run_testnet_bitcoin.sh status
docker/scripts/tools/run_testnet_bitcoin.sh wait
```

`up/wait` 会立即输出一次等待状态，之后默认每 60 秒向 stderr 输出 UTC 时间、elapsed、blocks/headers、
Bitcoin Core verification progress、txindex 高度、peer 数和当前 blockers。`progress` 只查询一次，输出
`usdb-bitcoin-readiness:v1` JSON，适合监控程序和另一条 SSH 会话读取；它不会改变容器或同步状态。

readiness 必须同时满足：

1. `getblockchaininfo.chain == main`；
2. `pruned == false`；
3. `initialblockdownload == false`；
4. `blocks == headers` 且高度至少为 `963800`；
5. `getindexinfo.txindex.synced == true`；
6. `txindex.best_block_height == blocks`；
7. tip 时间不早于当前时间 `7200` 秒、`networkactive == true`，且至少有一个 peer。

TCP 可连、只达到 snapshot 高度、Bitcoin block 已追平但 txindex 未追平，都不是 ready。USDB runtime
启动命令会再次执行同一检查，失败时不启动 balance-history/indexer/chain。

## 6. 资源基线

32 GiB 共机模板默认：

| 组件 | cgroup 上限/关键 cache |
| --- | --- |
| Bitcoin Core | `5g`，`dbcache=3072 MiB` |
| balance-history | `12g`，UTXO `2 GiB`，balance `6 GiB` |
| indexer / chain / control-plane | `4g / 5g / 1g` |

hard limits 合计 `27 GiB`，为内核、page cache、Docker 和短时峰值保留约 `5 GiB`。这是安装
height-963800 snapshot 后的增量运行模板，不是从 BTC genesis 构建 balance-history snapshot 的 profile。
容量紧张或发生 cgroup OOM 时应提高机器内存，不得通过启用 Bitcoin pruning 规避。

Bitcoin blocks、chainstate 和 txindex 需要长期独立磁盘容量。上线前以实际数据目录加增长余量做
preflight，不把 snapshot 下载目录、Docker build cache 或 USDB chain DB 与其误算为可用空间。

## 7. 停止、升级与恢复

正常停止：

```bash
docker/scripts/tools/run_testnet_bitcoin.sh down
```

升级 image 时保持数据目录不变，修改 digest 后依次执行 `pull`、`up`、`status`。升级前后记录
`getblockchaininfo`、`getindexinfo`、image digest 和数据目录备份策略。若新 image 无法打开已有 DB，停止
并回退旧 digest；不要同时启动两个版本，也不要自动删除/重建 BTC 数据。

当前不提供 Bitcoin data snapshot 分发。节点从已有受控目录或官方 P2P 同步；以后若发布 Bitcoin data
artifact，需要独立定义来源、校验、assume-valid 边界和恢复测试，不能复用 balance-history snapshot 的
签名含义。
