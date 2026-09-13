# P7.2：Core 31.1 镜像与快照导入编排

日期：2026-09-13。P7.1 已提交为 `6c3836d`，P7.2 基础编排已提交为 `eb6c8c2`。
后续新增[自有源与签名分发](./balance-history-bitcoin-artifact-distribution.md)，包含显式来源模式、发布工具与独立可信公钥。

## 1. 实现与验收边界

本批接入 Core 31.1 镜像、固定发布签名、原始 UTXO 下载/校验/导入工具，以及独立 Core Compose overlay。
本地镜像构建、禁网临时 Core 容器和 HTTPS/RPC 故障恢复测试已通过。
**本批没有运行主网 935000 快照导入，没有升级现有28.1数据目录，也没有切换 node-kit 默认部署流程或发布镜像。**
完整 BH/indexer/chain/control-plane 编排与资源阶段属于 P7.3，主网长任务继续按 P6.5 与本手册安排。

镜像与候选标签、release candidate resolver 已统一为31.1。旧模式默认 `txindex=1`；显式原生模式默认
`txindex=0`，可通过 `BTC_TXINDEX=1` 保留已有索引需求。两者均 `prune=0`。
原生模式拒绝用 `BTC_EXTRA_ARGS` 改写 chain/datadir/prune/txindex/conf/reindex 等身份或导入关键选项。
旧模式的 readiness 契约保持原状，原生 overlay 使用独立 baseline readiness。

## 2. 固定身份与上游依据

| 项目 | 固定值 |
| --- | --- |
| Bitcoin 版本/架构 | 31.1 / x86_64-linux-gnu |
| 发行归档 SHA-256 | `b80d9c3e04da78fb6f0569685673418cf686fadba9042d926d13fb87ff503f9e` |
| Guix keys revision | `f6a216c90095f5e316318760da6411fe465b7482` |
| 必须全部通过的签名 | achow101、fanquake、hebasto |
| 快照 B | Bitcoin mainnet 935000 |
| 快照字节数 | `9387990306` |
| 快照文件 SHA-256 | `e572ddbe456d254f05fb004cebe225bdb3656074b66f0e9b1c7fa83e1301d486` |

公钥文件 SHA-256 和签名 key fingerprint 固定在 `bitcoin_release.py` 中，由 Dockerfile 调用；fanquake 的发布签名使用其公钥内已固定的签名子密钥。
本轮重新获取官方文件，并实际验证三份 `VALIDSIG`、归档 hash 和镜像构建中的相同校验步骤。
快照网络/B/hash/文件 hash 复用 Rust 检查点目录，不提供外部 commit 或快照 hash 覆盖参数。
快照大小用于下载和容量预检，最终仍验证完整文件 SHA-256，并由 Core 验证内置 UTXO 承诺。

上游依据：[31.1 发布目录](https://bitcoincore.org/bin/bitcoin-core-31.1/)、
[固定公钥目录](https://github.com/bitcoin-core/guix.sigs/tree/f6a216c90095f5e316318760da6411fe465b7482/builder-keys)、
[AssumeUTXO 使用说明](https://github.com/bitcoin/bitcoin/blob/v31.1/doc/assumeutxo.md)、
[loadtxoutset/getchainstates 实现](https://github.com/bitcoin/bitcoin/blob/v31.1/src/rpc/blockchain.cpp)、
[快照激活与后台验证](https://github.com/bitcoin/bitcoin/blob/v31.1/src/validation.cpp)。

## 3. 下载和恢复契约

入口为 `docker/scripts/tools/bitcoin_assumeutxo.py`，提供 `download`、`bootstrap`、`status`。

- `download`：支持已有本地文件或 HTTPS 来源；源 URL 不含凭据、query 或 fragment，重定向仍必须为 HTTPS。
  在 `<snapshot>.download/` 保存身份、进度及 `snapshot.part`；重跑从已保留字节续传。
  验证 Range、响应长度和 encoding，完成后扫描全文件 SHA-256，再原子发布最终文件。
  已存在的错误文件不会自动覆盖或删除；下载身份变化、空间不足或目录锁冲突均明确失败。
- `bootstrap`：首先读取 Core 实际状态。已激活相同 B 的快照链，或已经完整验证并达到 B 的普通链，
  核对 canonical B/hash 后直接复用，不依赖本地成功标记，也不会重复下载或导入。
  若尚未达到该边界，准备文件、等待所需 header，然后调用一次无客户端超时的 `loadtxoutset`。
- `status`：只读 JSON 探针。`bootstrap_ready` 仅表示这个 Core 可以作为基线供块节点，
  不代表 tip 追平、BH 已封存 G、indexer 共识就绪或后台历史验证已完成。
- Core RPC 使用短轮询超时；长导入在独立请求中运行并持续记录阶段/耗时。
  `getrpcinfo.active_commands` 中已有 `loadtxoutset` 时只观察，不重复提交。
  观察超时/进程中断不会取消 Core 的导入，下一次先核对 Core 状态。
- 丢失响应后若 Core 已激活，按真实状态恢复成功；若请求结果未知且没有活动 load，
  保持 `load_uncertain` 并要求检查 Core 后显式传 `--retry-interrupted-load`。
  有明确 RPC 错误时保留错误码并退出，日志不回显上游错误正文或认证信息。

状态 journal 绑定快照身份、文件路径和 RPC 目标摘要。同一目标使用同一 state 目录，目录锁只允许一个工具实例操作；
不要另开手工 `loadtxoutset` 与工具并发。工具不读取或删除 Core 内部 LevelDB/chainstate 文件。

下载预检要求“剩余文件字节 + `--reserve-bytes`”，后者默认1GiB。
这是 artifact 文件系统的局部预检；**不能替代完整节点容量预算**。仍需为原始9.39GB文件、Core 双 chainstate、
全部 blocks/undo、BH staging/正式库、系统和其他服务留足空间。P7.3 再整合各挂载点及并行内存预算。

## 4. 主网长任务：独立目录运行

以下步骤供操作人员安排。它使用独立 Core 目录和网络，保留原节点，采用 `prune=0`。
当前机器已有 `/data/btc/mainnet-935000-utxos.dat`，因此示例复用该文件，不再复制9GB数据。
新机器可在独立 artifact 目录放置文件，或设置匹配固定文件身份的 `BTC_ASSUMEUTXO_SOURCE_URL`。

### 4.1 构建本地候选

```bash
cd /home/bucky/work/usdb
docker build -f docker/Dockerfile.bitcoin-core -t usdb-bitcoin-core:p72-local .
```

本地验收使用上面的临时标签；发布部署必须使用经过交付验收的 digest。构建不会启动现有节点。

### 4.2 准备独立目录和私有配置

```bash
umask 077
P72_ROOT=$(mktemp -d /data/usdb-assumeutxo-validation/p72-core.XXXXXX)
P72_NAME="usdb-p72-${P72_ROOT##*.}"
P72_NAME="${P72_NAME,,}"
mkdir -m 700 "$P72_ROOT/bitcoin" "$P72_ROOT/bootstrap"
cat > "$P72_ROOT/node.env" <<EOF
USDB_BITCOIN_IMAGE=usdb-bitcoin-core:p72-local
BTC_NETWORK=bitcoin
BTC_CONTAINER_UID=$(id -u)
BTC_CONTAINER_GID=$(id -g)
BTC_NODE_DATA_HOST_DIR=$P72_ROOT/bitcoin
BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR=/data/btc
BTC_ASSUMEUTXO_STATE_HOST_DIR=$P72_ROOT/bootstrap
BTC_ASSUMEUTXO_SOURCE_URL=
BTC_RPCAUTH_HOST_FILE=$P72_ROOT/rpcauth
BTC_RPC_USER=
BTC_RPC_PASSWORD=
BTC_MIN_READY_HEIGHT=963800
BTC_MAX_TIP_AGE_SECS=7200
BTC_MIN_CONNECTIONS=1
BTC_P2P_BIND_ADDRESS=127.0.0.1
BTC_P2P_BIND_PORT=28333
BTC_MEMORY_LIMIT=5g
BTC_MEMORY_SWAP_LIMIT=5g
BTC_DBCACHE_MB=3072
USDB_DOCKER_NETWORK=$P72_NAME
EOF
python3 docker/scripts/tools/generate_bitcoin_rpcauth.py \
  --username usdb-p72 --output "$P72_ROOT/rpcauth" --node-env "$P72_ROOT/node.env"
```

记录 `P72_ROOT` 路径。确认28333未被占用；如需修改，仅改该独立配置。
artifact目录/已有文件须能被配置的 UID 读取，下载时其目录还须可写。
此处只启动 Core 与轻量观察器，不能把该 Core 内存配置直接叠加到 BH 并行启动资源策略。

### 4.3 启动与观察

```bash
docker network create "$P72_NAME"
P72_COMPOSE=(docker compose --project-name "$P72_NAME" \
  --env-file "$P72_ROOT/node.env" \
  -f docker/compose.bitcoin.yml -f docker/compose.bitcoin-assumeutxo.yml)
"${P72_COMPOSE[@]}" up -d btc-node btc-snapshot-bootstrap
"${P72_COMPOSE[@]}" logs -f --tail 100 btc-snapshot-bootstrap btc-node
```

Core 先同步 headers，同时观察器校验/下载快照；header就绪后开始加载。
日志里的文件扫描/下载有实际字节数，Core加载过程使用Core自身日志；`getchainstates`没有已导入Coin数的百分比接口，工具不伪造它。
新终端需恢复 `P72_ROOT`、`P72_NAME` 和 `P72_COMPOSE` 数组。文件和日志均可能长期增长，按目标全块容量配置磁盘。

只读检查并归档：

```bash
"${P72_COMPOSE[@]}" exec -T btc-node python3 \
  /opt/usdb/docker/scripts/tools/bitcoin_assumeutxo.py status \
  > "$P72_ROOT/core-status.json"
"${P72_COMPOSE[@]}" ps -a
```

未达到 B 时 `status` 返回非零，JSON仍说明当前状态；`bootstrap_ready=true` 时返回0。
核对 `snapshot_active`、`active_height`、`background_height`、`history_validated`；
在快照已激活、后台尚未追至 B 时，ready可以为true而history_validated为false。
每个快照节点仍运行官方后台历史验证；后台完成并清理后，重启可能只剩一个普通、已验证 chainstate。

### 4.4 中断与重跑

下载或文件校验失败，排除原因后重新运行观察器：

```bash
"${P72_COMPOSE[@]}" run --rm --no-deps btc-snapshot-bootstrap
```

若显示请求结果未知，先检查 Core 日志、`getchainstates` 与 `getrpcinfo`。
已有活动load则继续观察；若确认前次load已停止且未激活，才使用：

```bash
"${P72_COMPOSE[@]}" run --rm --no-deps btc-snapshot-bootstrap --retry-interrupted-load
```

工具不会通过超时自动发第二次load，也不会删除损坏或半成品Core目录。Core自身无法启动时，先按其具体错误恢复。
成功后重跑仍会校验真实 chainstate；即使删除/丢失观察器成功记录，也不能迫使它对相同快照再次导入。

需要停止本次实验时，先停观察器，再让 Core 正常退出：

```bash
"${P72_COMPOSE[@]}" stop btc-snapshot-bootstrap
"${P72_COMPOSE[@]}" exec -T btc-node bitcoin-cli -datadir=/data/bitcoin stop
"${P72_COMPOSE[@]}" wait btc-node
```

该命令保留数据和配置。不要在BH完成原生导入前删除原始UTXO文件。
若复用已完整验证的Core但准备建立全新BH库，先单独运行工具的 `download` 命令准备文件；
`bootstrap` 的Core复用分支不会为另一个服务自动补下载。

## 5. 本批复验与后续

```bash
PYTHONDONTWRITEBYTECODE=1 python3 tests/test_bitcoin_assumeutxo.py -v
PYTHONDONTWRITEBYTECODE=1 python3 tests/run_bitcoin_assumeutxo_container.py --image usdb-bitcoin-core:p72-local
```

14项HTTPS/RPC集成测试通过。真实容器验收覆盖31.1版本、镜像工具hash、Compose渲染、原生/旧入口参数、
禁网Core实际启动、无txindex、genesis阶段不误报ready及正常退出。
另有候选解析14项、旧部署14项、旧readiness17项回归通过；共享Range测试帮助器改动同步运行既有下载回归。
本轮还通过既有Range下载8项、P7.1入口8项及资源策略12项，共87项Python测试。
完整日志保留于 `/tmp/usdb-p72-*.log`，真实容器报告位于本次输出的独立run目录。
P7.2 基础编排提交前的容器报告：`/tmp/usdb-p72-container-v80qwzwm/result.json`，状态 `pass`；
本地 image ID 为 `sha256:a8a067126c4c863f72759b04e8c42d161f8fa1b4d99cc181067cd902234372a0`，
镜像内 bootstrap 工具 SHA-256 为 `58dabf0a87bfb2216a47483b372fa6f362107983dca347e55679f193848e82ec`。
该 image ID 是本地构建身份，不能冒充已经发布的 registry digest。
真实容器使用主网genesis且禁网，**没有把模拟RPC的成功load当成真实主网快照加载证据**。

主网长任务完成后归档镜像ID/digest、快照身份、Core激活/前后台状态、加载耗时与资源数据，
再接 P6.5 的原生BH同锚点验收。下一步 P7.3 负责让 node-kit/controller 按这些状态启动BH/indexer及下游服务。
