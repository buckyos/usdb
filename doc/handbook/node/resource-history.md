# 资源历史与内存压力排查

节点启动后，Bitcoin 前台追块、后台补齐历史、balance-history 导入/重放和索引可能同时运行。
只看当时的 `status` 或系统空闲内存，容易错过几分钟前的资源压力。新版 monitor 会在本机持续保存
资源历史，便于在出现 RPC 超时、同步变慢或容器重启后回看。

## 开始记录和查看

启用 monitor 的节点通过正常升级流程激活包含此功能的 node kit，再执行 `up` 即开始记录。
无需开启 control-plane，也无需额外启动一套监控服务。已经发生但未记录的历史不能补录。
前台部署保持 `usdb-node monitor run` 运行；`down` 会停止采样并保留记录。

```bash
usdb-node monitor status
usdb-node monitor resources --limit 12
usdb-node monitor resources --service btc-node --limit 12
usdb-node monitor resources --service balance-history --limit 12
```

`Resource history: available` 表示资源采样和历史写入成功，不代表所有服务健康或每项内核指标都可用。
记录中的 `missing` 列出无法读取的字段；`unknown` 不是零占用。`last sample` 是最后保存样本的时间。
查询直接读取本地历史，Bitcoin RPC 超时期间也可以使用。

## 回看一次 RPC 超时

先用 UTC 时间圈定发生问题的窗口，例如：

```bash
usdb-node monitor resources --since 2026-09-25T16:30:00Z --until 2026-09-25T17:00:00Z --service btc-node --json
usdb-node monitor resources --since 2026-09-25T16:30:00Z --until 2026-09-25T17:00:00Z --service host --json
usdb-node monitor resources --resolution transitions --since 2026-09-25T16:00:00Z --json
usdb-node monitor events --service btc-node --since 2026-09-25T16:00:00Z --json
```

重点对照以下证据：

| 字段或记录 | 含义与用法 |
| --- | --- |
| `configuration.phase` | 当时的资源阶段：`bitcoin`、`overlap`、`steady`。同时保存配置内存上限、Bitcoin dbcache 和 BH 缓存预算。 |
| `memory_current_bytes` / `memory_limit_bytes` | 容器实际总占用和实际生效的上限，包含文件缓存。主机还有空闲 RAM，并不代表容器还有可用额度。 |
| `anon_bytes` / `file_bytes` | 匿名内存与文件缓存，帮助区分缓存占用和进程工作内存。`working_set_bytes` 是 Docker 展示的工作集，口径不同。 |
| `swap_used_bytes` | 容器当前使用的 swap；主机还有换入/换出速率，帮助判断是否持续换页。 |
| `memory_psi_full_avg10` | 最近约 10 秒内因内存压力而全部任务停顿的时间百分比；持续升高比单看使用率更值得关注。 |
| `events_max_delta` / `events_high_delta` | 两次资源采样间触及内存上限/回收阈值的次数。另有 `oom` / `oom_kill` 计数及增量。 |
| `observation.components` | 最近一次服务探针看到的高度、同步/导入/核验状态；Bitcoin 前台和后台高度分别保存。 |
| `observation.probes` / `collection` | 已有探测的耗时与结果。Bitcoin、BH、indexer 的 helper 耗时包含进程启动和检查开销，并非纯 RPC 延迟；chain 记录 RPC batch 耗时。整轮超时也会记录。 |
| `RESOURCE_TRANSITION_*` | controller 开始、恢复执行、完成或失败的配额切换；同一次尝试用 `operation_id` 关联，重启后的重试用 `plan_id` 关联，另有目标配置和实际观察到的容器配额。 |

资源样本通常每 **10 秒**一次，服务探针仍通常每 **30 秒**一轮，采样命令执行较慢时实际间隔会变长。
两者时间戳独立：`observation.observed_at_ms` 是探针轮次开始时间，`observation_is_new=false` 表示复用了
此前探测结果，不能当作新的 RPC 成功。整轮采集被超时终止时，只有整轮耗时/失败信息，不能补出未返回的逐服务探针结果。

`observation.probes[].outcome` 和分钟汇总中的 `probes.*.outcomes` 区分以下结果：

| 结果 | 含义 |
| --- | --- |
| `ok` | 探测成功。节点整体是否就绪仍看各服务的状态和就绪条件。 |
| `not_ready` | 收到有效响应，但尚未满足就绪条件，例如 Bitcoin 仍在追块，或 BH/indexer 尚未同步完成。不是 RPC 通信失败。 |
| `timeout` | helper 执行超时，或 Bitcoin 返回明确的 RPC 超时诊断。 |
| `failed` | 探测失败，例如 RPC 无法连接、响应无效、配置错误或 helper 异常退出。 |

BH 在原生基线启动期间尚未开放 RPC，此时探针确实可能记录 `failed`；是否属于服务异常，
由 monitor 结合启动进度判断，见 [冷启动告警规则](monitor.md#首期规则原则)。
新分类只影响升级后的采样，历史记录不会重写。旧版本的 `failed` 可能包含正常未就绪，
不能直接把旧统计中的全部 `failed` 当成 RPC 故障次数。

BH 核验期间，`observation.components` 中对应组件的 `bootstrap_progress` 还会保存
`verification_stage`、`verification_scanned`、`verification_total`：分别为子阶段、已扫描记录数和
已知总数（未知为 `null`）。`updated_at_ms` 保留服务进度文件的实际更新时间，资源采样重读不会刷新它。
这些字段需要支持核验进度的新服务镜像；旧记录和旧镜像没有的计数不会补成零。

如果 RPC 超时同时伴随容器触顶、内存压力和 swap 增长，而主机仍有大量可用内存，应优先核对容器配额。
若内存压力不明显，应继续检查磁盘 I/O、进程日志及网络；不能仅凭一次超时就判定内存不足。
配额调整步骤见 [日常维护](maintenance.md)。监控只记录和告警，不会自动改配额或重启服务。

## 导出、长时间观察与存储上限

```bash
# 分钟汇总保留资源数值的最小值、最大值、平均值和有效样本数；探针保留耗时范围及各结果次数。
usdb-node monitor resources --resolution minute --service btc-node --limit 100 --json > bitcoin-resources.json

# 同时保留原始样本中的上下文、配置及服务状态。
usdb-node monitor resources --since 2026-09-25T16:30:00Z --limit 1000 --json > resources-page-1.json
```

输出按记录 ID 从新到旧排列，默认最多 100 条，单页最多 1000 条。结果中的 `next_before_id` 非空时，
以相同筛选条件加 `--before-id 该值` 继续导出下一页；单条命令不是自动导出整个时间窗口。
`--service` 筛选每条记录中的资源项，服务未运行/不可读时可能为空，时间点仍保留；服务探针上下文仍可用于关联。
`--session` 可按 `monitor status --json` 中的会话 ID 筛选；无活动 monitor 会话的 controller 事件会话 ID 为空。

默认原始样本保留 **7 天**，分钟汇总和配额切换记录保留 **30 天**，资源历史独立限制为 **256 MiB**。
容量上限优先于天数：触及预算会先清理较旧原始样本，再清理汇总/切换记录，`capacity_evictions` 会显示累计清理条数。
汇总按版本、会话、主机启动、容器身份和配置分别计算，不会把不同配额或重启前后的计数混在一起。
采样缺口不会补零；同一服务探针结果不会因为被多个资源样本引用而重复计数。

如需改变记录策略，在维护窗口停止节点后配置：

```bash
usdb-node down
usdb-node monitor configure --resource-interval-secs 10 --resource-raw-days 7 --resource-minute-days 30 --resource-max-mib 256
usdb-node up
```

资源历史位于 `node.env` 同目录下的 `monitor/resources.sqlite3`，与事件/告警库独立。
备份方法沿用 [monitor 存储与备份](monitor.md#事件存储与备份)，包含存在的 SQLite WAL/SHM 文件。
持续占用数据库读事务可能阻碍 WAL 回收；达到预留上限后会暂停资源历史写入并报告异常。
降低预算时，如果现有数据库文件仍超过新上限，也会明确报告容量问题，不会偷偷删除整个数据库。

## 如何理解告警

`MEMORY_PRESSURE` 需要持续的压力证据：例如停顿伴随内存回收或 swap，或者明显的持续停顿。
文件缓存较大、使用率高但没有压力，单独不会触发此告警。
默认持续 120 秒 warning、600 秒 critical，连续 60 秒有效恢复观测后解除，沿用 monitor 告警窗口配置。
指标缺失、采样过期和容器重建不会被当作恢复证据；重启或计数器重置后的差值不会跨身份计算。

`RESOURCE_HISTORY_FAILED` / `RESOURCE_HISTORY_UNAVAILABLE` 表示资源采集或写入出了问题，应检查空间、
权限和 monitor journal；恢复后会记录 `RESOURCE_HISTORY_RECOVERED`。此时原有节点事件和告警链路仍独立运行，
但磁盘整体故障也可能影响两者。资源告警同样通过已配置的 [通知渠道](notifications.md) 投递。

精细容器指标依赖 Linux cgroup v2 和相应 `/proc`、cgroup 文件的读权限。其他环境仍记录可读取的主机/Docker
指标，其余显式标记缺失。记录不包含 RPC 密码、钱包私钥、完整环境变量或原始 RPC 错误。
