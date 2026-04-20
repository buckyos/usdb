# USDB Docker 部署说明

这个目录承载了 `usdb` 当前的 Docker 化部署骨架。  
建议把本文件作为理解整套本地/开发/冷启动机制的总入口。

当前这套 Docker 体系已经覆盖：

- `joiner`：基础加入节点运行形态
- `dev-sim`：本地单机开发栈
- `world-sim`：在 `dev-sim` 之上叠加 BTC 自动产块与协议动作模拟
- `bootstrap`：ETHW 冷启动与 SourceDAO bootstrap
- `usdb-control-plane`：统一控制台服务
- `balance-history` 快照恢复

当前仍然属于开发期部署体系，重点是：

- 本地 bring-up
- 本地模拟
- 冷启动编排
- 控制台观测

而不是正式发布包的最终形态。

## 1. 目录结构

当前 `docker/` 目录中最重要的内容如下：

- `Dockerfile.usdb-services`
  - 构建 `balance-history`、`usdb-indexer`、`usdb-control-plane`
- `Dockerfile.world-sim-bitcoin`
  - world-sim 使用的 Bitcoin Core 28.x 运行镜像
- `Dockerfile.world-sim-tools`
  - world-sim 使用的 `ord + bitcoin-cli + simulator` 工具镜像
- `compose.base.yml`
  - 所有主要服务的基础定义
- `compose.joiner.yml`
  - joiner 模式 overlay
- `compose.dev-sim.yml`
  - 本地 regtest / dev-sim overlay
- `compose.world-sim.yml`
  - 在 `dev-sim` 上叠加 `ord + world-sim` 的 overlay
- `compose.bootstrap.yml`
  - 冷启动 bootstrap overlay
- `env/*.env.example`
  - 各运行模式的环境变量模板
- `scripts/`
  - 各类启动器、渲染器、helper 脚本
- `local/`
  - 本地运行文件目录，默认不进 git

## 2. 整体分层

可以把这套 Docker 体系理解成四层。

### 2.1 基础运行层

由 [compose.base.yml](/home/bucky/work/usdb/docker/compose.base.yml) 提供：

- `btc-node`
- `snapshot-loader`
- `balance-history`
- `usdb-indexer`
- `ethw-node`
- `usdb-control-plane`

这是所有模式的公共底座。

### 2.2 模式层

不同 overlay 在基础层之上追加不同语义：

- [compose.joiner.yml](/home/bucky/work/usdb/docker/compose.joiner.yml)
  - 面向 joiner / 普通节点接入
- [compose.dev-sim.yml](/home/bucky/work/usdb/docker/compose.dev-sim.yml)
  - 面向本地 regtest 开发
- [compose.world-sim.yml](/home/bucky/work/usdb/docker/compose.world-sim.yml)
  - 面向 BTC 自动模拟
- [compose.bootstrap.yml](/home/bucky/work/usdb/docker/compose.bootstrap.yml)
  - 面向 ETHW cold-start 与 SourceDAO bootstrap

### 2.3 编排辅助层

由 `docker/scripts/*.sh` 提供：

- 渲染配置
- one-shot init
- world-sim 启动
- 控制台预览
- SourceDAO bootstrap helper

### 2.4 本地输入层

由 `docker/local/` 提供：

- 本地 env
- 本地 genesis / manifest / snapshot
- 本地 bootstrap config
- 本地 keys

## 3. 各 compose 文件作用

### 3.1 `compose.base.yml`

作用：定义整套服务的**基础形态**。

主要服务：

- `btc-node`
  - 默认使用 `ruimarinho/bitcoin-core:latest`
- `snapshot-loader`
  - `balance-history` 快照安装 one-shot
- `balance-history`
  - BTC 余额历史索引服务
- `usdb-indexer`
  - 矿工证与协议索引服务
- `ethw-node`
  - ETHW / geth 节点
- `usdb-control-plane`
  - 本地统一控制台 API + Web 托管

特点：

- 只描述基础依赖关系
- 不带明确的开发/模拟/冷启动语义
- 端口、数据路径、RPC 地址都通过 env 控制

### 3.2 `compose.joiner.yml`

作用：定义 joiner 模式的 overlay。

主要特点：

- 默认面向加入网络场景
- 保持 `INSCRIPTION_SOURCE=bitcoind`
- 不引入 `ord`
- 只是在基础服务上覆写网络和端口语义

适用场景：

- 本地模拟“加入节点”
- 不需要 world-sim
- 不需要 SourceDAO cold-start

### 3.3 `compose.dev-sim.yml`

作用：定义本地 `regtest` 开发栈。

主要特点：

- `btc-node` 开启 `-regtest=1`
- 各服务默认端口切到开发期端口：
  - `btc-node` RPC `28132`
  - `balance-history` RPC `28110`
  - `usdb-indexer` RPC `28120`
  - `control-plane` `28140`
- `USDB_GENESIS_BLOCK_HEIGHT` 默认是 `1`

适用场景：

- 本地开发
- 控制台页面联调
- 不带 world-sim 的基础 regtest

### 3.4 `compose.world-sim.yml`

作用：在 `dev-sim` 上叠加 BTC 自动模拟能力。

新增服务：

- `ord-server`
- `world-sim-bootstrap`
- `world-sim-runner`

主要特点：

- 默认切到 `BTC_AUTH_MODE=userpass`
- 使用 world-sim 专用镜像：
  - `WORLD_SIM_BITCOIN_IMAGE`
  - `WORLD_SIM_TOOLS_IMAGE`
- 支持：
  - deterministic identity
  - seeded reset
  - bootstrap-once + loop-runner
  - ord 索引 + inscription 模拟

适用场景：

- 本地持续模拟 BTC 协议行为
- 控制台里观察协议数据变化
- 后续接钱包前的自动演示环境

### 3.5 `compose.bootstrap.yml`

作用：定义冷启动 bootstrap overlay。

新增服务：

- `bootstrap-init`
- `ethw-init`
- `sourcedao-bootstrap`

主要流程：

1. `snapshot-loader`
2. `bootstrap-init`
3. `ethw-init`
4. `ethw-node`
5. `sourcedao-bootstrap`

当前语义：

- 复制/校验 ETHW genesis artifact
- 运行 `geth init`
- 启动 ETHW 节点
- 运行 SourceDAO one-shot bootstrap

适用场景：

- 本地 ETHW cold-start
- 本地完整 SourceDAO 部署验证

## 4. 关键 env 文件

### 4.1 `env/joiner.env.example`

用于 joiner 模式。

重点变量：

- `ETHW_IMAGE`
- `ETHW_COMMAND`
- `BTC_RPC_URL`
- `SNAPSHOT_MODE`

### 4.2 `env/dev-sim.env.example`

用于本地 dev-sim。

重点变量：

- `USDB_SERVICES_IMAGE`
- `ETHW_IMAGE`
- `ETHW_COMMAND`
- `BTC_NETWORK=regtest`

### 4.3 `env/world-sim.env.example`

用于 world-sim。

重点变量：

- `WORLD_SIM_BITCOIN_IMAGE`
- `WORLD_SIM_TOOLS_IMAGE`
- `BTC_AUTH_MODE=userpass`
- `BTC_RPC_USER`
- `BTC_RPC_PASSWORD`
- `WORLD_SIM_STATE_MODE`
- `WORLD_SIM_IDENTITY_SEED`
- `ETHW_SIM_PROTOCOL_ALIGNMENT`
- `ETHW_IDENTITY_MODE`
- `ETHW_IDENTITY_SEED`

### 4.4 `env/bootstrap.env.example`

用于 cold-start / SourceDAO bootstrap。

重点变量：

- `ETHW_IMAGE`
- `ETHW_COMMAND`
- `ETHW_INIT_COMMAND`
- `ETHW_BOOTSTRAP_TRUST_MODE`
- `ETHW_BOOTSTRAP_GENESIS_INPUT_FILE`
- `SOURCE_DAO_CONFIG_INPUT_FILE`
- `SOURCE_DAO_BOOTSTRAP_MODE`
- `SOURCE_DAO_BOOTSTRAP_SCOPE`
- `SOURCE_DAO_BOOTSTRAP_PREPARE`
- `SOURCE_DAO_REPO_HOST_DIR`

## 5. 关键脚本说明

### 5.1 配置渲染脚本

- [render_balance_history_config.sh](/home/bucky/work/usdb/docker/scripts/render_balance_history_config.sh)
  - 生成 `balance-history` 的 `config.toml`
- [render_usdb_indexer_config.sh](/home/bucky/work/usdb/docker/scripts/render_usdb_indexer_config.sh)
  - 生成 `usdb-indexer` 的 `config.json`
- [render_control_plane_config.sh](/home/bucky/work/usdb/docker/scripts/render_control_plane_config.sh)
  - 生成 `usdb-control-plane` 配置

### 5.2 基础服务启动脚本

- [start_balance_history.sh](/home/bucky/work/usdb/docker/scripts/start_balance_history.sh)
  - 启动 `balance-history`，并做 snapshot marker gate
- [start_usdb_indexer.sh](/home/bucky/work/usdb/docker/scripts/start_usdb_indexer.sh)
  - 启动 `usdb-indexer`
- [start_control_plane.sh](/home/bucky/work/usdb/docker/scripts/start_control_plane.sh)
  - 启动控制台服务
- [start_ethw_node.sh](/home/bucky/work/usdb/docker/scripts/start_ethw_node.sh)
  - 在 ETHW 已初始化前提下启动节点，并校验 init marker

### 5.3 snapshot / bootstrap 辅助脚本

- [snapshot_loader.sh](/home/bucky/work/usdb/docker/scripts/snapshot_loader.sh)
  - 安装 `balance-history` snapshot
- [snapshot_marker.sh](/home/bucky/work/usdb/docker/scripts/snapshot_marker.sh)
  - snapshot marker 相关逻辑
- [bootstrap_init.sh](/home/bucky/work/usdb/docker/scripts/bootstrap_init.sh)
  - 冷启动输入准备器
- [ethw_init.sh](/home/bucky/work/usdb/docker/scripts/ethw_init.sh)
  - 执行 `geth init`
- [ethw_bootstrap_artifact.sh](/home/bucky/work/usdb/docker/scripts/ethw_bootstrap_artifact.sh)
  - ETHW genesis artifact 校验
- [ethw_init_marker.sh](/home/bucky/work/usdb/docker/scripts/ethw_init_marker.sh)
  - ETHW init marker 读写

### 5.4 world-sim 脚本

- [run_world_sim.sh](/home/bucky/work/usdb/docker/scripts/run_world_sim.sh)
  - world-sim 顶层 helper
- [start_world_sim.sh](/home/bucky/work/usdb/docker/scripts/start_world_sim.sh)
  - world-sim bootstrap / runner 主入口
- [start_world_sim_bitcoind.sh](/home/bucky/work/usdb/docker/scripts/start_world_sim_bitcoind.sh)
  - world-sim 模式下的 bitcoind 启动器
- [start_ord_server.sh](/home/bucky/work/usdb/docker/scripts/start_ord_server.sh)
  - ord server 启动器
- [build_world_sim_release_images.sh](/home/bucky/work/usdb/docker/scripts/build_world_sim_release_images.sh)
  - 打包 world-sim 发布镜像

### 5.5 控制台和测试 helper

- [run_console_preview.sh](/home/bucky/work/usdb/docker/scripts/run_console_preview.sh)
  - 最小控制台预览栈
- [run_container_smoke.sh](/home/bucky/work/usdb/docker/scripts/run_container_smoke.sh)
  - 容器级 smoke

### 5.6 SourceDAO bootstrap 脚本

- [start_sourcedao_bootstrap.sh](/home/bucky/work/usdb/docker/scripts/start_sourcedao_bootstrap.sh)
  - 实际执行 SourceDAO one-shot bootstrap
- [run_sourcedao_bootstrap.sh](/home/bucky/work/usdb/docker/scripts/run_sourcedao_bootstrap.sh)
  - 完整本地 ETHW + SourceDAO bootstrap helper

## 6. 本地目录约定

本地运行文件统一放在：

- [docker/local](/home/bucky/work/usdb/docker/local)

推荐结构：

```text
docker/local/
  bootstrap/
    env/
      bootstrap.env
    manifests/
    keys/
    snapshots/
  dev-sim/
    env/
      dev-sim.env
  joiner/
    env/
      joiner.env
  world-sim/
    env/
      world-sim.env
    runtime/
```

说明：

- `env/`
  - 本地真实 `.env`
- `manifests/`
  - genesis、bootstrap config、manifest
- `keys/`
  - trusted public keys
- `snapshots/`
  - snapshot 文件

详细说明见：
- [docker/local/README.md](/home/bucky/work/usdb/docker/local/README.md)

## 7. 常用启动路径

### 7.1 仅看控制台页面

```bash
cd /home/bucky/work/usdb
docker/scripts/run_console_preview.sh up
```

访问：

```text
http://127.0.0.1:28140/
```

### 7.2 基础 dev-sim

```bash
cd /home/bucky/work/usdb
docker compose \
  --env-file docker/local/dev-sim/env/dev-sim.env \
  -f docker/compose.base.yml \
  -f docker/compose.dev-sim.yml \
  up --build
```

### 7.3 BTC world-sim

```bash
cd /home/bucky/work/usdb
docker/scripts/run_world_sim.sh build-images
docker/scripts/run_world_sim.sh up
```

如果要把 ETHW 节点也带上：

```bash
docker/scripts/run_world_sim.sh up-full
```

### 7.4 完整本地 ETHW + SourceDAO bootstrap

推荐直接使用：

```bash
cd /home/bucky/work/usdb
docker/scripts/run_sourcedao_bootstrap.sh build-images
docker/scripts/run_sourcedao_bootstrap.sh up
```

该 helper 会自动：

1. 初始化 `docker/local/bootstrap/env/bootstrap.env`
2. 初始化 `docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json`
3. 生成 `docker/local/bootstrap/manifests/ethw-genesis.json`
4. 启动：
   - `compose.base.yml`
   - `compose.dev-sim.yml`
   - `compose.bootstrap.yml`

查看状态：

```bash
docker/scripts/run_sourcedao_bootstrap.sh ps
docker/scripts/run_sourcedao_bootstrap.sh logs
docker/scripts/run_sourcedao_bootstrap.sh state
```

### 7.5 手工 compose 启动 bootstrap

如果你不想用 helper，也可以手工执行：

```bash
cd /home/bucky/work/usdb

docker compose \
  --env-file docker/local/bootstrap/env/bootstrap.env \
  -f docker/compose.base.yml \
  -f docker/compose.dev-sim.yml \
  -f docker/compose.bootstrap.yml \
  up -d --build
```

## 8. 关键部署机制

### 8.1 `snapshot-loader -> balance-history`

- `snapshot-loader` 是 one-shot
- 成功后写 marker
- `balance-history` 启动前校验 marker

### 8.2 `bootstrap-init -> ethw-init -> ethw-node`

- `bootstrap-init`
  - 准备 `/bootstrap` volume
  - 拷贝 ETHW genesis 与 SourceDAO config
- `ethw-init`
  - 对 datadir 执行 `geth init`
- `ethw-node`
  - 启动前校验 ETHW init marker

### 8.3 `sourcedao-bootstrap`

当前 `sourcedao-bootstrap` 是一个 one-shot job：

- 等待 ETHW RPC ready
- 消费 `/bootstrap/sourcedao-bootstrap-config.json`
- 调用 `SourceDAO` 仓库里的部署脚本
- 产出：
  - `sourcedao-bootstrap-state.json`
  - `sourcedao-bootstrap.done.json`
  - `sourcedao-bootstrap.log`

当前支持的 scope：

- `dao-dividend-only`
- `full`

`full` 模式会继续部署并 wiring：

- `Committee`
- `DevToken`
- `NormalToken`
- `Project`
- `TokenLockup`
- `Acquired`

## 9. 当前前提与限制

### 9.1 ETHW 镜像要求

当前 `ETHW_IMAGE` 至少需要：

- `bash`
- `sha256sum`
- `python3`
- 如果启用 signed manifest，还要有 `openssl`

### 9.2 SourceDAO 现状

当前 Docker bootstrap 仍是**开发期工作区模式**：

- `SOURCE_DAO_BOOTSTRAP_MODE=dev-workspace`
- 依赖本地 `SourceDAO` 工作区
- 依赖 `artifacts-usdb`

还不是最终发布级 artifact bundle 形态。

### 9.3 world-sim 与正式 runtime 的边界

`world-sim` 依赖：

- `ord`
- world-sim tools image
- deterministic seed / runtime state

它是开发和演示环境，不是最终正式网络运行模式。

## 10. 推荐阅读顺序

如果你要快速理解整套机制，推荐按这个顺序看：

1. 本文件
2. [compose.base.yml](/home/bucky/work/usdb/docker/compose.base.yml)
3. [compose.dev-sim.yml](/home/bucky/work/usdb/docker/compose.dev-sim.yml)
4. [compose.bootstrap.yml](/home/bucky/work/usdb/docker/compose.bootstrap.yml)
5. [run_world_sim.sh](/home/bucky/work/usdb/docker/scripts/run_world_sim.sh)
6. [run_sourcedao_bootstrap.sh](/home/bucky/work/usdb/docker/scripts/run_sourcedao_bootstrap.sh)
7. [start_sourcedao_bootstrap.sh](/home/bucky/work/usdb/docker/scripts/start_sourcedao_bootstrap.sh)

再往下看设计文档：

- [usdb-docker-deployment-plan.md](/home/bucky/work/usdb/doc/usdb-docker-deployment-plan.md)
- [dev-sim-world-sim-plan.md](/home/bucky/work/usdb/doc/dev-sim-world-sim-plan.md)
- [sourcedao-bootstrap-job-plan.md](/home/bucky/work/usdb/doc/sourcedao-bootstrap-job-plan.md)
- [full-sim-with-ethw-plan.md](/home/bucky/work/usdb/doc/full-sim-with-ethw-plan.md)
