# USDB Release Node Kit 与简化部署

Status: first implementation complete; target-host and published-release E2E pending.

本文定义 USDB 节点发布包与部署控制器的边界。目标是让运维人员不再手工读取 commit、复制 image
digest、编辑 Bitcoin RPC 密码或记忆服务启动顺序，同时保留不可变 release、私钥隔离和失败关闭。

## 1. 配置分层

部署输入严格分为三层：

| 层 | 所有者 | 内容 | 是否进入 Release |
| --- | --- | --- | --- |
| release identity | 发布流程 | release ID、三仓 revision、三张 digest-pinned image | 是 |
| network identity | network bundle | chain/network ID、genesis、BTC origin/registry、SourceDAO public config | 是 |
| node-local state | 节点运维 | 数据路径、RPC secret、role、bootnodes、NAT、miner address | 否 |

前两层由 `usdb-release-publish.yml` 冻结。节点只提供第三层，不能覆盖 manifest 中的 image 或 network
identity。Bootstrap Admin 私钥、矿工私钥和 snapshot signing key 都不进入 node kit。

## 2. GitHub Release 资产

每个新 release 包含：

- `usdb-release-manifest.json` 及 SHA-256；
- `<release_id>-network-bundle.tar.gz` 及 SHA-256；
- `<release_id>-node-kit.tar.gz` 及 SHA-256；
- `install-<release_id>.sh` 及 SHA-256；
- `install-usdb-node.sh` 及 SHA-256。

node kit 是自包含的部署控制面，包含：

- release manifest；
- 对应 network bundle；
- `compose.bitcoin.yml` 和 `compose.runtime.yml`；
- network、release 和 readiness validators；
- `usdb-node`、Bitcoin/runtime helper、主机与防火墙工具。

节点不再 clone Git 仓库，也不根据 source revision 构建镜像。Node kit 不包含容器 image、secret、
snapshot 大文件或任何节点数据库。

## 3. 安装和配置

每个 release 提供唯一、不可跨 release 复用的快捷安装脚本。它内置 release ID、GitHub asset URL、
manifest、node kit 和通用 installer 的 SHA-256：

```bash
RELEASE_ID=usdb-testnet-v0-r1
bash <(curl -fsSL \
  "https://github.com/buckyos/usdb/releases/download/${RELEASE_ID}/install-${RELEASE_ID}.sh")
```

快捷脚本先校验通用 installer，再由通用 installer 校验 release 自带 checksum 和脚本内冻结的
manifest/node-kit digest。它安装到
`~/.local/share/usdb/releases/<release_id>`，并创建 `~/.local/bin/usdb-node`。对同一不可变 release
重复执行是幂等的；如果目标目录内容不同则拒绝覆盖。

mainnet 或需要先审阅脚本时，下载 `install-<release_id>.sh` 及其 checksum，执行 `sha256sum -c` 后
再运行。`install-usdb-node.sh --release-id ...` 保留为镜像站和故障排查使用的高级入口，不作为默认操作。

节点私有配置独立保存在 `~/.config/usdb/<bundle-id>/node.env`，不属于任何 `rN` release 目录。
因此同一 network bundle 升级 release 时可继续使用已有 Bitcoin RPC secret、角色和拓扑。

首次配置使用交互向导：

```bash
export PATH="${HOME}/.local/bin:${PATH}"
usdb-node setup
usdb-node doctor
```

`setup` 只询问无法从 release 或主机安全推导的值：数据根、节点角色、miner 地址/线程、joiner
bootnode，以及是否开放 Bitcoin 入站 P2P。默认使用 `~/.usdb`、full role、Bitcoin private P2P。

它自动完成：

- 从 manifest 写入三张 image digest；
- 在一个 `USDB_DATA_ROOT` 下生成所有绝对数据目录；
- 根据 bundle ID 和 hostname 派生 Bitcoin RPC username；
- 生成 Bitcoin RPC password 和 `rpcauth`，但不打印密码；
- 写入权限为 `0600` 的 `node.env`；
- 校验 release/network identity、RPC credential 和所有安全 bind address。

它拒绝覆盖已有 `node.env` 或 `rpcauth`。角色切换使用 `set-role`，不重新生成 secret。

无人值守部署继续使用确定性的底层接口，例如：

```bash
usdb-node configure --role full --data-root /data/usdb
```

`--bitcoin-rpc-user`、`--sync-timeout-secs`、`--nat` 等只属于高级覆盖项，不需要进入普通节点手册。

宿主机持久化布局统一为：

```text
<USDB_DATA_ROOT>/bitcoin/mainnet
<USDB_DATA_ROOT>/balance-history
<USDB_DATA_ROOT>/usdb-indexer
<USDB_DATA_ROOT>/usdb-chain
<USDB_DATA_ROOT>/control-plane
<USDB_DATA_ROOT>/secure
<USDB_DATA_ROOT>/releases
```

这些目录通过 bind mount 映射到容器内稳定的 `/data/*` 路径。默认根是 `~/.usdb`；专用数据盘使用
`usdb-node configure --data-root /data/usdb ...` 或在 `setup` 中选择 `/data/usdb`。工具不会自行迁移旧目录。

安装同一 network bundle 的新 `rN` 后，先显式激活新的 manifest image：

```bash
usdb-node activate-release
usdb-node doctor
usdb-node up
```

`activate-release` 只替换三个 release-owned image digest；如果 network bundle 变化，则使用新的
bundle-scoped `node.env`，不能把旧 genesis 的配置直接带入新网络。

## 4. 启动和续跑

```bash
usdb-node up
usdb-node status
```

`up` 内部顺序固定为：

1. 运行 release、network、node 和 Docker preflight；
2. 拉取三张 digest-pinned image；
3. 启动 Bitcoin Core，并等待 mainnet IBD、txindex、peer 和 tip readiness；
4. 启动 balance-history 并等待 consensus readiness；
5. 启动 usdb-indexer 并等待 consensus readiness；
6. 初始化并启动 USDB chain 与 control-plane。

默认同步等待上限是 7 天，普通操作无需提供该参数；高级模式可使用 `--sync-timeout-secs` 覆盖。
命令中断或等待超时不会删除容器、bind-mounted 数据。重新执行相同 `up` 会从现有同步状态继续。长期 Bitcoin
IBD 可以在 `tmux`、`screen` 或受控 systemd unit 中运行；后续可再为 node kit 增加 systemd 模板。

常用命令：

```bash
usdb-node status
usdb-node logs balance-history
usdb-node logs usdb-chain
usdb-node logs --bitcoin
usdb-node down
usdb-node down --include-bitcoin
```

默认 `down` 不停止独立 Bitcoin Core，且所有命令都不会执行 `compose down -v`。

## 5. 矿工和 Joiner

切换矿工角色：

```bash
usdb-node set-role \
  --role miner \
  --miner-address 0x1111111111111111111111111111111111111111 \
  --miner-threads 1
usdb-node up
```

GPU remote sealer 使用 `--miner-threads 0`。Joiner 在首次 `configure` 时传入 `--bootnodes` 和必要的
`--nat`；这些是节点本地拓扑，不属于 release identity。

## 6. 保留人工确认的事项

以下动作不应为了“一键部署”而自动化绕过：

- 主机软件安装和 root 级防火墙修改；
- GHCR 私有 package 登录；
- SourceDAO Bootstrap Admin 私钥使用和链上 bootstrap；
- miner address 与 Active Standard pass 的上线复核；
- PoW calibration、release approval 和深 BTC reorg 整网重置批准；
- snapshot 来源、签名和 paired checkpoint 选择。

SourceDAO bootstrap 保持独立，是因为 node kit 不应接触管理员私钥。未来可以增加一个只生成待签交易和
验收报告的子命令，但不能把 signer secret 放入 Compose environment。

## 7. 当前验证边界

本批次覆盖：

- release manifest 与 bundled network identity 交叉校验；
- image digest、路径、RPC credential 和 role 配置生成；
- 配置拒绝覆盖和 role 原子更新；
- Bitcoin/runtime 启动调用顺序；
- installer checksum、release identity 和幂等安装。

仍需在新 release ID 上完成 GitHub workflow 产物检查，并在空白目标机执行一次从 installer 到三类
image、Bitcoin IBD/resume 和完整 runtime readiness 的跨进程 E2E。
