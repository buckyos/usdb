# USDB Docker Deployment Plan

## 1. 目标

这份文档用于收口 USDB 系统的 Docker 部署规划。

这里的部署对象不仅包括：

- `balance-history`
- `usdb-indexer`

也包括和 USDB 联动运行所需的关键服务：

- `bitcoind`
- `ethw/geth`

目标是让下面几类场景都能被统一支持：

1. 冷启动部署
2. 后续普通节点加入
3. 单机开发 / 演示 / 联调模拟

## 2. 当前结论

## 2.0 第一阶段已落地范围

当前第一阶段已经落地的是：

- `usdb/docker` 基本目录结构
- `balance-history` / `usdb-indexer` 服务镜像构建入口
- `usdb-control-plane` 聚合服务与统一控制台入口
- `joiner` 模式 compose 骨架
- `dev-sim` 模式 compose 骨架
- `balance-history` 的可选 snapshot-loader
- `balance-history` / `usdb-indexer` 的配置渲染脚本

当前明确还没有落地的是：

- `ord` 容器及其 compose profile
- `usdb-indexer` 快照恢复
- 发布级镜像与签名分发流程
- 全自动 ETHW genesis 生成与正式网 bootstrap 发布流程

因此第一阶段 Docker 重点是：

- 先让新节点加入和单机模拟有统一的可运行入口
- 而不是一次性覆盖全部部署生命周期

同时，冷启动已经补到最小可执行骨架：

- `compose.bootstrap.yml`
- `bootstrap-init` one-shot 容器

当前它的职责是：

- 准备共享 bootstrap 目录
- 接收并复制 canonical ETHW genesis artifact
- 按 trust mode 校验 ETHW genesis manifest
- 可选复制 ETHW / SourceDAO bootstrap 配置
- 记录一份 `bootstrap-manifest.json`
- 驱动独立的 `ethw-init` one-shot 对本地 datadir 执行 `geth init`

它当前还不负责：

- 在容器内生成 ETHW genesis
- 发送链上初始化交易
- 完成正式发布流程

其中和完整 SourceDAO 链上部署相关的动作，后续建议拆成独立的：

- `sourcedao-bootstrap` one-shot job

而不是继续叠加到 `bootstrap-init` 中。

详细设计见：

- `doc/sourcedao-bootstrap-job-plan.md`
- `doc/sourcedao-bootstrap-dev-artifacts-plan.md`
- `doc/usdb-btc-mint-runtime-profiles.md`

当前已经补到第一阶段开发骨架：

- `compose.bootstrap.yml` 中新增 `sourcedao-bootstrap`
- 默认 `SOURCE_DAO_BOOTSTRAP_MODE=disabled`
- 开启 `dev-workspace` 后会复用本地 `SourceDAO` repo 的 `artifacts-usdb`
- 通过 `SourceDAO/scripts/usdb_bootstrap_smoke.ts` 完成 `Dao` / `Dividend` 初始化与验链

因此当前状态是：

- `sourcedao-bootstrap` 已有可运行的开发期 one-shot
- 完整 SourceDAO 其余模块部署仍待后续阶段补齐

统一本地控制台的总体设计见：

- `doc/usdb-control-console-plan.md`

## 2.1 现有 e2e 测试继续与 Docker 分离

当前已经存在的脚本型 e2e：

- BTC regtest / ord / `balance-history` / `usdb-indexer`
- ETHW reward e2e
- SourceDAO bootstrap smoke

这些测试先保持现状，不强行搬进 Docker 编排。

原因：

- 现有 e2e 已经可运行，适合快速调试业务与共识问题
- Docker 更适合解决部署、启动、演示和运维问题
- 把两者混在一起会增加排障复杂度

因此当前原则是：

- **测试脚本继续独立**
- **Docker 先解决部署与运行**

## 2.2 Docker 配置与实现先放在 `usdb/docker`

当前推荐的仓库落点是：

- `usdb/docker`

而不是现在就拆成独立仓库。

原因：

- 当前系统仍处于快速迭代期
- `usdb`、`go-ethereum`、`SourceDAO` 之间还在高频联动
- 如果现在单独拆部署仓库，会过早增加版本联动成本

后续只有在下面这些边界稳定后，再考虑拆独立部署仓库：

- 镜像边界
- 配置格式
- 启动模式
- 快照格式
- 发布节奏

同时建议把本地运行期文件单独收口到：

- `usdb/docker/local`

这个目录用于存放：

- 实际 `.env`
- 快照文件与 sidecar manifest
- 受信公钥集
- 本地 bootnodes / service manifest

原则是：

- `docker/` 里保留模板、脚本、compose
- `docker/local/` 里保留本机实际运行配置
- 容器运行期数据库优先继续使用 Docker volume，而不是提交一套 rootfs

## 2.3 快照是可选能力，不是前置条件

当前快照策略应明确为：

- **节点可以完全不使用快照**
- **节点可以从 0 开始同步**

快照的定位只是：

- 加快节点追上同步
- 缩短新节点加入时间

不是：

- 网络运行的强依赖
- 共识正确性的前提

## 2.4 当前只支持 `balance-history` 快照

当前已有快照能力的是：

- `balance-history`

而不是：

- `usdb-indexer`
- `bitcoind`
- `ethw/geth`

因此 Docker 第一阶段的快照策略应明确为：

- 只支持 `balance-history` 快照恢复
- `usdb-indexer` 快照后续按需要增加
- `bitcoind` 与 `ethw/geth` 暂不纳入额外快照体系

同时，当前 Docker 快照恢复不是只靠“数据目录是否非空”来判断是否完成安装，而是拆成两层：

1. Compose 启动顺序约束
- `snapshot-loader` 是 one-shot init service
- `balance-history` 通过 `depends_on: condition: service_completed_successfully` 等待它成功退出

2. 本地 marker gate
- 当 `SNAPSHOT_MODE=balance-history` 时，`snapshot-loader` 在安装成功后会在共享数据目录写入完成 marker
- `balance-history` 启动前会检查这个 marker
- 没有 marker、或 marker 与当前快照输入不匹配时，`balance-history` 会直接 fail fast，不会绕过快照安装进入运行

这样可以避免残留目录、半安装状态或错误快照被误当成“首次启动已完成”。

## 2.5 `bitcoind` 默认跟随 Docker 一起部署

当前推荐默认模式是：

- Docker 中包含本地 `bitcoind`

同时保留一个可选项：

- 用户也可以配置使用本地已有 BTC 节点
- 或使用 remote BTC RPC

但这个可选项应标记为：

- **支持**
- **但不推荐**

原因：

- remote RPC 延迟更高
- 吞吐更差
- 同步与查询稳定性可能更弱

## 2.6 `bitcoind` 和 `ethw/geth` 暂不引入额外快照管理

当前判断是：

- `bitcoind` 自身区块同步速度较快，暂不需要我们额外管理快照
- `ethw/geth` 也有自己的链数据同步和数据库恢复机制，不宜现在额外引入一套外部快照体系

后续如有必要，可以再单独优化。

## 2.7 除 `sourcedao-bootstrap` 外的剩余任务

当前除 `sourcedao-bootstrap` 之外，Docker 体系还剩下这些主要工作。

### 2.7.1 近期优先项

- ETHW 节点标准启动模板
  - `miner`
  - `full`
  - `joiner`
- `joiner` 入网配置约定
  - bootnodes / static peers / 推荐网络参数
- `usdb-control-plane` 的 Docker 运维说明继续细化
  - 例如 BTC cookie 挂载、bootstrap marker 来源、`sourcedao-bootstrap` 状态接入

### 2.7.2 发布级仍缺的部分

- 发布级镜像 tag / artifact 版本规则
- ETHW genesis artifact 的正式分发约定
- 受信公钥和签名发布流程
- 更完整的 readiness / health / 运维说明

### 2.7.3 后续优化项

- `ord` 的 `dev-sim` profile
- `usdb-indexer` snapshot restore
- development-only `dumpgenesis` 生成模式
- 更完整的多节点 devnet 编排

## 2.8 `world-sim` 作为 `dev-sim` 的可选附加层

当前建议已经明确：

- 不改变默认 `dev-sim`
- 新增一个独立的 `world-sim` overlay
- 用于让本地 regtest 持续自动出块并驱动真实协议动作

这个模式的目标是：

- 让 `usdb-control-plane` 和控制台不再长期停留在“服务可达但几乎没有链上活动”
- 给后续钱包集成、矿工证 mint 演示和协议交互提供一个更接近真实运行的本地环境

第一批实现的方向是：

- 新增 `compose.world-sim.yml`
- 新增 `ord-server`
- 新增 `world-sim-bootstrap`
- 新增 `world-sim-runner`
- 在 `world-sim` overlay 下覆盖 `btc-node`
- 引入 world-sim 专用发布镜像：
  - `usdb-bitcoin28-regtest`
  - `usdb-world-sim-tools`
- 直接复用现有：
  - `regtest_world_simulator.py`
- 并通过专门的本地入口脚本统一完成：
  - 构建镜像
  - 启动 overlay
  - 保留状态的关闭
  - 显式 reset
- 暴露明确的 world-state policy：
  - `WORLD_SIM_STATE_MODE=persistent`
  - `WORLD_SIM_STATE_MODE=reset`
  - `WORLD_SIM_STATE_MODE=seeded-reset`
  - `seeded-reset` 基于 `WORLD_SIM_IDENTITY_SEED` 确定性重建 miner / agent ord wallet identity
- 在 simulation loop 前增加 ord wallet / ord server 稳定性 gate
- `world-sim` / `up-full` 默认改用：
  - `BTC_AUTH_MODE=userpass`
  - 而不是 cookie auth
  - 以降低 fresh bootstrap 和 `-rpcwallet` 路径上的认证脆弱性

这个模式仍然是：

- **可选**
- **开发期专用**
- **不进入默认 joiner / 默认 dev-sim 路径**

详细方案见：

- `doc/dev-sim-world-sim-plan.md`
- `doc/world-sim-release-image-plan.md`
- `doc/world-sim-deterministic-state-plan.md`

## 3. 部署模式

当前建议将 Docker 部署分成三类模式。

## 3.1 `bootstrap`

用于：

- 新链冷启动
- 启动第一批核心节点
- 执行必要初始化

特点：

- 需要初始化链和系统配置
- 可能需要导入 `balance-history` 快照
- 需要执行一次性的 bootstrap 流程

当前第一阶段已经支持最小 bootstrap 入口，但它仍是“输入准备器”，不是完整冷启动自动化器。

当前 ETHW 冷启动链路也拆成两层：

1. canonical artifact consume
- `bootstrap-init` 负责接收 `ethw-genesis.json`
- 如果启用 `manifest` / `signed` trust mode，还会校验 `ethw-genesis.manifest.json`
- 在 `signed` 模式下，还会校验 `ethw-genesis.manifest.sig` 与 trusted keys
- 校验通过后，把产物写入共享 `/bootstrap`

2. local datadir init
- `ethw-init` 是 one-shot init service
- 它负责对共享 `ethw-data` 执行一次 `geth init`
- 成功后会写入本地 marker
- `ethw-node` 启动前要求 marker 与当前 genesis artifact 匹配

这样可以把“canonical genesis 的分发”和“本地节点的数据目录初始化”分成两个独立责任边界。

## 3.2 `joiner`

用于：

- 后续新矿工节点或全节点加入网络

特点：

- 不再重复冷启动逻辑
- 直接接入已有网络
- 可选使用 `balance-history` 快照加速追上同步
- 默认不包含 `ord`
- 默认 `usdb-indexer` 使用 `inscription_source = "bitcoind"`

如果启用 `SNAPSHOT_MODE=balance-history`，joiner 启动链路是：

1. `snapshot-loader` 先执行安装，或命中“现有 DB 与 marker 匹配”的跳过路径
2. `balance-history` 再检查 marker 并启动
3. `usdb-indexer` 最后依赖 `balance-history`

如果不启用快照：

- `snapshot-loader` 直接 no-op 成功退出
- `balance-history` 正常从 0 开始同步

## 3.3 `dev-sim`

用于：

- 单机开发
- 演示
- 跨系统联调

建议范围：

- `bitcoind regtest`
- `ord`
- `balance-history`
- `usdb-indexer`
- `ethw/geth`

这个模式的目标不是替代现有脚本 e2e，而是提供：

- 一条更容易上手的一键式本地运行入口

当前第一阶段的 `dev-sim` 仍保持：

- `bitcoind regtest`
- `balance-history`
- `usdb-indexer`
- `ethw/geth`

但 `usdb-indexer` 仍先使用：

- `inscription_source = "bitcoind"`

`ord` 仍只应视为开发期依赖，后续也只进入 `dev-sim` profile，不进入默认 `joiner`。

## 4. 镜像与编排策略

## 4.1 尽量复用镜像，不区分“冷启动镜像”和“普通镜像”

推荐方式是：

- 同一组镜像
- 不同的 compose profile / 启动脚本 / 环境变量

也就是说：

- **冷启动和普通启动主要是流程差异**
- **不是镜像边界差异**

## 4.2 建议的服务划分

第一阶段可以考虑下面这些容器角色：

- `btc-node`
  - 本地 `bitcoind`
- `balance-history`
- `usdb-indexer`
- `ethw-node`
  - 可通过参数区分 miner / full / bootnode
- `snapshot-loader`
  - one-shot 容器
  - 可选恢复 `balance-history` 快照
- `bootstrap-init`
  - one-shot 容器
  - 用于冷启动期的一次性初始化
- `ethw-init`
  - one-shot 容器
  - 用于 ETHW datadir 的一次性 `geth init`

## 5. 快照策略

## 5.1 第一阶段只支持 `balance-history`

推荐将快照模式定义成显式配置，例如：

- `SNAPSHOT_MODE=none`
- `SNAPSHOT_MODE=balance-history`

其中：

- `none`：完全从 0 开始同步
- `balance-history`：恢复 `balance-history` 快照后继续同步

推荐的状态模型是：

- `SNAPSHOT_MODE=none`
  - `snapshot-loader` 直接成功退出
  - `balance-history` 不要求任何 snapshot marker

- `SNAPSHOT_MODE=balance-history`
  - `snapshot-loader` 必须先成功完成安装，或确认现有 DB 与 marker 匹配
  - 安装成功后写入 `snapshot-loader.done.json`
  - `balance-history` 启动前必须看到合法 marker，否则直接失败

ETHW canonical genesis consume 也建议使用同样的 marker 设计：

- `bootstrap-init` 写共享 bootstrap artifact
- `ethw-init` 初始化本地 datadir 后写 `ethw-init.done.json`
- `ethw-node` 启动前必须看到与当前 genesis artifact 匹配的 marker

## 5.2 快照不建议 baking 到镜像

当前不推荐把快照直接做进镜像。

原因：

- 快照体积大
- 更新频繁
- 会导致镜像膨胀
- 镜像版本与快照版本耦合过深

更合适的方式是：

- 用 volume / bind mount / 远程下载
- 由 `snapshot-loader` 恢复到工作目录

建议 marker 至少记录：

- `snapshot_mode`
- `snapshot_file`
- `snapshot_manifest`
- `installed_at`

这样既能表达“只在首次安装”，也能表达“当前 volume 中安装的是哪一份快照”。

## 6. `bitcoind` 部署策略

## 6.1 默认模式：本地容器化部署

默认推荐：

- `bitcoind` 跟随整套系统一同部署

这样有利于：

- 性能稳定
- 配置可控
- 避免外部依赖不一致

## 6.2 可选模式：外部 BTC RPC

保留支持：

- 本地已有 BTC 节点
- 远程 BTC RPC

但需明确：

- 这只是可选项
- 不作为推荐默认值

## 7. `ethw/geth` 部署策略

`ethw/geth` 当前建议继续使用自身的链数据同步和数据库机制。

暂不额外增加：

- 外部链快照管理
- 自定义 state 快照发布流程

后续如果数据规模和启动成本明显上升，再单独讨论。

## 8. 建议的目录结构

第一阶段建议目录结构如下：

```text
usdb/docker/
  README.md
  Dockerfile.usdb-services
  compose.base.yml
  compose.bootstrap.yml
  compose.joiner.yml
  compose.dev-sim.yml
  env/
    bootstrap.env.example
    joiner.env.example
    dev-sim.env.example
  local/
    README.md
  scripts/
    bootstrap_init.sh
    render_balance_history_config.sh
    render_usdb_indexer_config.sh
    snapshot_loader.sh
    start_balance_history.sh
    start_usdb_indexer.sh
    wait_for_tcp.sh
  manifests/
```

含义：

- `compose.base.yml`
  - 公共服务定义
- `compose.bootstrap.yml`
  - 冷启动 bootstrap overlay
- `compose.joiner.yml`
  - 普通节点加入
- `compose.dev-sim.yml`
  - 单机开发和模拟
- `env/`
  - 环境变量模板
- `local/`
  - gitignored 的本地运行期配置
- `scripts/`
  - 启动、初始化、快照恢复脚本
- `manifests/`
  - 快照和外部服务配置清单

## 9. 当前阶段建议落地顺序

推荐按下面顺序推进：

1. 已完成：建立 `usdb/docker` 基本目录结构
2. 已完成：做 `joiner` 与 `dev-sim` 的最小可运行版本
3. 已完成：加入 `balance-history` 快照可选恢复骨架
4. 已完成：补 `bootstrap-init` 最小冷启动编排
5. 已完成：补 ETHW canonical genesis artifact consume 与 `ethw-init`
6. 已完成：补 signed ETHW genesis manifest 与 trusted-key 验证
7. 已完成：补容器级 smoke，把 `bootstrap-init -> ethw-init -> ethw-node -> balance-history -> usdb-indexer` 串起来实跑
8. 下一阶段：补开发期 `dumpgenesis` 生成模式接入 Docker
9. 下一阶段：补 `ord` profile 和更完整的 dev-sim 编排
10. 下一阶段：补 DAO / Dividend 初始化 hook
11. 已完成：补 single-node ETHW full-sim 与 deterministic miner identity
12. 已完成：补 ETHW miner identity 与 world-sim miner-pass `eth_main` 对齐

原因：

- `joiner` / `dev-sim` 更容易先验证
- `bootstrap` 涉及的初始化链路更复杂，适合后置

其中 ETHW full-sim 的最小接入设计单独记录在：

- [full-sim-with-ethw-plan.md](/home/bucky/work/usdb/doc/full-sim-with-ethw-plan.md)

## 10. 当前结论总结

当前 Docker 方案的核心结论是：

- 现有 e2e 继续与 Docker 分离
- Docker 配置先放在 `usdb/docker`
- 快照是可选能力，不是前置条件
- 第一阶段只支持 `balance-history` 快照
- `bitcoind` 默认随 Docker 一起本地部署
- 外部 BTC RPC 仅作为可选项
- `bitcoind` 和 `ethw/geth` 暂不引入额外快照体系
- 冷启动、普通启动、单机模拟三种模式应统一规划，但优先从 `joiner` 和 `dev-sim` 落地
