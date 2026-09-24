# 节点观测契约与严重事故

[返回状态说明](status.md) · [维护与告警](maintenance.md#持续观察与告警)

本页描述新增的 `usdb-node-observation:v1`。它随包含此功能的 node kit 发布；
控制台展示还需要配套的 `usdb-services` 镜像。已运行的旧版 console monitor
需要按正常升级流程更新，单独刷新网页不会更新宿主机采集代码。

这一阶段提供结构化证据和持久事故的只读观测。邮件、webhook、告警窗口、通知重试、
维护静默和外部心跳属于后续告警服务；本阶段没有通知投递或自动修复功能。

## 采集入口

```bash
usdb-node status --progress-json
usdb-node status --json
usdb-node status --watch --details
```

两个 JSON 报告都新增 `observations`，原有顶层 schema 和字段保持兼容。
`--progress-json` 是完整组件观测入口，提供本轮 BH/Indexer readiness 和 Chain RPC 证据。
`--json` 是生命周期入口：只附带该路径已经采集的容器证据，不额外重复服务 RPC；
没有采集的服务字段缺失，`probe_status=not_observed`。不能把它当作完整 readiness 报告。
两者都会独立读取严重事故记录，链未运行或生命周期检查提前返回也不会跳过此检查。

私有控制台的 `node_monitor.report.observations` 使用同一契约，经严格字段白名单再次投影。
不导出原始 RPC 错误、环境变量、RPC 地址、事故文件完整内容或 Docker inspect 环境。
旧报告没有 `observations` 时，不能推断没有事故。

## 可用性和就绪状态分别判断

| 字段 | 语义 |
| --- | --- |
| `schema_version` | 固定为 `usdb-node-observation:v1`；不认识的版本不能作为健康证据 |
| `status` | `available` 表示契约已采集；`not_observed` / `invalid` 表示缺少或不支持该契约 |
| `observed_at` | 本轮观测的 UTC 时间；不是最后成功时间或故障首次发生时间 |
| `services.<id>.probe_status` | readiness / chain-head 探测的可用性，或 `not_observed`；不是容器健康判定 |
| `services.<id>.readiness.status` | `available`、`unavailable` 或 `invalid`；与是否就绪无关 |
| `readiness.observed_at` | 本次服务探测时间，console 再次序列化不会刷新它 |
| `rpc_alive / query_ready / consensus_ready` | 服务明确返回的布尔值；缺字段保持 `null`，不从端口或百分比推断 |
| `blockers` | 原样保留已知枚举；缺字段为 `null`，不认识的原因归为 `UnknownBlocker` |
| `readiness.failure` | 无法观察时为 `READINESS_UNAVAILABLE` / `READINESS_INVALID`；严重程度和恢复方式均为 `unknown` |
| `incidents.status` | 是否成功检查持久事故；`unavailable` 绝不表示已恢复 |

例如 Indexer 已处理到目标高度，但系统状态尚未建立时，进度可能为 `SYNCING 100%`，
对应证据仍是：

```json
{
  "status": "available",
  "rpc_alive": true,
  "query_ready": true,
  "consensus_ready": false,
  "blockers": ["SystemStateMissing"],
  "current": 100,
  "total": 100
}
```

`BlockProcessingPending`、`ReorgRecoveryPending`、`CatchingUp` 等 blocker 只说明阻断原因，
单独出现不能证明不可恢复。该契约还保留持久同步高度、上游稳定高度、历史锚点缺口、
待提交块高度、`upstream_reorg_epoch` 以及服务返回的状态 commitment/hash。
未提供的可选字段为 `null`。

后续规则应跨样本计算持续时间和进度变化，并区分首次同步、依赖等待与已就绪节点退化。
本版没有将 Rust/Go 内部日志的重试次数、最近成功时间或挖矿工作构建失败计数暴露为 RPC；
这些数据不能从采集时间或日志出现次数伪造。

## 容器和链的证据

`services` 使用 `bitcoin`、`balance_history`、`usdb_indexer`、`usdb_chain` 标识。
容器证据位于 `runtime`：`state`、`health`、`exit_code`、`container_id`、
`restart_count`、`oom_killed`、`started_at`、`finished_at`、`details_available`。

Docker 扩展 inspect 失败不会覆盖已有容器状态，缺失的重启/OOM 字段保持 `null`。
`restart_count` 属于当前容器，重建后可能重置，比较时必须同时检查 `container_id`。
`oom_killed` 是 Docker 本次状态中的标志，不是完整历史事件账本；短暂事件仍需后续
Docker 事件采集补齐。Docker 的零时间值不能当作真实退出时间。

Chain 的 `head.number/hash/timestamp` 来自同一个最新块对象，`peer_count` 来自同轮 RPC。
有链头且 `eth_syncing=false` 不证明全网仍在出块。停滞判定需要连续观测或其他节点对照；
同高度换 hash 也需要保留，不能只比较高度。BTC lag 仅用于运维，不改变共识验证规则。

## 严重事故：深度 BTC 重组停机

当前接入的明确事故类型为 `DEEP_REORG_HALTED`。事故来源是 Chain 数据目录下的
`recovery/deep-btc-reorg/halted.json`，由 geth runtime 的深重组 guard 写入。
普通 RPC 超时、同步等待、矿工资格暂不可用或容器 OOM，不会被转换为这个事件。

```json
{
  "status": "available",
  "events": [{
    "event_id": "0123456789abcdef0123456789abcdef",
    "service": "usdb_chain",
    "code": "DEEP_REORG_HALTED",
    "severity": "critical",
    "recovery": "manual_intervention",
    "latched": true,
    "source": "deep_btc_reorg_marker",
    "detected_at": "2026-09-24T01:02:03+00:00",
    "evidence_status": "available",
    "reason": "upstream_reorg_epoch_advanced",
    "baseline_epoch": 2,
    "observed_epoch": 3
  }]
}
```

新 guard 在现有 v1 文件中增加事故 UUID、固定 code、severity 和 recovery，并对文件及
父目录执行 fsync。重复观察和重启不会改写该记录或生成新 ID。
旧 guard 的 v1 记录仍可读取，使用 `legacy-sha256:<文件摘要>` 作为稳定 ID，不迁移原文件。
后续通知去重键应包含节点身份、网络身份和事件 ID，不能只按 code 或旧记录摘要全网去重。

事故文件存在但 JSON 损坏、字段不一致、超出大小限制或不是普通文件时，仍报告持久停机，
`evidence_status=invalid`，未知的发生时间为 `null`；无法取得可靠 ID 时也为 `null`。
这表示需要检查事故证据，不能忽略停机标记。采集器拒绝跟随事故文件软链接或读取 FIFO。
文件权限不足时，可通过缓存的固定版本 Chain 镜像执行有时间限制的只读探测；
不会拉取镜像、修改权限、打开数据库或重启服务。探测仍失败时 `incidents.status=unavailable`。

如果已经确认标记存在，只是无法读取其内容，则保留 critical 事件，
`evidence_status=unavailable`；不会因缺少详情丢掉已知停机证据。

事故存在时 CLI 总体状态为 `BLOCKED`，watch 的 Attention 分组及控制台显示事故。
控制台观测过期后只显示“上次观测”，不能据旧事件或旧 READY 判断当前状态。
`events=[]` 只有在 `status=available` 时才表示该路径本轮没有停机标记；它不是告警恢复事件，
也不能证明数据已经修复。确认、解除、跨重建保留和通知恢复由后续事故处理流程负责。

处理时保留事故文件和日志，联系网络运维核对网络代次、epoch 与链状态。
`up`、重启容器和恢复 RPC 不会解除持久停机；不要通过删除 `halted.json` 绕过保护。

## 验证与发布

Python 契约测试覆盖原因保留、未知字段、脱敏、旧事故兼容、重复读取、不可读/损坏记录、
只读权限回退及 CLI/console 一致性。node-kit 打包清单包含契约模块，快速 CI 执行对应测试。
go-ethereum guard 测试覆盖事故 ID 保持及旧记录；runtime 测试继续覆盖重启锁定和依赖恢复。

更新 node kit 可读取旧 guard 事故；新事故 UUID 需要升级包含该 guard 改动的 Chain 镜像。
按正常发版、升级和回滚流程部署，不直接修改线上事故记录或发布资产。
