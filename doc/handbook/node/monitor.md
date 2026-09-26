# 单节点核心监控

[节点状态](status.md) · [观测契约](observations.md) · [维护](maintenance.md)

## 已确认的方案与实施顺序

监控是默认启用的单节点核心运维服务，首先用于自有测试网，同时支持外部节点。
每个节点独立保存配置和记录。集群集中管理、外部心跳、主机断电/断网检测不在首期范围。
monitor 不参与共识，不自动修复服务或解除深重组保护。

第一批提供常驻服务、setup/controller/up/down 接入、本地事件库、规则状态、查询命令和控制台展示。
第二批增加 [Webhook/SMTP 通知](notifications.md)、独立持久队列、文件与网页配置；故障持续时按
级别控制通知频率，获得有效恢复证据后自动结束。没有配置通知时，本地事件记录仍独立工作。

以下命令随包含本功能的新 node-kit 提供；现网旧版本不会因文档更新自动获得这些能力。
控制台展示新增事件还需要配套的 `usdb-services` 镜像。

## 职责和生命周期

controller 编排启动和同步阶段；monitor 持续观察服务并记录事件；control-plane 展示
状态与历史。monitor 由宿主机 systemd 管理，独立于控制台和 controller 的进程生命周期。
原 console observer 的采集和安全投影继续复用，由核心 monitor 统一调度，避免双重采集。

首次 setup 默认启用 monitor，允许关闭；重复 setup 和升级保留用户选择。
controller install 安装服务定义和开机策略，up 实际启动。前台部署可显式运行 monitor。
显式 down 记录计划停机并停止 monitor，再停止业务服务；意外 Docker 故障不级联停止
monitor。升级沿用 down → 安装/激活版本 → 更新服务定义 → up 的流程。
配置、节点身份和事件库位于稳定的私有节点目录，不随 release 目录替换。

up 恢复已有告警并开始新的观测会话。停机、采集缺口及重启空档不算连续故障或恢复时间。
普通规则有启动宽限期，持久严重事故立即呈现。禁用监控时控制台明确显示监控关闭。
监控关闭或停止不删除事件、历史记录或事故，不清除节点的保护状态。

## 持久事件与告警

SQLite 保存稳定节点 ID、版本化元数据、事件历史、当前告警及规则计算状态。
事件与告警变化事务提交，进程重启后可继续查询；通知使用独立持久游标消费这些事件。
通知未配置、投递失败或外部网络异常均不影响本地事件落盘。

事件记录发生/发现时间、服务、稳定代码、严重程度、结构化证据和日志排查线索；未知
源时间不伪造。只保存白名单证据，不复制凭据、原始 RPC 错误、完整环境或完整服务日志。
同一故障重复采样更新最近观察时间/次数，只在首次、升级、关键状态变化和恢复时追加事件。
历史按时间与数量清理，活动事故和告警保留；数据库写入失败必须在 journal 和状态中可见。
数据库版本不兼容时拒绝写入，不能偷偷重建或清空历史；升级前应备份，回退需使用兼容版本。

告警经过 pending → firing → resolved，unknown 不代表恢复。持续故障按严重级别定期通知，
无须 ack 或人工消警。严重事故源仍然 latched；monitor 在保护标记已解除、事故源可读且为空、
并连续获得新鲜 READY 观测后自动登记恢复，不修改 halted.json 或链数据。历史记录保留。
每次重复发生的普通告警有独立生命周期。

## 操作入口

```bash
usdb-node monitor status
usdb-node monitor status --json
usdb-node monitor alerts --json
usdb-node monitor events --limit 100
usdb-node monitor events --service usdb_chain --severity critical --json
usdb-node monitor events --since 2026-09-24T00:00:00Z --json
usdb-node monitor events --id EVENT_ID --json
```

查询直接读取本地记录，不依赖业务 RPC。事件详情包含稳定 ID 和结构化证据，`--json`
可用于导出；默认最近 100 条，单次上限 1000 条。`alerts` 包含 pending 和 firing。
事件使用 `usdb-node-event:v1`，包含节点 ID 和稳定网络身份，便于后续统一接收与去重。
控制台按严重程度优先展示最多 32 个活动告警和最近 20 个事件，完整记录通过 CLI 查询。

旧版的 `monitor ack` 和 `monitor resolve` 入口已移除；已有审计历史保留，不影响自动恢复。

采集规则配置修改要求节点和监控已停止；通知配置支持在线修改：

```bash
usdb-node down
usdb-node monitor configure --enabled on --interval-secs 30
usdb-node monitor configure --warning-after-secs 120 --critical-after-secs 600
usdb-node up
```

`--enabled off` 关闭监控并保留记录；首次 `configure --monitor off` 也可关闭。
setup 中的对应问题默认开启，重复 setup 按回车保持现值。通知配置使用独立文件或控制台表单，
参见 [通知配置与投递](notifications.md)。

标准 systemd unit 为 `usdb-node-monitor-<bundle-id>.service`。`up` 迁移可识别的旧
`usdb-console-monitor-<bundle-id>.service`：先停止并禁用旧进程，保留其 unit 文件供核查，
再启动核心 monitor。旧 unit 或覆盖配置被用户修改时，不自动覆盖。`controller disable`
同时关闭配套 monitor 的开机启动策略，不改写 `USDB_MONITOR_ENABLED`；手动 up 不重新启用
开机启动，显式 controller install 才恢复默认策略。

前台部署另开终端执行 `usdb-node monitor run`。旧 `usdb-node console monitor` 是同一入口的
兼容别名；单实例锁阻止重复运行。前台进程先用 Ctrl-C 正常退出，再执行 down；如果 down
发现前台 monitor 仍在退出，会提示重试，不在观察仍进行时继续停止业务服务。
`console export` 仍可生成一次性观测，核心 monitor 运行时不会覆盖其控制台快照。

## 事件存储与备份

资源诊断另有独立的周期采样、历史库和查询入口，详见 [资源历史与内存压力排查](resource-history.md)。
它可关联冷启动阶段、容器内存压力、服务进度和探针耗时；服务 RPC 超时不阻断资源历史采样。

当前数据库版本为 `1`，保存在 `node.env` 所在目录的 `monitor/events.sqlite3`，规则配置为
同目录的 `monitor/config.json`；目录权限 0700，文件 0600。数据库绑定稳定网络身份，
同一网络的 release 升级保留节点 ID。不要把整个私有 monitor 目录挂载给网页容器；
控制台只读取既有 `console/node-progress.json` 中受限的告警和最近事件投影。
快照导出失败会记录 `CONSOLE_EXPORT_FAILED` 并继续本地采集；控制台恢复后记录恢复事件。

完整备份先执行 down 并确认 monitor 已停止，再备份整个 monitor 目录（包括存在的
SQLite WAL/SHM 文件）。数据库损坏、权限不符或版本不兼容时会明确失败，不自动重建。
磁盘故障时无法保证继续落盘；应保留数据库并检查 monitor 的 systemd journal。

## 首期规则原则

默认每 30 秒采集，普通服务探测整轮上限 25 秒；独立事故检查另有最多 5 秒预算。
普通服务启动宽限 120 秒，异常连续 120 秒 warning、600 秒 critical，恢复需连续 60 秒
有效证据。同步停滞在确认上游推进后连续 900 秒 warning、1800 秒 critical。
历史默认保留 90 天、最近 20000 个普通事件，活动告警/事故关联事件不受此清理上限影响。
规则参数可通过 `monitor configure --help` 查看。规则策略在每次启动时记录到本地事件中。
critical 表示影响严重；latched/manual_intervention 描述事故源的保护状态，修复后监控自动确认恢复。

- `DEEP_REORG_HALTED`：首次有效观测立即 critical，证据损坏但已知标记存在仍保留事故。
- 采集失败、证据无法读取：监控能力异常，与服务故障分别记录。
- 服务异常退出、容器不健康、RPC 持续不可用：按启动阶段及连续时间判定。
- 共识就绪退化：已观察到就绪后持续未就绪；正常首次同步不按未就绪直接告警。
- 同步停滞：仅在上游实际推进而本地提交不推进时成立，重组、进度变化及采集缺口重置窗口。
- 容器 OOM 和重启：保存实际观察到的证据；容器 ID 改变后重新建立计数基线。

可选组件关闭属于预期状态；controller 正常完成不算退出故障。单节点链头不推进不能证明
整个网络停摆。首期只使用已存在的观测证据，不伪造内部重试次数或短暂 Docker 事件历史。

## 验收要求

使用临时节点目录、假 systemd/RPC 和可控时间验证：重启后事件和事故保留；重复采样
不刷屏；未知/过期证据不解除告警；计划停机无误报；禁用选择升级后保留；旧 observer
安全迁移且不会双运行；采集超时不阻塞事件查询和退出；数据库错误不报告记录正常；
CLI/控制台快照不泄露敏感信息；node-kit 打包和完整升级入口可用。
