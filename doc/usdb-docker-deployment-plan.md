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

## 3.2 `joiner`

用于：

- 后续新矿工节点或全节点加入网络

特点：

- 不再重复冷启动逻辑
- 直接接入已有网络
- 可选使用 `balance-history` 快照加速追上同步

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

## 5. 快照策略

## 5.1 第一阶段只支持 `balance-history`

推荐将快照模式定义成显式配置，例如：

- `SNAPSHOT_MODE=none`
- `SNAPSHOT_MODE=balance-history`

其中：

- `none`：完全从 0 开始同步
- `balance-history`：恢复 `balance-history` 快照后继续同步

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
  compose.base.yml
  compose.bootstrap.yml
  compose.joiner.yml
  compose.dev-sim.yml
  env/
  scripts/
  manifests/
```

含义：

- `compose.base.yml`
  - 公共服务定义
- `compose.bootstrap.yml`
  - 冷启动相关 profile
- `compose.joiner.yml`
  - 普通节点加入
- `compose.dev-sim.yml`
  - 单机开发和模拟
- `env/`
  - 环境变量模板
- `scripts/`
  - 启动、初始化、快照恢复脚本
- `manifests/`
  - 快照和外部服务配置清单

## 9. 当前阶段建议落地顺序

推荐按下面顺序推进：

1. 先建立 `usdb/docker` 基本目录结构
2. 先做 `joiner` 与 `dev-sim` 的最小可运行版本
3. 加入 `balance-history` 快照可选恢复能力
4. 最后再补 `bootstrap` 的正式冷启动流程

原因：

- `joiner` / `dev-sim` 更容易先验证
- `bootstrap` 涉及的初始化链路更复杂，适合后置

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
