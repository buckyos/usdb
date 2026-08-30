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
- `usdb-node`、Bitcoin/runtime helper、主机、防火墙和 snapshot download/install 工具。

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
usdb-node prepare-host
usdb-node setup
usdb-node doctor
```

`prepare-host` 先执行完整只读主机检查；检查失败时才询问是否调用受支持的 APT 安装流程。软件安装会明确请求
确认和 sudo，不会由 `setup` 或 `doctor` 隐式触发。自动化和故障排查可直接使用：

```bash
usdb-node host check
usdb-node host install
```

如果机器连 release installer 所需的 `curl`/Python 都没有，仍需先使用主机软件基线文档中的独立
`prepare_usdb_host.sh` bootstrap 路径。

`setup` 只询问无法从 release 或主机安全推导的值：数据根、节点角色、miner 地址/线程、joiner
bootnode，以及是否开放 Bitcoin 入站 P2P。它从当前 `SSH_CONNECTION` 检测宿主机 SSH server port，显示并要求
确认；无法检测时默认建议 `22`。这里记录的是宿主机实际监听端口，不是客户端临时端口或云 NAT 的外部端口。
默认使用 `~/.usdb`、full role、Bitcoin private P2P。

它自动完成：

- 从 manifest 写入三张 image digest；
- 在一个 `USDB_DATA_ROOT` 下生成所有绝对数据目录；
- 根据 bundle ID 和 hostname 派生 Bitcoin RPC username；
- 生成 Bitcoin RPC password 和 `rpcauth`，但不打印密码；
- 写入权限为 `0600` 的 `node.env`；
- 校验 release/network identity、RPC credential 和所有安全 bind address。
- 保存已确认的 operator SSH port；
- 询问是否立即应用并验证 UFW profile，默认是 `yes`。

UFW 写操作会再次显示确认问题，并通过 sudo 在放行 SSH 后才启用默认拒绝入站策略。选择不应用时，`setup`
仍保留已生成配置，并提示稍后显式执行：

```bash
usdb-node firewall apply --confirm
```

完整 firewall check 必须在 `setup` 后运行，因为它需要依据生成的 `node.env` 对照 SSH、USDB P2P、Bitcoin
P2P 和 operator API 的实际 bind policy。高级入口为 `usdb-node firewall check` 和
`usdb-node firewall apply --confirm`；`--ssh-port` 仅用于显式覆盖已经保存的端口。

`setup` 拒绝覆盖已有 `node.env` 或 `rpcauth`。角色切换使用 `set-role`，不重新生成 secret。

正式 snapshot 是可选启动加速器。取得经过 review 的 content-addressed record URL 后，在第一次
`up` 前执行：

```bash
usdb-node snapshot install --record-url \
  https://usdb-snapshot.tbudr.top/snapshot-records/v1/<record-sha256>.json
```

该命令不需要 S3 凭证；它先校验小 record 与 bundle trusted-key catalog，再断点下载大文件、逐文件
校验、原子发布并更新 `node.env`。snapshot 高度高于当前 bundle index origin、network/catalog 不匹配、
或 balance-history DB 已初始化时都会失败关闭。完整操作见
[Snapshot 对象存储发布与安装](./balance-history-snapshot-object-storage.md)。

`doctor` 是一次性、只读的启动前检查，不是后台健康监控服务。它会检查：

- Linux kernel/架构、Docker/Compose、Git、Python、curl、jq 和 Docker daemon/user access；
- release manifest、network bundle 和节点私有配置是否相互一致；
- `node.env` 的路径、RPC credential、安全 bind address 和角色配置是否有效；
- 三张 image 是否仍是当前已安装 release 冻结的 digest。
- UFW 是否 active、默认入站策略、SSH/USDB/Bitcoin 规则是否与 `node.env` 一致，以及敏感 RPC 端口是否未开放。

`doctor` 不拉取 image、不启动或停止容器，也不修改 `node.env`。首次配置后单独执行它，便于在开放防火墙或
开始长时间 Bitcoin IBD 前尽早发现问题；`usdb-node up` 也会先执行同一组检查，因此正常启动不依赖运维人员
预先手工运行 `doctor`。服务启动后的当前状态使用 `usdb-node status`，持续运行期间依赖 Docker healthcheck、
restart policy 和各服务自身的 readiness/consensus gate，不能把 `doctor` 当作监控探针。读取 UFW 状态可能
请求 sudo，但仍是只读操作；上游云防火墙不在宿主机可见范围内，仍需独立复核。

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

安装同一 network bundle 的新 `rN` 后，如果继续复用该 bundle 已有的 `node.env`，先显式激活新 release
冻结的 image：

```bash
usdb-node activate-release
usdb-node doctor
usdb-node up
```

`activate-release` 只替换三个 release-owned image digest；如果 network bundle 变化，则使用新的
bundle-scoped `node.env`，不能把旧 genesis 的配置直接带入新网络。

`activate-release` 的使用边界如下：

| 场景 | 是否执行 | 原因 |
| --- | --- | --- |
| 首次安装并运行 `setup`/`configure` | 否 | 新配置已经写入当前 release 的 image digest |
| 同一 bundle 从 `rN` 升级到 `rN+1`，复用原 `node.env` | 是 | bundle-scoped 配置仍记录旧 release image digest |
| 重复安装或重启同一 release | 否 | release identity 和配置没有变化 |
| 新建 `vN` network bundle、chain ID 或 genesis | 禁止复用旧配置 | 应运行新 bundle 的 `setup`，使用独立配置和数据处置流程 |

该命令不拉取 image、不重启容器、不修改 RPC secret、角色、bootnode、数据路径或 miner 配置。更新采用原子写入；
校验失败时恢复原配置。激活后运行 `doctor`，再执行 `up` 让 Compose 拉取并协调新 image。若跳过激活，
`doctor` 和 `up` 都会因 image digest 与当前 release 不一致而失败关闭。

## 4. 启动和续跑

```bash
usdb-node up
usdb-node status
```

首次部署路径是 `prepare-host -> setup -> doctor -> up -> status`。`setup` 已成功应用 UFW 时，可以省略
单独的 `doctor`，因为 `up` 会重新执行；同一 bundle 的 release 升级路径是
`install new release -> activate-release -> doctor -> up -> status`。

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
- snapshot content-addressed record、断点 staging、原子选择和已有 DB 拒绝。

仍需在新 release ID 上完成 GitHub workflow 产物检查，并在空白目标机执行一次从 installer、可选
R2 snapshot 到三类 image、Bitcoin IBD/resume 和完整 runtime readiness 的跨进程 E2E。
