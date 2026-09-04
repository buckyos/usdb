# USDB Release Node Kit 与简化部署

Status: first implementation complete; target-host and published-release E2E pending.

本文定义 USDB 节点发布包与部署控制器的边界。目标是让运维人员不再手工读取 commit、复制 image
digest、编辑 Bitcoin RPC 密码或记忆服务启动顺序，同时保留不可变 release、私钥隔离和失败关闭。

## 1. 配置分层

部署输入严格分为四层：

| 层 | 所有者 | 内容 | 是否进入 Release |
| --- | --- | --- | --- |
| release identity | 发布流程 | release ID、三仓 revision、三张 digest-pinned image | 是 |
| network identity | network bundle | chain/network ID、genesis、BTC origin/registry、SourceDAO public config | 是 |
| runtime compatibility | release manifest | data layout、各服务 storage/source identity、compatibility ID | 是 |
| node-local state | 节点运维 | 数据路径、RPC secret、role、bootnodes、NAT、miner address | 否 |

前三层由 `usdb-release-publish.yml` 冻结。节点只提供第四层，不能覆盖 manifest 中的 image、network 或 runtime
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

该安装脚本只安装经过校验的 node kit 和 launcher，不自动修改宿主机、生成节点配置或启动服务。成功后会明确
输出首次部署的 `prepare-host -> setup -> doctor -> up -> status` 路径，以及同 bundle 升级的
`activate-release -> controller install -> doctor -> up -> status` 路径。

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
bootnode、是否开放 Bitcoin 入站 P2P，以及 firewall mode。只有选择 managed UFW 时，它才从当前
`SSH_CONNECTION` 检测宿主机 SSH server port 并要求确认；这里不是客户端临时端口或云 NAT 外部端口。
默认使用 `~/.usdb`、full role、Bitcoin private P2P 和 external firewall。

输入 data root 后，`setup` 会立即检查该路径所在文件系统，而不是只检查目录本身。文件系统总容量和当前
可用空间任一少于 `1.5 TiB` 都会在创建配置、凭证或开始 snapshot/Bitcoin 下载前失败关闭；`2.0 TiB` 是
长期运行建议容量。常见标称 `2 TB` 磁盘格式化后约为 `1.8 TiB`，可通过硬下限但会显示低于建议值的警告。
底层 `configure --data-root` 执行相同硬检查，自动化不能绕过。

它自动完成：

- 从 manifest 写入三张 image digest；
- 校验 data root 所在文件系统的总容量和当前可用空间；
- 在一个 `USDB_DATA_ROOT` 下按 source dataset 与 network bundle 生成绝对目录；
- 写入 release runtime compatibility ID，并为每个服务目录创建公开 dataset identity marker；
- 根据 bundle ID 和 hostname 派生 Bitcoin RPC username；
- 生成 Bitcoin RPC password 和 `rpcauth`，但不打印密码；
- 写入权限为 `0600` 的 `node.env`；
- 校验 release/network identity、RPC credential 和所有安全 bind address。
- 保存检测到的 operator SSH port；managed 模式额外要求 operator 确认；
- 显示 release-approved snapshot 的高度、下载量和建议剩余空间，并询问使用 snapshot 还是 full sync；
- 选择宿主机 firewall 模式，默认 `external`；只有明确选择 `managed` 才安装、应用并验证 UFW profile；
- 默认安装并 enable bundle-scoped systemd bootstrap controller，但不在 `setup` 内启动长时间同步。

只有明确的前台调试、CI、非 systemd 主机或外部编排环境才使用 `usdb-node setup --no-controller`。该选项仅
生成节点配置；之后必须执行 `usdb-node up --foreground`，或补充执行 `usdb-node controller install` 后再使用
普通后台 `up`。

`external` 表示防火墙由云平台、虚拟化环境或 operator 管理，或该节点处于不需要 UFW 的隔离 VM；此模式下
`usdb-node` 不安装、不读取、不修改 UFW，但仍校验所有容器 bind address。切换到 managed 并应用：

```bash
usdb-node set-firewall-mode --mode managed
usdb-node firewall apply --confirm
```

默认 `setup` 为安装 systemd unit 要求 operator 是 root，或拥有可用密码和 sudo 权限；sudo 验证当前 operator
密码，不接受 root 密码。`setup --no-controller` 的 external firewall 模式不需要 sudo，但该 operator 为控制
Docker 仍需加入 Docker group，而 Docker group 本身具备等价 root 的主机控制能力。

managed 模式的完整 firewall check 依据 `node.env` 对照 SSH、USDB P2P、Bitcoin P2P 和 operator API 的
实际 bind policy。external 模式的 `doctor/up` 跳过 UFW inspection，但相同的 bind policy 校验不会跳过。
高级入口为 `set-firewall-mode`、`firewall check` 和 `firewall apply --confirm`。

`setup` 拒绝覆盖已有 `node.env` 或 `rpcauth`。角色切换使用 `set-role`，不重新生成 secret。

正式 snapshot 是可选启动加速器。release manifest 已冻结经过 review 的 content-addressed record、
高度、BTC block hash、snapshot ID、下载规模和 trusted-key catalog。交互式 `setup` 会显示这些信息并
询问是否使用；选择后只把 release-approved snapshot 固定到 `node.env`，不在前台执行长时间下载。
artifact 下载、校验和选择由后续 systemd bootstrap controller 可续传执行，不要求运维人员填写 URL；
live RocksDB 导入仍由之后的 `snapshot-loader` 完成。

若 `setup` 中选择 full sync，之后仍可在 balance-history 第一次启动前执行：

```bash
usdb-node snapshot install
```

该命令只使用当前 release 批准的 record，不接受任意 URL，也不需要 S3 凭证。它先把 snapshot 选择写入
`node.env`，再校验小 record 与 bundle trusted-key catalog、默认用 `8 x 64 MiB` HTTP Range 并行续传
大文件、逐文件校验并原子发布。高级硬件可通过 `--download-concurrency` 和
`--download-chunk-size-mib` 覆盖本次下载参数。
如果 `<USDB_DATA_ROOT>/artifacts/balance-history/<snapshot-release-id>` 已完整存在，命令使用 release bundle
内冻结的本地 record 逐文件复核 size/SHA-256，跳过全部网络下载并直接完成 snapshot 选择。目录存在但
record 缺失、文件不完整或哈希不一致时失败关闭，不会把可疑目录当作缓存命中，也不会静默覆盖。这里指的是
已经进入最终不可变目录的异常状态；正常下载中断只会留下 `.<snapshot-release-id>.installing`、`.part` 和
`.ranges.json`，重跑命令会继续下载而不是按上述规则拒绝。
大文件下载完成后的 SHA-256 阶段显示 `Verify downloaded ...` 的字节、吞吐和 ETA；首次安装只进行这一次
完整哈希。复用已有最终 artifact 时则显示 `Verify cached ...`。managed 模式会在 snapshot 下载前应用 UFW，
并单独提示可能请求 operator sudo 密码；external 模式不执行该步骤。
下载中断后 controller 会由 systemd 退避重启并继续 `.part/.ranges.json`；手工重跑同一命令也会续传。
snapshot 高度高于当前
bundle index origin、network/catalog 不匹配、磁盘文件异常或 balance-history DB 已初始化时都会失败关闭。
完整操作见
[Snapshot 对象存储发布与安装](./balance-history-snapshot-object-storage.md)。

`doctor` 是一次性、只读的启动前检查，不是后台健康监控服务。它会检查：

- Linux kernel/架构、Docker/Compose、Git、Python、curl、jq 和 Docker daemon/user access；
- release manifest、network bundle 和节点私有配置是否相互一致；
- `node.env` 的路径、RPC credential、安全 bind address 和角色配置是否有效；
- 三张 image 是否仍是当前已安装 release 冻结的 digest。
- 始终检查安全 bind address；managed 模式额外检查 UFW active/default/rules，external 模式明确跳过 UFW。

`doctor` 不拉取 image、不启动或停止容器，也不修改 `node.env`。首次配置后单独执行它，便于在开放防火墙或
开始长时间 Bitcoin IBD 前尽早发现问题；`usdb-node up` 也会先执行同一组检查，因此正常启动不依赖运维人员
预先手工运行 `doctor`。服务启动后的当前状态使用 `usdb-node status`，持续运行期间依赖 Docker healthcheck、
restart policy 和各服务自身的 readiness/consensus gate，不能把 `doctor` 当作监控探针。managed 模式读取
UFW 状态可能请求 sudo；external 模式的上游或宿主机防火墙不在工具检查范围内，必须独立复核。

无人值守部署继续使用确定性的底层接口，例如：

```bash
usdb-node configure --role full --data-root /data/usdb --firewall-mode external
```

`--bitcoin-rpc-user`、`--sync-timeout-secs`、`--nat` 等只属于高级覆盖项，不需要进入普通节点手册。

新节点的宿主机持久化布局统一为：

```text
<USDB_DATA_ROOT>/datasets/bitcoin/<btc-network-id>
<USDB_DATA_ROOT>/datasets/balance-history/<btc-network-id>/<balance-history-contract-id>
<USDB_DATA_ROOT>/datasets/usdb-indexer/<indexer-contract-id>
<USDB_DATA_ROOT>/artifacts/balance-history
<USDB_DATA_ROOT>/networks/<bundle-id>/usdb-chain
<USDB_DATA_ROOT>/networks/<bundle-id>/control-plane
<USDB_DATA_ROOT>/networks/<bundle-id>/secure
```

这些目录通过 bind mount 映射到容器内稳定的 `/data/*` 路径。默认根是 `~/.usdb`；专用数据盘使用
`usdb-node configure --data-root /data/usdb ...` 或在 `setup` 中选择 `/data/usdb`。hash 路径由工具生成，
以 `node.env` 为准。工具不会自行迁移、复制或认领旧目录。完整兼容边界见
[Network 数据布局与 Release 兼容契约](./usdb-network-data-layout-and-release-compatibility.md)。

安装同一 network bundle 的新 `rN` 后，如果继续复用该 bundle 已有的 `node.env`，先显式激活新 release
冻结的 image：

```bash
usdb-node activate-release
usdb-node controller install
usdb-node doctor
usdb-node up
```

`activate-release` 先校验 runtime compatibility ID、所有数据路径和 dataset marker，完全一致后才替换三个
release-owned image digest。如果 contract 或 network bundle 变化，则使用新的 bundle-scoped `node.env`
和相应数据目录，不能把旧 genesis 的配置直接带入新网络。

`activate-release` 的使用边界如下：

| 场景 | 是否执行 | 原因 |
| --- | --- | --- |
| 首次安装并运行 `setup`/`configure` | 否 | 新配置已经写入当前 release 的 image digest |
| 同一 bundle 从 `rN` 升级到 `rN+1`，runtime contract 不变 | 是 | 只需更新 release image digest |
| 同一 bundle 的新 release 改变 storage/source contract | 禁止 | 当前不实现迁移，使用新配置和 rebuild |
| 重复安装或重启同一 release | 否 | release identity 和配置没有变化 |
| 新建 `vN` network bundle、chain ID 或 genesis | 禁止复用旧配置 | 应运行新 bundle 的 `setup`，使用独立配置和数据处置流程 |

该命令不拉取 image、不重启容器、不修改 RPC secret、角色、bootnode、数据路径、compatibility marker 或 miner
配置。更新采用原子写入；
校验失败时恢复原配置。`controller install` 是幂等的，会刷新 unit 但不会启动服务；首次安装后已经执行过时，
同一 operator 和参数的普通 release 升级可以省略。激活后运行 `doctor`，再执行 `up` 让 controller 拉取并
协调新 image。若跳过激活，`doctor` 和 controller 都会因 image digest 与当前 release 不一致而失败关闭。

## 4. 启动和续跑

默认 `setup` 已以日常节点 operator 安装 controller；独立 `controller install` 用于修复 unit、刷新 release
launcher 或冻结高级参数。命令仅在写入 `/etc/systemd/system` 和调用 `systemctl` 时内部请求 sudo，unit 的
`User`、`HOME`、private `node.env` 和稳定 launcher 都冻结为当前 operator。不要通过
`sudo usdb-node controller install` 改变 service user；只有节点本来就由 root 管理时才直接以 root 执行。
unit 的 `ExecStart` 使用 installer 创建的稳定 `~/.local/bin/usdb-node` 软链接，而不是某个不可变 release 目录，
release activation 仍必须由 operator 显式执行。

```bash
usdb-node up
usdb-node status
```

首次部署路径是 `prepare-host -> setup -> doctor -> up -> status`。无论 external
还是 managed，均可省略单独的 `doctor`，因为 controller 会重新执行对应模式的 preflight；同一 bundle 的
release 升级路径是 `install new release -> activate-release -> controller install -> doctor -> up -> status`。
`setup --bitcoin-profile auto` 会根据主机可见物理内存在 `balanced-32g` 与 `performance-64g` 之间选择；
自动化 `configure` 应显式指定 profile。`ibd-64g` 是 64 GiB 主机首次 Bitcoin IBD/txindex 的临时档，
不会被 `auto` 选择，完成后必须切回 `performance-64g`。每个 profile 同时冻结 memory、memory+swap 和
`dbcache`。运行中调整先执行 `down` 停止 controller 和所有容器，然后使用 `set-bitcoin-profile`，最后重新执行 `up`；数据目录不会
因此重建，profile 修改也会拒绝仍有运行中容器的状态。

默认的 `up` 不把数天的初始化生命周期绑定到当前终端。它向 bundle-scoped systemd unit 提交任务，
然后在交互式终端附加只读进度面板；Ctrl+C、SSH 断开或本地界面退出只会脱离观察，不会停止 controller。
systemd unit 内部顺序固定为：

1. 运行 release、network、node 和 Docker preflight；
2. 拉取三张 digest-pinned image；
3. 启动 Bitcoin Core；达到 snapshot 高度（无 snapshot 时为 BTC index origin）并通过可选 block-hash
   锚点校验后，启动 snapshot-loader 和 balance-history；
4. balance-history 达到 USDB origin、进入 query-ready 且存在 block hash/commit 后启动 usdb-indexer；
5. Bitcoin Core、balance-history 和 usdb-indexer 继续流水线追块；
6. 重新要求 Bitcoin mainnet IBD、txindex、peer、tip readiness，以及两个索引服务的 consensus
   readiness，全部通过后才初始化并启动 USDB chain 与 control-plane。

controller 默认单次同步等待上限是 7 天；超时或其他临时失败后由 systemd 以 30 秒退避重启，并从现有容器和
数据状态继续。systemd 在 30 分钟窗口内最多允许 20 次启动，持续故障会停止而不是无限刷日志；Docker daemon
未就绪由 unit 的 `docker info` 前置检查归入这一有界重试。高级参数在
`controller install --sync-timeout-secs ... [--skip-pull]` 时冻结；前台调试可使用
`up --foreground` 并为该次运行覆盖参数。controller 在每个阶段输出 UTC 时间和阶段名；长时间
Bitcoin、balance-history、usdb-indexer readiness 等待会定期把 elapsed、同步高度、百分比和 blocker 写入
journal。

交互式 TTY 中，`up` 提交 controller 后会自动显示固定五行的只读进度面板：可选 snapshot、Bitcoin、
balance-history、usdb-indexer 和 USDB chain。snapshot 未选择时显示 `SKIPPED`；并行 range 下载按已完成
chunk 的实际字节计数，不把预分配文件误算为完成。artifact 下载并校验完成后显示 `WAITING`，明确等待
Bitcoin tip 达到 stable anchor 加 registry lag 的 data-start gate；`snapshot-loader` 开始把 SQLite 导入 live RocksDB 后显示 `IMPORTING`，并区分 source
verify、staging DB、balance history、UTXO、block commit、script registry、finalize 和 atomic swap 八个阶段。
只有匹配的 `snapshot-loader.done.json` 与非空 live DB 同时存在才显示 `READY`。USDB chain 使用标准
`eth_syncing`、`eth_blockNumber` 和 `net_peerCount`，同时只读核对 `eth_chainId` 与 genesis hash。面板只观察
已有状态，不启动、停止、重试或放宽任何 readiness gate；非 TTY、重定向和 `up --json` 提交 controller
后立即返回，阶段日志与 heartbeat 通过 `usdb-node controller logs --follow` 查询。

需要在另一个终端持续观察，或由监控系统采集单次结构化状态时使用：

```bash
usdb-node status --watch
usdb-node status --progress-json
```

面板状态为 `WAITING/STARTING/SYNCING/INSTALLING/VERIFYING/IMPORTING/READY/SKIPPED/BLOCKED/FAILED`。
`--progress-json` 输出 `usdb-node-progress:v3`，固定包含 `controller_state` 和五个 component；Snapshot 在导入期间额外包含
`stage/stage_index/stage_count/stage_current/stage_total/updated_at_unix`。这是观测接口，不可代替下述
`usdb-node-status:v2` 生命周期判断。

`snapshot-loader` 将导入观测值以原子替换方式写入
`<BH_DATA_HOST_DIR>/bootstrap/snapshot-loader.progress.json`，写入失败只记录 warning，不影响 installer
原有校验与原子切换。每次 loader 启动先清理旧进度；面板还要求进度中的 snapshot file 与当前 `node.env`
选择一致。该文件即使显示 `complete` 也不能代替 `snapshot-loader.done.json` 完成 marker。

`usdb-node status` 查询的是完整节点生命周期，而不只是已启动服务的 readiness。它先检查 release kit、私有
配置、release activation、数据契约和 snapshot 安装状态。Bitcoin 容器已经 running、但仍处于 IBD/txindex
同步时，状态是 `STARTING`，并附带 Bitcoin block/header、verification、txindex 和 peer 进度；达到分层
data-start/origin gate 后，balance-history 和 usdb-indexer 可先后进入 `SYNCING`，不会把正常流水线追块误报为
`DEGRADED`。USDB chain 仍只在所有最终 readiness 通过后启动。

生命周期状态及主要处置如下：

| 状态 | 含义 | 典型 `next_actions` |
| --- | --- | --- |
| `UNCONFIGURED` | 尚无 private `node.env` | `usdb-node setup` |
| `ACTIVATION_REQUIRED` | 配置仍引用同 bundle 的旧 release image | `usdb-node up --activate-release` |
| `SNAPSHOT_INCOMPLETE` | 已选择 snapshot，但下载或校验尚未完成 | `usdb-node up` |
| `READY_TO_START` | 本地安装和数据前置条件完成，容器未启动 | `usdb-node up` |
| `STARTING` | 核心容器正在创建、启动或等待 health | `usdb-node up` / `logs` |
| `READY` | 核心容器和服务 readiness 全部通过 | 无 |
| `DEGRADED` | 已启动服务退出、不健康或 readiness 失败 | `usdb-node logs` |
| `BLOCKED` | 配置、数据契约或状态读取异常 | `usdb-node doctor` |

节点已配置但 unit 尚未安装时，以上可自动推进状态的 `next_actions` 会先列出
`usdb-node controller install`；controller 是否安装是运维能力检查，不改变已经运行节点的共识 readiness，
因此一个已完整启动的节点仍可报告 `READY`。

自动化使用结构化输出：

```bash
usdb-node status --json
```

输出 schema 为 `usdb-node-status:v2`，包含 `release_id`、`network_bundle_id`、`overall_state`、分层
`checks`、`up.mode`、有序 `next_actions` 和 `operator_guidance`。只有 `READY` 返回退出码 `0`，其他状态
返回 `1`；调用方应读取 `overall_state`，不要从人类可读文本或底层连接错误推断安装阶段。

### 4.1 受限恢复

systemd controller 安装、snapshot 下载或服务启动前，可先预览允许的状态转换，再提交后台任务：

```bash
usdb-node up --dry-run
usdb-node up
```

`up` 是幂等的 controller 提交和观察前端，不是通用修复器。全新配置、安装中断、主机重启和已达 `READY` 都
使用同一命令。它只按内部状态枚举执行以下白名单转换，不会执行
`next_actions` 中的任意字符串：

- `SNAPSHOT_INCOMPLETE`：继续 release-approved snapshot 的 `.part/.ranges.json` 下载、签名与哈希校验；
- `READY_TO_START`、`STARTING`：重入幂等的 readiness-ordered `up`；
- `ACTIVATION_REQUIRED`：默认停止，只有 `--activate-release` 才允许同 compatibility contract 的 image 激活；
- `READY`：不启动 controller、不做修改并以退出码 `0` 成功退出。

systemd unit 名为 `usdb-node-bootstrap-<bundle-id>.service`，随主机启动启用。controller 到达 `READY` 后正常
退出，长期服务由 Docker `restart: unless-stopped` 维持；尚未完成时，终端退出不影响它继续工作。controller
进程异常退出时 systemd 退避重启；遇到 activation、degraded、blocked、状态不前进等人工边界则使用专用退出码
停止重试，避免循环修改现场。一次 controller 运行最多执行四个不同转换；相同动作执行后状态没有前进时主动
停止。bundle-scoped `flock` 防止配置、activation、前台启动和 controller 并发执行，进程退出后由内核释放，
不应手工删除锁文件。

`setup` 中选择 snapshot 后，选择意图先写入 `node.env`，实际下载由 controller 执行。因此配置完成后即使 SSH
断开，后续仍保持 `SNAPSHOT_INCOMPLETE` 并可安全续传，不会误启 full sync。

自动化接口为：

```bash
usdb-node up --dry-run --json
usdb-node up --json
```

输出 schema 为 `usdb-node-up:v1`。JSON 模式把底层同步日志定向到 stderr，只在 stdout 输出一个结果对象。
后台 JSON 模式在成功提交 systemd 后立即返回 `controller_started`。`--skip-pull` 和
`--sync-timeout-secs` 是 `controller install` 或显式 `--foreground` 的高级覆盖项，不能临时改变已安装 unit。

`UNCONFIGURED`、`DEGRADED`、`BLOCKED` 不自动处理。状态输出会保留第一个 invalid/unavailable check，并给出
人工处置建议：先保存 `node.env`、artifact、持久数据和服务日志；再运行 `doctor` 或对应 `logs`；不要通过覆盖
配置、删除最终 snapshot 目录、移动 dataset marker 或循环重启来绕过身份、哈希、storage 或 consensus 错误。
`DEGRADED` 尤其要求先确认退出服务及 readiness 失败原因。若另一个操作仍持有锁，等待原命令结束或确认其进程
已退出后再重试，不能删除锁文件来绕过仍运行的操作。

Ctrl+C 只退出 `up` 的前端进度显示；controller 与容器继续运行。明确执行
`usdb-node controller stop` 只停止后续编排，也不停止已经 detached 的 Docker 服务；停止整个 runtime 仍使用
`usdb-node down --keep-bitcoin`，停止整个节点使用默认的 `usdb-node down`。controller 及其 journal 使用：

```bash
usdb-node controller status
usdb-node controller logs --follow
usdb-node controller stop
usdb-node controller disable
```

`stop` 只结束本次 controller 运行，unit 仍会在下次开机启动；`disable` 同时停止并取消开机启动。两者都不执行
Compose down，也不删除容器、journal、配置或数据。重新运行 `controller install` 会再次启用 unit。

主机重启时，enabled controller 会在 Docker 和 network-online 后重新读取状态并继续未完成阶段。节点已经
`READY` 时再次执行 `up` 只复核状态并成功退出，不会重启容器。此时 systemd controller 显示
`inactive (dead)` 是正常完成，不表示 Docker 服务停止；只有节点尚未 READY 且 controller 为 `failed` 时才需要
检查 journal。

常用命令：

```bash
usdb-node status
usdb-node status --watch
usdb-node up --dry-run
usdb-node up
usdb-node controller status
usdb-node controller logs --follow
usdb-node logs balance-history
usdb-node logs usdb-chain
usdb-node logs --bitcoin
usdb-node down
usdb-node down --keep-bitcoin
```

需要独立采集 Bitcoin 当前门禁进度时，可在 release node kit 环境中执行
`run_testnet_bitcoin.sh progress`；该命令只查询一次并输出 `usdb-bitcoin-readiness:v1` JSON，不等待同步完成。

默认 `down` 停止 controller、USDB runtime 和独立 Bitcoin Core；`--keep-bitcoin` 明确保留 Bitcoin 继续同步。
所有命令都不会执行 `compose down -v`，也不会删除持久数据。

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
- 是否采用 release-approved snapshot，以及 paired checkpoint 恢复选择。

SourceDAO bootstrap 保持独立，是因为 node kit 不应接触管理员私钥。未来可以增加一个只生成待签交易和
验收报告的子命令，但不能把 signer secret 放入 Compose environment。

## 7. 当前验证边界

本批次覆盖：

- release manifest 与 bundled network identity 交叉校验；
- image digest、路径、RPC credential 和 role 配置生成；
- 配置拒绝覆盖和 role 原子更新；
- Bitcoin/runtime 启动调用顺序；
- installer checksum、release identity 和幂等安装。
- release-approved snapshot binding、setup 选择、断点 staging、原子安装、中断阻断和已有 DB 拒绝。

仍需在新 release ID 上完成 GitHub workflow 产物检查，并在空白目标机执行一次从 installer、可选
R2 snapshot 到三类 image、Bitcoin IBD 断点续跑和完整 runtime readiness 的跨进程 E2E。
