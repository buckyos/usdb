# 状态与同步进度

[返回手册首页](../README.md) · [版本与验证范围](../networks/testnet.md#版本与验证范围)

本页的基础命令适用于 r25；新增的 controller 诊断见[后台任务状态](#后台任务状态)，需使用包含该功能的工具版本。屏幕布局、详细进度和错误提示可能随版本变化；判定运行是否正常时，同时看总体状态、具体阶段、最近进度和日志。

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

## 节点就绪后的独立检查

节点 `READY` 不代表矿工已经产块，也不代表 SourceDAO 已完成初始化。按节点职责分别检查：

| 职责 | 检查命令与判断 |
| --- | --- |
| 普通 full 节点 | 保持整体运行与入网正常，本地挖矿 `DISABLED` 是正常状态 |
| 矿工节点 | `usdb-node mining status` 检查挖矿状态；本地产块证据见[CPU 挖矿](mining.md#判断是否已经在挖矿) |
| SourceDAO 初始化执行节点 | `usdb-node sourcedao status` 检查最近任务结果和链上初始化状态；导出及验证单独完成 |
| 加入已有网络的节点 | 需要核对 DAO 时使用 `usdb-node sourcedao check`；本机没有部署任务不代表链上未初始化 |

SourceDAO 的 `check/status` 不发送交易，但可能拉取工具镜像、启动短时检查容器。初始化期间使用[专用进度与恢复指南](../network-admin/sourcedao.md)，不能仅凭主节点的后台 controller 状态判断初始化任务。

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

重启时，包含快照复用优化的版本会查询 Bitcoin 当前基线。如果基线已可用、原始快照文件仍存在且大小匹配，`UTXO snapshot` 会显示 `READY` 和 `Existing snapshot baseline reused; no file rescan needed`，不再重复扫描整个文件。后台历史验证尚未完成不影响这条复用路径。

首次导入 Bitcoin 前仍需完整校验；原文件缺失时仍会下载并校验，以供尚未完成导入的 balance-history 使用。balance-history 在需要导入时独立核对文件 SHA-256 和 UTXO 状态哈希，已完成导入的数据库则验证持久化状态后继续运行。

r27 等旧版会在 `down → up` 后重新校验本地文件，因此短暂出现 `UTXO snapshot VERIFYING`、下游服务 `WAITING`；这本身不表示重新下载、重新导入或丢失同步进度。复用优化需要更新配套节点工具与 Bitcoin 镜像后生效。

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

包含 controller 诊断的新工具会在 `usdb-node status` 中增加 `Controller` 行，并在必要时提供 `Next actions`。r25 尚无这项完整诊断，可以使用 `usdb-node controller status` 和 `usdb-node controller logs --follow` 单独检查。

| `Controller` | 含义与操作 |
| --- | --- |
| `MISSING` | 尚未安装后台编排；使用后台启动前执行 `usdb-node controller install`。有意使用前台模式时按提示运行 `up --foreground` |
| `UPDATE_REQUIRED` | unit 与当前工具生成的配置不一致，或 systemd 尚未加载磁盘上的配置；核对后执行 `Next actions` 中的安装命令 |
| `REVIEW_REQUIRED` | 存在自定义覆盖、屏蔽、账号不匹配或无法识别的配置；先查看 `controller status`，确认自定义内容后处理 |
| `RUNNING` | 正在运行启动或运维编排；查看同步进度或 controller 日志 |
| `STOPPING` | 编排正在停止，等待当前停机操作完成 |
| `IDLE` | 当前没有编排任务；节点已经 `READY` 时可以是正常完成，不必重新安装 |
| `MANUAL_ACTION` | 上次编排以退出码 2 要求人工处理；先看当前节点检查项，`AWAITING_PEERS` 应处理入网条件 |
| `FAILED` | 编排异常退出；按提示先查日志，再决定是否重试 `up` |
| `UNAVAILABLE` | 未取得可靠的 systemd 状态；检查 `controller status`，不能据此断定节点服务失败 |

状态说明还会显示开机启动是否启用。若显示 `automatic startup after reboot is disabled`，手动 `up` 仍可启动，但不会恢复开机启动；仅在希望恢复开机启动时执行提示中的 `controller install`。该操作不会由状态查询自动执行。

**普通 rN 升级不要求每次重装 controller。** 稳定命令入口、配置路径和 unit 模板仍匹配时可以复用；支持的自定义超时和 `--skip-pull` 不会被误判为版本过旧。需要刷新时，提示命令保留这些选项。手工编辑或 systemd 覆盖配置需要自行核对。

`Controller` 是独立的运维检查，不改变核心服务的 `Overall` 或状态命令退出码。因此节点可能同时显示 `Overall READY` 和 controller 需要维护；监控应同时检查 `checks.controller.action_required`。JSON 中还包含 `configuration_state`、`runtime_state`、`autostart`、退出结果及建议操作。

`controller stop` 只停止编排；停止节点用 `usdb-node down`，具体区别见[日常维护](maintenance.md#停止与重新启动)。

## 自动采集

```bash
usdb-node status --json
usdb-node status --progress-json
usdb-node peers status --json
```

`status --json` 用于总体状态和下一步建议；`--progress-json` 用于分组件进度；peers 输出用于实际连接和入网状态。采集系统应允许同步期间返回非零退出码，并将正常等待与明确故障区分开。

输出和日志可能包含本机路径、节点地址等运维信息。对外提交前按[求助信息清单](../troubleshooting/README.md#需要协助时提供什么)检查。
