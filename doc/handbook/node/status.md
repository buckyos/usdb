# 状态与同步进度

[返回手册首页](../README.md) · [版本与验证范围](../networks/testnet.md#版本与验证范围)

本页按 r25 的命令和状态含义说明。屏幕布局、详细进度和错误提示可能随版本变化；判定运行是否正常时，同时看总体状态、具体阶段、最近进度和日志。

## 先看总体状态

```bash
usdb-node status
```

先读 `Overall`，再读失败检查项及 `Next actions`。命令只有在 `READY` 时返回退出码 0；正在同步或等待入网时的非零退出码，不单独代表服务崩溃。

| `Overall` | 含义 | 下一步 |
| --- | --- | --- |
| `UNCONFIGURED` | 当前账号下还没有本网络配置 | 新节点按安装流程配置；已有节点先确认账号和配置路径 |
| `ACTIVATION_REQUIRED` | 新装工具与现有配置中的版本还未完成切换 | 按[升级步骤](maintenance.md#升级节点)处理 |
| `READY_TO_START` | 配置具备启动条件，节点尚未运行 | 执行 `usdb-node up` |
| `STARTING` | 正在启动、同步或等待依赖 | 观察进度；有具体失败时查日志 |
| `AWAITING_PEERS` | 数据服务已运行，入网条件尚未满足 | 查看 `usdb-node peers status` |
| `READY` | 当前节点核心运行和入网检查通过 | 日常观察；不等于本机已挖到区块或公共 Explorer 已可用 |
| `DEGRADED` | 已启动服务不健康 | 先看故障服务日志，再决定恢复操作 |
| `BLOCKED` | 配置、数据、身份或运行环境存在阻断 | 保存错误，执行 `doctor` 查明原因 |
| `SNAPSHOT_INCOMPLETE` | 旧 BH 快照流程尚未完成 | 只按对应旧版本说明恢复；不据此对 AssumeUTXO 节点运行旧快照命令 |

进度面板中的单个组件状态与这里的总体状态不是同一层级。单行 `READY`、下载 100% 或容器显示 `running`，都不代表整个节点就绪。

## 连续观察同步

```bash
usdb-node status --watch
```

这是观察命令，不会启动或停止节点。Ctrl+C 退出面板。原生启动主要经历以下工作，部分工作会重叠进行：

| 面板组件或阶段 | 正在做什么 | 怎样观察 |
| --- | --- | --- |
| `UTXO snapshot`：下载 / 校验 | 准备启动文件 | 看字节进度和当前阶段；下载完成后仍需校验和导入 |
| `UTXO snapshot`：导入 | Bitcoin 导入启动数据 | 看导入状态与 Bitcoin 日志；r25 不一定有详细导入百分比 |
| `Bitcoin` | Bitcoin 前台同步 | 看当前高度、目标高度及连接情况 |
| `Core background history` | Bitcoin 后台验证较早历史 | 单独观察其高度和 `VALIDATED` 状态 |
| `balance-history` | 导入、重放、校验后提供数据服务 | 看阶段和已处理数量；重放完成后仍可能处于校验或等待服务启动 |
| `usdb-indexer` | 等待可查询的上游数据，然后继续索引 | 上游尚未可查询时等待是正常依赖关系 |
| `USDB chain` | 等待上游就绪，再连接并同步 USDB 网络 | 上游完成后看链高度与 peers 状态 |

Bitcoin 前台可以先就绪，后台历史验证继续进行。**总体可用和后台验证完成分别观察**；不要为了消除后台进度而停止验证。

旧 BH snapshot 的 loader/registry 不是原生流程的手工补装步骤。原生面板中相应辅助项显示跳过，并不表示安装缺少组件。

## 看上去没动时如何判断

先看当前阶段是否有可靠的总量。文件校验、数据库打开、落盘或最终校验期间，不一定有百分比或 ETA。阶段进入下一步时，百分比也可能重新计算。

`Process elapsed` 表示进程已经运行多久，等待上游的时间也包括在内。ETA 只用于当前可估算的工作；目标继续增长、速度波动或没有可靠观测时，可能不显示。

如果高度、处理量或最近日志持续变化，继续观察。若长时间没有任何新进度，或同一错误反复出现，查看后台任务和对应服务日志：

```bash
usdb-node controller status
usdb-node controller logs --follow
```

退出日志观察后，可以分别查看：

```bash
usdb-node logs --bitcoin
usdb-node logs balance-history
usdb-node logs usdb-indexer
usdb-node logs usdb-chain
```

这些日志命令会持续跟随输出，每次用 Ctrl+C 退出后再运行下一条。日志中的失败阶段和时间比单个停止变化的百分比更有助于判断问题。

r25 中可能出现缺少详细进度、RPC 暂不可用时状态变化、等待上游却显示无意义高度等情况。不要把仓库中新版面板截图当作 r25 必须具有的显示，也不要仅因显示变化清理数据。见[同步或导入看起来卡住](../troubleshooting/README.md#同步或导入看起来卡住)。

## 后台任务状态

默认安装的 controller 是负责启动和同步编排的后台任务，容器运行由 Docker 维持。

- 节点已经 `READY` 时，controller 显示 `inactive (dead)` 可以是正常完成。
- 等待 peers 时，controller 也可能结束本次编排并保留数据服务运行，接下来补充 Seed 或处理连通性。
- 节点未就绪且 controller 显示 `failed` 时，读取其日志，不仅凭容器是否存在判断成功。
- `controller stop` 只停止编排；停止节点用 `usdb-node down`，具体区别见[日常维护](maintenance.md#停止与重新启动)。

## 自动采集

```bash
usdb-node status --json
usdb-node status --progress-json
usdb-node peers status --json
```

`status --json` 用于总体状态和下一步建议；`--progress-json` 用于分组件进度；peers 输出用于实际连接和入网状态。采集系统应允许同步期间返回非零退出码，并将正常等待与明确故障区分开。

输出和日志可能包含本机路径、节点地址等运维信息。对外提交前按[求助信息清单](../troubleshooting/README.md#需要协助时提供什么)检查。
