# 本地 Docker 运行目录

这个目录默认不进入 git，只用于存放本机运行时文件。

它的职责很简单：

- 保存本地实际使用的 `.env`
- 保存本地 genesis / manifest / snapshot / key set
- 给不同运行模式提供各自隔离的输入目录

不建议把大型链数据目录放在这里。  
链数据默认仍应保留在 Docker volume 中，只有在明确需要 bind mount 调试时再单独处理。

## 推荐目录结构

```text
docker/local/
  bootstrap/
    env/
      bootstrap.env
    snapshots/
    keys/
    manifests/
  joiner/
    env/
      joiner.env
    snapshots/
    keys/
    manifests/
  dev-sim/
    env/
      dev-sim.env
    snapshots/
    keys/
    manifests/
  dev-full/
    env/
      dev-full.env
    snapshots/
    keys/
  dev-full-sim/
    env/
      dev-full-sim.env
    bootstrap/
      manifests/
      keys/
      snapshots/
  world-sim/
    env/
      world-sim.env
    runtime/
```

## 各子目录含义

- `env/`
  - 本机真实 `.env`
  - 一般从 `docker/env/*.env.example` 复制而来
- `snapshots/`
  - `balance-history` snapshot DB、manifest、签名文件
- `keys/`
  - 受信 snapshot 公钥集
  - 受信 ETHW genesis manifest 公钥集
- `manifests/`
  - bootnodes manifest
  - ETHW genesis artifact
  - 本地 service manifest
  - 开发期 `ethw-bootstrap-config.json`
  - 开发期 `sourcedao-bootstrap-config.json`

## 四种本地启动入口

### 1. 仅预览控制台页面

使用：

- `docker/local/dev-sim/env/dev-sim.env`

启动命令：

```bash
docker/scripts/tools/run_local_console.sh up
```

它只会启动最小服务子集：

- `btc-node`
- `snapshot-loader`
- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`

适用场景：

- 只看控制台页面
- 不需要 `ord`
- 不需要 `world-sim`
- 不需要 SourceDAO bootstrap

如果 `dev-sim.env` 不存在，helper 会自动从
`docker/env/dev-sim.env.example` 初始化，且不会覆盖已有文件。

### 2. `dev-full` 本地完整开发档位

使用：

- `docker/local/dev-full/env/dev-full.env`

启动命令：

```bash
docker/scripts/tools/run_local_runtime.sh build-images
docker/scripts/tools/run_local_runtime.sh up
```

这个入口的准确定位是：

- **本地 `dev-full` 档位**
- **不是 `joiner`**
- **不是 `world-sim`**
- **也不是 ETHW + SourceDAO bootstrap**

它对应的是：

- `compose.base.yml`
- `compose.dev-sim.yml`
- `compose.ord.yml`

会启动：

- `btc-node`
- `snapshot-loader`
- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`
- `ethw-node`
- `ord-server`

适用场景：

- 本地完整功能联调
- 带 `ord` 的控制台能力基础
- 后续接控制台内 BTC mint 能力

如果你要模拟“普通加入节点”，应该走 `joiner` 路径；  
如果你要自动产块和自动协议动作，应该走 `world-sim`。

### 3. `world-sim` 本地自动模拟

使用：

- `docker/local/world-sim/env/world-sim.env`

启动命令：

```bash
docker/scripts/tools/run_local_world_sim.sh build-images
docker/scripts/tools/run_local_world_sim.sh up
```

如果要把 `ethw-node` 一起带上：

```bash
docker/scripts/tools/run_local_world_sim_ethw.sh up
```

这个入口的定位是：

- 在 `dev-full` 基础上继续叠加自动模拟
- 自动产 BTC 块
- 自动驱动矿工证 / 余额 / inscription 相关协议动作

适用场景：

- 演示
- 本地动态数据观察
- 钱包接入前的自动网络行为模拟

### 4. ETHW + SourceDAO bootstrap

使用：

- `docker/local/bootstrap/env/bootstrap.env`
- `docker/local/bootstrap/manifests/ethw-bootstrap-config.json`
- `docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json`

启动命令：

```bash
docker/scripts/tools/run_local_bootstrap.sh up
```

这个入口的定位是：

- 本地 ETHW cold-start
- SourceDAO one-shot bootstrap
- 验证 `sourcedao-bootstrap` 是否能完整落地

当前 stage-one 的 `sourcedao-bootstrap` 还依赖工作区外部的本地
`SourceDAO` repo。默认 bootstrap env 使用：

- `SOURCE_DAO_REPO_HOST_DIR=../../SourceDAO`

### 5. `dev-full-sim` 完整本地协议模拟

使用：

- `docker/local/dev-full-sim/env/dev-full-sim.env`
- `docker/local/dev-full-sim/bootstrap/manifests/`

启动命令：

```bash
docker/scripts/tools/run_local_full_sim.sh build-images
docker/scripts/tools/run_local_full_sim.sh up
```

这个入口的定位是：

- 本地 **完整开发期协议模拟**
- 在 `dev-full` 基础上继续叠加：
  - `world-sim`
  - `ETHW`
  - `SourceDAO bootstrap`

它对应的是：

- `compose.base.yml`
- `compose.dev-sim.yml`
- `compose.ord.yml`
- `compose.bootstrap.yml`
- `compose.world-sim.yml`

会启动：

- `btc-node`
- `snapshot-loader`
- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`
- `ord-server`
- `ethw-node`
- `bootstrap-init`
- `ethw-init`
- `sourcedao-bootstrap`
- `world-sim-bootstrap`
- `world-sim-runner`

适用场景：

- 本地完整联调
- 观察 BTC + ETHW + SourceDAO 的组合状态
- 控制台上验证完整协议模拟和冷启动结果

## 不建议存放的内容

不要放这里：

- 生产签名私钥
- 正式发布用 canonical manifest
- 大型持久链数据目录

这些内容应有更严格的分发与运维边界。
