# USDB testnet-v0 首节点发布与部署操作手册

## 1. 适用范围

本文面向负责第一个 `usdb-testnet-v0` 节点的发布和运维人员。当前网络是
`development-resettable` 测试网，不是 public mainnet。本文采用：

- BTC mainnet full node；
- balance-history 默认从 BTC 创世全量同步，也可在首次启动前选择已发布 signed snapshot；
- digest-pinned GHCR images；
- SourceDAO bootstrap 由独立运维机执行，私钥不进入节点 Compose。

Snapshot 只是 balance-history 的启动加速器。没有正式 record URL 时保持 `SNAPSHOT_MODE=none`，节点仍会
建立完整 UTXO、balance、block commit 和 state-ref；只有同步耗时不同，最终共识视图不应不同。若使用
snapshot，应在 `setup` 后、首次 `up` 前运行 `usdb-node snapshot install --record-url ...`，不得手工改写
高度或文件路径。

## 2. 当前发布级别

本流程允许进行一次 **release candidate bring-up**。开始前仍必须补齐：

1. 最终 `usdb` commit 已进入 `origin/master`；
2. `go-ethereum/scripts/usdb/ci-revisions.json` 已锁定该 `usdb` commit 和当前 SourceDAO commit；
3. 三仓普通 Fast CI 成功；
4. `usdb` / `go-ethereum` 已在明确 commit 上创建同名 annotated release tag，且两仓
   `USDB Release Build` 成功；
5. `USDB Release Candidate Manifest` 已从 release tag builds 解析并验证 services、chain、Bitcoin Core
   三个 digest-only image reference；
6. 生成并保存跨仓 candidate manifest 与 SHA-256；
7. 确认 testnet 专用 bootstrap admin 私钥能够派生出 bundle 中的
   `0x0b5223FD31cDc1536f31b3627e6D7025b52310c9`，且未使用 development fixture 私钥；
8. 确认至少一个 BTC mainnet Active Standard pass 可作为初始 miner。

第 7、8 项未满足时，可以完成 BTC/balance-history 同步和非矿工节点启动，但不能完成 SourceDAO
bootstrap 或持续出块。

## 3. 机器与网络基线

建议首节点至少具备：

| 资源 | testnet-v0 基线 |
| --- | --- |
| 系统 | Linux kernel 5.10+、x86-64；Ubuntu 24.04 为优先验证基线 |
| CPU | 8 个逻辑核或以上；首轮只要求低难度 Ethash 可持续出块 |
| 内存 | 32 GiB；不要在同机运行额外构建任务 |
| 磁盘 | 至少 2 TiB 可用 NVMe，建议预留扩容空间 |
| 软件 | Docker Engine、Compose plugin、Git、Python 3、curl、jq |
| 时间 | Bitcoin IBD 与 balance-history full sync 均按小时到数天规划 |

防火墙必须公开：

- `31303/TCP+UDP`：USDB devp2p；
- 运维 SSH。

`8333/TCP` 是可选的 Bitcoin 入站 P2P，不是 USDB 运行依赖。testnet-v0 默认把它绑定到
`127.0.0.1`；Bitcoin Core 仍会主动建立出站 peer 并正常同步。只有明确希望该节点为 Bitcoin 网络
提供入站容量时，才改为公网绑定并放行 `8333/TCP`。

`8332`、`8545`、`8546`、`28010`、`28020`、`28040` 必须保持 localhost 或 Docker 私网可见，
不得直接暴露公网。节点需要正常的出站 DNS、NTP、HTTPS 和 Bitcoin P2P。

### 3.1 软件环境自动检查与安装

测试网和正式网共用 [USDB 节点主机软件基线与准备工具](./usdb-node-host-prerequisites.md)。
`prepare_usdb_host.sh check` 是发行版无关的只读检查；自动 `install` 当前覆盖 Ubuntu
22.04/24.04 和 Debian 12/13。

在完全空白、尚未安装 Git/curl 的机器上，先从发布协调机传入与 candidate revision 一致的脚本：

```bash
scp docker/scripts/tools/prepare_usdb_host.sh root@<node-ip>:/tmp/
ssh root@<node-ip> 'chmod 0755 /tmp/prepare_usdb_host.sh'
```

创建独立运行用户并安装软件：

```bash
ssh root@<node-ip>
id usdb >/dev/null 2>&1 || useradd --create-home --shell /bin/bash usdb
/tmp/prepare_usdb_host.sh install --docker-user usdb
```

`docker` 组具有等同 root 的主机权限。只加入专用运维用户，不加入普通业务账户。安装完成后必须
退出 SSH 并重新以 `usdb` 登录，使新组权限生效，然后再次执行只读检查：

```bash
/tmp/prepare_usdb_host.sh check --docker-user usdb
```

归档检查输出，其中包含实际 Docker、Compose、Git、Python、curl 和 jq 版本。若目标机器已有
容器运行时或工作负载，不要直接执行建议的卸载命令，应先人工确认迁移和数据保留方案。
Docker 发布的容器端口可能绕过 UFW 规则；本项目因此同时校验 Compose bind address 和 UFW。
具体操作见 [USDB 节点防火墙与端口暴露操作](./usdb-node-firewall-operations.md)，并在上游云防火墙中
复核同一暴露面。

## 4. 发布协调机流程

### 4.1 冻结 revision 与 release tag

记录三个完整 commit：

```bash
git -C /path/to/usdb rev-parse HEAD
git -C /path/to/go-ethereum rev-parse HEAD
git -C /path/to/SourceDAO rev-parse HEAD
```

所有 worktree 必须 clean。先 push USDB commit并等待 Fast CI，再使用 Go 仓库的协调工具更新
`ci-revisions.json`。工具默认从当前 Go checkout 推导 sibling workspace；非标准布局才需要在子命令
前提供 `--workspace-root`：

```bash
cd /path/to/go-ethereum
python3 scripts/usdb/prepare_release.py sync-lock
python3 scripts/usdb/prepare_release.py sync-lock --commit --push
```

Go 普通 Fast CI 通过后，选择未使用的 release ID；先 dry-run 确认三仓 SHA，再显式创建和 push
两个同名 annotated tag：

```bash
python3 scripts/usdb/prepare_release.py tag --release-id usdb-testnet-v0-r1
python3 scripts/usdb/prepare_release.py tag \
  --release-id usdb-testnet-v0-r1 --create --push
```

tag push 分别触发两个 `USDB Release Build`，重新运行 Fast gate 并构建三张 image。两边成功后从
同名 tag 运行 candidate workflow：

```bash
gh workflow run usdb-release-candidate.yml \
  --repo buckyos/usdb \
  --ref usdb-testnet-v0-r1 \
  -f release_id=usdb-testnet-v0-r1
```

若从 GitHub Actions 页面执行，`Use workflow from` 必须选择同一个 release tag，不能使用默认
`master`；填写 `release_id` 不会自动切换 workflow ref。

不要手工填写 revisions、image digest 或 genesis block hash；workflow 必须从 tag、compatibility lock、
OCI provenance 和 network bundle 派生并校验这些值。candidate review 通过后，再从同名 tag 启动
Environment-protected publish workflow：

```bash
gh workflow run usdb-release-publish.yml \
  --repo buckyos/usdb \
  --ref usdb-testnet-v0-r1 \
  -f release_id=usdb-testnet-v0-r1
```

网页执行 publish 时也必须选择同名 tag。publish 会先重新验证 candidate artifact，再等待
`usdb-release` Environment 审批，通过后才发布 GitHub Release。

`publish` job 必须等待 `usdb-release` Environment required reviewer 批准。workflow 引用 environment
名称并不等于 protection rules 已配置；首次发布前必须在 repository settings 确认 required reviewer、
prevent self-review、禁止 bypass 和 release-tag restrictions。

保存三个 `image@sha256:...`、candidate manifest、manifest checksum、GitHub Release URL、candidate / publish
workflow URL 和运行时间。
首轮建议使用 release ID `usdb-testnet-v0-r1`。

### 4.2 发布前人工输入

在启动节点前记录并双人复核：

- bootstrap admin 地址及其 signer custody；
- active standard pass ID、owner、`usdb_main` recipient；
- miner recipient 私钥/keystore 的保管位置；
- 节点公网 IP、P2P 端口和责任人；
- 当前采用候选 PoW 难度的明确批准记录。

私钥、BTC RPC password 和 `node.env` 不进入 GitHub artifact、聊天记录或工单附件。
development fixture、testnet signer、未来 mainnet signer 必须是三个独立身份；mainnet 不得复用
本 testnet signer。bundle/release record 只登记公开地址和 custody 责任人，不登记私钥材料或助记词。

## 5. 首节点安装

### 5.1 推荐：Release Node Kit

发布完成后不再读取 commit、clone 仓库、填写 asset base URL 或手工复制三个 image digest。每个
release 都有绑定自身 RID 和 SHA-256 的唯一安装脚本：

```bash
bash <(curl -fsSL \
  "https://github.com/buckyos/usdb/releases/download/usdb-testnet-v0-r1/install-usdb-testnet-v0-r1.sh")
export PATH="${HOME}/.local/bin:${PATH}"
```

生成节点私有配置。该命令自动写入 manifest 中的 image digest、创建数据目录、生成 Bitcoin RPC
password/rpcauth，并保持 operator RPC 只监听 loopback：

```bash
usdb-node prepare-host
usdb-node setup
usdb-node doctor
```

`prepare-host` 统一包装主机软件检查，并只在失败后询问是否安装；如果安装修改了 docker 用户组，退出后重新
登录再继续。`doctor` 是可选的显式预检：它会执行完整主机、release/network identity、节点配置、image
digest 和 UFW 只读检查，不会拉取 image、启动服务或修改配置。读取 UFW 状态可能请求 sudo。后面的
`usdb-node up` 会自动再次执行同一检查。节点运行后的服务状态使用 `usdb-node status` 查看，`doctor` 不是
常驻监控程序。

向导默认选择 `/home/usdb/.usdb`、full role 和 private Bitcoin P2P。RPC username 自动使用
bundle ID 与 hostname 派生，password 自动生成。SSH server port 从当前 SSH 会话检测并要求确认，随后写入
节点本地配置。只有角色、miner 地址、bootnode、SSH port 无法正确检测和确实需要开放 Bitcoin 入站时才需要
运维修改默认值。专用数据盘可在向导中选择 `/data/usdb`。

`setup` 最后询问是否应用 UFW，默认 `yes`。它会先保留确认的 SSH 端口，再开放 `31303/TCP+UDP`，并按
Bitcoin private/public 选择处理 `8333/TCP`。选择暂不应用时，后续显式执行：

```bash
usdb-node firewall apply --confirm
```

防火墙通过后，一条命令完成拉取镜像和 readiness-ordered 启动：

```bash
usdb-node up
usdb-node status
```

SSH 中断不会删除数据。重新执行 `usdb-node up` 会从 Bitcoin/balance-history 的现有同步状态继续。
SourceDAO bootstrap 仍保持独立，因为 Bootstrap Admin 私钥不能进入 node kit 或 Compose。设计与故障边界见
[Release Node Kit 与简化部署](./usdb-release-node-kit-and-deployment.md)。

首次安装不执行 `usdb-node activate-release`，因为 `setup` 已写入当前 release 的 image digest。只有以后安装
同一 `usdb-testnet-v0` bundle 的新 `rN`、并继续复用现有 `node.env` 时，才按
`activate-release -> doctor -> up -> status` 升级。新的 `vN`、chain ID 或 genesis 不得复用本配置，必须
按新 network bundle 重新执行 `setup` 并遵循对应的数据处置方案。

### 5.2 手工回退路径

以下命令以独立系统用户 `usdb` 执行：

```bash
RELEASE_ID=usdb-testnet-v0-r1
USDB_REVISION=<candidate-manifest-usdb-revision>

install -d -m 0755 "/home/usdb/releases/${RELEASE_ID}"
git clone https://github.com/buckyos/usdb "/home/usdb/releases/${RELEASE_ID}/usdb"
cd "/home/usdb/releases/${RELEASE_ID}/usdb"
git checkout --detach "$USDB_REVISION"
git status --short
```

最后一条命令必须无输出。节点不从 workspace 构建 image。

如果 GHCR package 不是公开读取，先使用只具备 `read:packages` 的 token 登录：

```bash
docker login ghcr.io
```

## 6. 节点私有配置

本节以及第 7 至第 9 节保留为手工回退和故障排查参考。使用 node kit 时，`usdb-node setup`
已经完成本节，`usdb-node up` 会执行后续服务顺序，不要再手工生成第二份 `node.env` 或 RPC secret。

```bash
cd "/home/usdb/releases/${RELEASE_ID}/usdb"
docker/scripts/tools/run_testnet_runtime.sh init-env
chmod 600 docker/networks/testnet-v0/node.env
```

编辑 `node.env`：

```text
USDB_SERVICES_IMAGE=ghcr.io/buckyos/usdb-services@sha256:<digest>
USDB_CHAIN_IMAGE=ghcr.io/buckyos/usdb-chain@sha256:<digest>
USDB_BITCOIN_IMAGE=ghcr.io/buckyos/usdb-bitcoin-core@sha256:<digest>

USDB_DATA_ROOT=/home/usdb/.usdb
BTC_NODE_DATA_HOST_DIR=/home/usdb/.usdb/bitcoin/mainnet
BH_DATA_HOST_DIR=/home/usdb/.usdb/balance-history
USDB_INDEXER_DATA_HOST_DIR=/home/usdb/.usdb/usdb-indexer
USDB_CHAIN_DATA_HOST_DIR=/home/usdb/.usdb/usdb-chain
CONTROL_PLANE_DATA_HOST_DIR=/home/usdb/.usdb/control-plane

SNAPSHOT_MODE=none
BH_SNAPSHOT_FILE=
BH_SNAPSHOT_MANIFEST=
USDB_NODE_ROLE=full
USDB_OPERATOR_SSH_PORT=<actual-ssh-port>
```

保留默认数据根 `/home/usdb/.usdb`，创建运行目录：

```bash
install -d -m 0700 /home/usdb/.usdb/secure
install -d -m 0700 /home/usdb/.usdb/bitcoin/mainnet
install -d -m 0700 /home/usdb/.usdb/balance-history
install -d -m 0700 /home/usdb/.usdb/usdb-indexer
install -d -m 0700 /home/usdb/.usdb/usdb-chain
install -d -m 0700 /home/usdb/.usdb/control-plane
install -d -m 0755 /home/usdb/.usdb/releases/balance-history
```

生成专用 Bitcoin RPC 凭据：

```bash
docker/scripts/tools/run_testnet_bitcoin.sh init-rpc-auth usdb-testnet-v0-node1
```

把命令只显示一次的 user/password 写入 `node.env`。随后执行：

```bash
docker/scripts/tools/run_testnet_runtime.sh validate
docker/scripts/tools/run_testnet_runtime.sh validate-node
sudo docker/scripts/tools/prepare_usdb_firewall.sh apply \
  --node-env docker/networks/testnet-v0/node.env \
  --ssh-port <actual-ssh-port> \
  --bitcoin-p2p private \
  --confirm
sudo docker/scripts/tools/prepare_usdb_firewall.sh check \
  --node-env docker/networks/testnet-v0/node.env \
  --ssh-port <actual-ssh-port> \
  --bitcoin-p2p private
docker/scripts/tools/run_testnet_bitcoin.sh pull
docker/scripts/tools/run_testnet_runtime.sh pull
```

如果明确需要接受 Bitcoin 入站 peer，先把 `BTC_P2P_BIND_ADDRESS` 改成 `0.0.0.0`，并将上述两个
命令的模式改成 `--bitcoin-p2p public`。不要把 `8332` 与 `8333` 一起开放。

## 7. Bitcoin Core 全量同步

```bash
export BTC_READY_WAIT_TIMEOUT_SECS=604800
docker/scripts/tools/run_testnet_bitcoin.sh up
```

SSH 断开或等待超时不会删除 Bitcoin 数据。重新连接后执行：

```bash
docker/scripts/tools/run_testnet_bitcoin.sh ps
docker/scripts/tools/run_testnet_bitcoin.sh status
docker/scripts/tools/run_testnet_bitcoin.sh wait
```

只有 readiness 同时确认 `chain=main`、`pruned=false`、`initialblockdownload=false`、
`blocks=headers`、txindex 同高度、tip 新鲜且存在 peer，才进入下一阶段。

## 8. Balance-History 从零同步

启动数据层，不启动 USDB chain：

```bash
docker/scripts/tools/run_testnet_runtime.sh up-data
docker/scripts/tools/run_testnet_runtime.sh data-status
```

runtime 会把 Bitcoin 数据目录只读挂载为 `/data/bitcoin`，落后超过 500 块时使用 LocalLoader。
同步数据保存在 `USDB_DATA_ROOT/balance-history` bind mount 中，重启或 `down` 不会删除。

等待完整共识状态：

```bash
docker/scripts/tools/run_testnet_runtime.sh wait-data 604800
```

最终必须满足：

- `service = balance-history`；
- `consensus_ready = true`；
- `stable_height` 接近 BTC tip 减 stable lag；
- `stable_block_hash`、`latest_block_commit` 非空；
- `snapshot_origin` 为空；
- blockers 为空。

## 9. 启动索引器和 USDB 链

首次先保持 `USDB_NODE_ROLE=full`：

```bash
docker/scripts/tools/run_testnet_runtime.sh up
docker/scripts/tools/run_testnet_runtime.sh indexer-status
docker/scripts/tools/run_testnet_runtime.sh ps
```

`up` 会拒绝未 ready 的 balance-history，启动 usdb-indexer 后等待其
`consensus_ready=true`，最后才启动 genesis init、USDB chain 和 control-plane。

检查 chain identity：

```bash
curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_chainId","params":[]}' \
  http://127.0.0.1:8545 | jq

curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"eth_getBlockByNumber","params":["0x0",false]}' \
  http://127.0.0.1:8545 | jq
```

记录 genesis hash、chain ID 和：

```bash
curl -fsS -H 'content-type: application/json' \
  --data '{"jsonrpc":"2.0","id":1,"method":"admin_nodeInfo","params":[]}' \
  http://127.0.0.1:8545 | jq '.result.enode, .result.id'
```

## 10. 开启首个 Miner

先调用 `resolve_miner_candidate` 复核该 `usdb_main` 在最新 external state 下能解析出
`Active + Standard` pass，并记录返回的具体 pass ID、matching count 和 state identity。该 pass
后续 consume/remint 时，只要新 pass 保持同一 `usdb_main`，miner 会自动跟随；改地址则停止组块。

确认 candidate 后切换节点角色：

使用 node kit 时执行：

```bash
usdb-node set-role \
  --role miner \
  --miner-address <stable-usdb-main-address> \
  --miner-threads 1
usdb-node up
usdb-node logs usdb-chain
```

以下是手工回退路径对应的 `node.env` 修改：

```text
USDB_NODE_ROLE=miner
USDB_MINER_ADDRESS=<stable-usdb-main-address>
USDB_MINER_THREADS=1
```

然后重新执行：

```bash
docker/scripts/tools/run_testnet_runtime.sh validate-node
docker/scripts/tools/run_testnet_runtime.sh up
docker/scripts/tools/run_testnet_runtime.sh logs usdb-chain
```

确认区块持续增长、header selector 可由同一 usdb-indexer state-ref 验证后，才执行 SourceDAO bootstrap。
后续节点以 `full -> late joiner -> second miner` 的顺序加入，CPU-only Ethash 的线程限制和验收指标见
[测试网节点角色与 CPU 挖矿方案](./usdb-testnet-node-roles-and-cpu-mining.md)。

## 11. SourceDAO Bootstrap

在独立受控运维机 checkout candidate manifest 固定的 SourceDAO revision，安装 Node.js 24 和依赖：

```bash
cd /path/to/SourceDAO
git checkout --detach <source-dao-revision>
npm ci
npm run test:usdb:compile-and-audit
```

通过 SSH tunnel 访问节点 `127.0.0.1:8545`。使用 bundle 中未经修改的公开配置；配置不携带
机器路径，脚本从当前固定 revision 的 `artifacts-usdb` 读取 artifact：

```bash
SOURCE_DAO_BOOTSTRAP_PRIVATE_KEY="${SOURCE_DAO_BOOTSTRAP_PRIVATE_KEY:?required}" \
  npx tsx scripts/usdb_bootstrap_full.ts \
  --config /path/to/usdb/docker/networks/testnet-v0/artifacts/sourcedao-bootstrap-config.json \
  --rpc-url http://127.0.0.1:8545 \
  --state-file /secure/release/usdb-testnet-v0-r1-sourcedao-state.json
```

脚本会拒绝私钥派生地址与 `bootstrapAdminAddress` 不一致。完成后执行严格只读复检：

```bash
npm run validate:bootstrap -- \
  --config /path/to/usdb/docker/networks/testnet-v0/artifacts/sourcedao-bootstrap-config.json \
  --rpc-url http://127.0.0.1:8545 \
  --state-file /secure/release/usdb-testnet-v0-r1-sourcedao-state.json \
  --output /secure/release/usdb-testnet-v0-r1-sourcedao-validation.json \
  --strict
```

必须在 block `8192` fee gate 前完成 `Dividend.finalizeBootstrap()`。保存每笔交易 hash、完成区块、
state file 和 strict validation report。

## 12. Restart 与故障处理

使用 node kit 安全重启：

```bash
usdb-node down
usdb-node up
```

Bitcoin 独立运行，不会被默认 `down` 停止。不要删除 `USDB_DATA_ROOT` 下的服务目录、Bitcoin 数据目录
或替换 genesis。发现以下任一情况时停止 miner 并保留现场：

- genesis/chain ID 不一致；
- BTC 或 indexer `consensus_ready=false`；
- pass 不再是 candidate；
- SourceDAO strict validation 失败；
- fee gate 已越过但 Dividend 尚未 finalized；
- reward、difficulty、state-ref 或 selector 验证异常。

testnet-v0 默认启用 deep-reorg guard。每个节点在
`<USDB_CHAIN_DATA_DIR>/recovery/deep-btc-reorg/` 保存 epoch 基线；发现 indexer 的 durable epoch 变化后，
写入 `halted.json`、停止 geth，并保持容器处于无 RPC 的 halted 状态。普通 restart 不会删除 latch。

测试网允许重置，但重置必须使用新的 network generation `vN`，记录旧节点最后区块/hash，并由负责人
批准归档旧 `USDB_DATA_ROOT/usdb-chain` 目录。不得把同一 genesis 下的 `rN` 更新当成深重组恢复，也不得直接删除
`halted.json` 后续跑旧链。具体步骤见
[深 BTC 重组停链与整网重置](./usdb-testnet-v0-deep-btc-reorg-operations.md)。

## 13. 首节点验收记录

至少归档：

- candidate manifest 与 SHA-256；
- 三仓 revision、三个 image digest 和 attestation URL；
- `network.json`、genesis SHA-256 和 genesis block hash；
- Bitcoin readiness、balance-history readiness、indexer readiness；
- bootnode enode、外部 IP、P2P 检查结果；
- miner pass/profile、首个区块和连续出块样本；
- SourceDAO state、strict validation 和 bootstrap 交易列表；
- restart 后的相同 identity/readiness 结果。
