# USDB 整机内存预算与自动资源切换

## 1. 配置入口与主机基线

新节点的 `usdb-node setup` 和 `configure` 默认使用 `--resource-mode auto`。
controller 根据主机内存生成完整的容器、Bitcoin dbcache 和 balance-history 应用缓存预算，
在 Bitcoin 独立同步、两个索引源交叠追赶和 Bitcoin 稳态之间自动切换。

最低要求为操作系统/controller 可见的 **32 GB，即 32,000,000,000 字节**。分配基准取
`MemTotal` 与 controller 所处 cgroup v1/v2 及其父级有效内存限制的较小值，不使用随负载变化的
`MemAvailable`。表格中的 GiB 和配置中的 `g` 按 1024³ 字节计算。

预算在配置时确定并写入 `node.env`。主机可用额度减小时，preflight 拒绝继续使用旧预算；
主机扩容或管理员希望改变封顶值时，在停机状态重新计算。Docker、操作系统及同机其他工作负载
必须纳入预留空间，swap 不计入可分配 RAM。

```bash
# 新节点：默认自动策略，可覆盖封顶值。
usdb-node setup --bh-memory-cap 64g

# 只读预览当前机器在所有阶段的计划。
usdb-node resources
usdb-node resources --json

# 已有节点显式启用或重新计算策略；保留现有数据。
usdb-node down
usdb-node set-resource-policy --mode auto --bh-memory-cap 96g
usdb-node doctor
usdb-node up
```

旧 `node.env` 缺少 `USDB_RESOURCE_MODE` 时仍采用 manual 模式。升级不会自动授权旧节点的
profile 切换。直接使用原始 `node.env.example` 的底层 runner 用户也保留 manual 模式。
`--bitcoin-profile` 仅用于 `--resource-mode manual`；自动模式通过整机策略管理 Bitcoin。

若同时升级同一 bundle 的 release，应先 `down` 并等待结束，安装包含本功能的新 node kit，
再执行 `set-resource-policy --mode auto -> activate-release -> doctor -> up`。
`activate-release` 只更新镜像引用，不启用自动资源策略，并且会校验现有缓存配置；旧的
12 GiB / 8 GiB / 75% 组合需要先由 `set-resource-policy` 修正，才能通过激活校验。

这一流程以现有数据契约和 snapshot 绑定兼容新 release 为前提。本资源策略不转换拆分前的
整体 snapshot：当前 preflight 只接受 core manifest，启动状态还会检查 release-approved
snapshot 绑定。已经从旧整体 snapshot 导入 RocksDB 的节点仍需要独立的续跑兼容处理，
不能仅凭导入成功就假定上述升级流程可完成启动。

## 2. 比例与封顶

每项分配取“有效主机内存 × 阶段比例”和封顶值的较小值，再向下取整到 MiB。

| 阶段 | Bitcoin 比例 | Bitcoin 默认封顶 | balance-history 比例 | BH 默认封顶 |
| --- | ---: | ---: | ---: | ---: |
| `bitcoin` | 80% | 32 GiB | 暂不启动；预生成交叠配置 | 64 GiB |
| `overlap` | 25% | 16 GiB | 37.5% | 64 GiB |
| `steady` | 12.5% | 8 GiB | 50% | 64 GiB |

上表为普通同步模式的基准。原生 AssumeUTXO 模式进入 `steady` 只要求前台追平、BH/indexer
共识就绪；**不表示 Core 后台历史验证完成**。该模式的稳态 Bitcoin 额度改为有效内存的
50%（受 `USDB_BTC_STEADY_MEMORY_CAP` 原生默认 16 GiB 封顶），增加部分从 BH 原额度等量转移，
BH 至少保留 4 GiB。自定义 BH 封顶不足以转移时，Bitcoin 的增量也相应减少。
BH 应用缓存随新额度重新计算，额度转移不增加整机总预算，所有阶段仍校验有效主机内存上限。

| 有效主机内存 | 独立阶段 BTC | 交叠阶段 BTC / BH | 稳态阶段 BTC / BH |
| --- | ---: | ---: | ---: |
| 32 GiB | 约 25.6 GiB | 8 / 12 GiB | 4 / 16 GiB |
| 64 GiB | 32 GiB | 16 / 24 GiB | 8 / 32 GiB |
| 256 GiB | 32 GiB | 16 / 64 GiB | 8 / 64 GiB |

原生 AssumeUTXO 的 **32 GiB** 主机稳态为 **BTC 16 GiB / BH 4 GiB**；64 GiB 主机为
**16 / 24 GiB**，256 GiB 主机为 **16 / 64 GiB**。实际有效内存约 30.3 GiB 的主机约为
BTC 14.9 GiB / BH 4 GiB，避免后台验证期间 Core 被过小的容器额度挤压。
历史验证结束后保留这一预算，不因状态查询触发停机降档；前台、chain 和 mining 的就绪条件不变。

已有原生自动配置如仍保存旧稳态预算，安装包含该修复的 node kit 后，需要在正常停机状态执行
`usdb-node set-resource-policy --mode auto --bitcoin-steady-memory-cap 16g`，再运行 `doctor` 和 `up`。
已有封顶值（包括旧默认 8g）会保留，因此需要显式提高；新原生配置默认使用 16g。重新计算保留 `steady`
阶段和已有数据，不回到独占同步阶段；不一致的旧预算会被预检明确拒绝，不会仅修改配置后
假装运行中的容器已获得新额度。若同次升级还需激活镜像，按第一节顺序先重新计算、再激活。

实际 `MemTotal` 比标称 64 GiB 小时，额度随之降低，不向上套用标称档位。
例如有效内存约 30.6 GiB 的主机，独立阶段 Bitcoin 约分配 24.5 GiB。
80% 仍受 `USDB_BTC_IBD_MEMORY_CAP` 约束；默认封顶保持 32 GiB，因此 64 GiB 及更大主机
若需超过 32 GiB，必须显式提高该封顶值。配置外部服务预留时，服务比例仍按扣除预留后的预算缩放。

已经写入旧 50% 配额的自动配置需要在升级后重新计算：正常执行 `usdb-node down`，再执行
`usdb-node set-resource-policy --mode auto`，随后按上述 release 激活流程运行 `doctor/up`。
该操作保留已有数据和用户配置的封顶值；仅安装新 node kit 不会更新正在运行的容器额度。

管理员可通过 setup/configure/set-resource-policy 的参数指定封顶值：

| CLI 参数 | 持久配置字段 | 默认值 |
| --- | --- | --- |
| `--bh-memory-cap` | `USDB_BH_MEMORY_CAP` | `64g` |
| `--bitcoin-ibd-memory-cap` | `USDB_BTC_IBD_MEMORY_CAP` | `32g` |
| `--bitcoin-overlap-memory-cap` | `USDB_BTC_OVERLAP_MEMORY_CAP` | `16g` |
| `--bitcoin-steady-memory-cap` | `USDB_BTC_STEADY_MEMORY_CAP` | 原生 AssumeUTXO `16g`；普通模式 `8g` |

其他服务按 64 GiB 基准同比缩放并分别封顶：indexer 4 GiB、chain 5 GiB、control-plane
1 GiB、registry installer 2 GiB、paired-checkpoint verification 1 GiB。`bitcoin` 阶段只计入
Bitcoin、系统和显式声明的外部服务预算；下游额度预先生成，但这个阶段禁止其他节点服务或安装容器运行。
进入 `overlap/steady` 后，即使下游尚未启动，计划也会保留其额度；chain-init 与 chain 顺序运行，
共用这一预算槽位。系统预留为有效内存的
15.625%，且不低于 4 GiB。包含预留的 64 GiB 交叠/稳态计划合计为 63 GiB。

snapshot-loader 与 balance-history 顺序运行，使用相同额度。实际容器检查仍按同时运行的
实例逐个计数；若两者意外并发，不会凭逻辑上的顺序关系少算一次内存。

## 3. 缓存预算

balance-history 应用缓存合计为其容器额度的 **62.5%**，其中 UTXO 占 25%，余额占 75%。
例如 32 GiB 容器分配 UTXO 5 GiB、余额 15 GiB。`BH_SYNC_MAX_MEMORY_PERCENT=80` 是主动
缩减缓存的触发阈值，容器内其余空间留给 RocksDB、批处理、文件缓存和分配器开销。

Bitcoin 独立同步阶段 dbcache 保留原有预算：先按有效主机内存的 50%（受 IBD 封顶及外部服务预留约束）
计算旧容器额度，再取其 62.5%。容器新增的空间留给文件缓存和其他开销，避免增大 dbcache 后再次挤压
文件缓存；约 30.6 GiB 主机上的 dbcache 仍为约 9.6 GiB。交叠/稳态阶段按**增加文件缓存余量前**的
Bitcoin 基准额度取 50%，并统一封顶到 16 GiB。因此原生 32 GiB 主机稳态的 Bitcoin 容器虽扩大到
16 GiB，dbcache 仍为 2 GiB，增加部分留给文件缓存及其他开销。Bitcoin 的
memory+swap 上限仅额外允许最多 2 GiB 或容器额度的八分之一，BH 额外允许 2 GiB。

自动计划中的具体字节字段应由配置工具生成。手工修改自动模式下的容器额度或缓存而未保持
整份计划一致，会在 preflight 被拒绝。manual 模式同样执行与 Rust 启动端一致的缓存检查：

```text
UTXO cache + balance cache <= BH memory limit × (pressure threshold - 10) / 100
```

因此，旧配置中的 12 GiB 容器、8 GiB 缓存和 75% 压力阈值会在 snapshot 导入前报错。

## 4. controller 的切换顺序

以下六步描述普通同步/旧快照路径。原生 AssumeUTXO 在 snapshot 激活且启动文件准备完成后
进入 `overlap` 并启动 BH/indexer；前台追平且两个数据服务共识就绪后进入 `steady` 并启动 chain。
Core 后台历史验证继续独立运行，采用上述保留文件缓存余量的稳态预算。

1. 启动 Bitcoin，周期性读取完整 readiness 和数据启动锚点。锚点仍为 snapshot 高度
   （无 snapshot 时为 index origin）加 stable lag，并校验配置的 BTC block hash。
   启用独立追块的 80% 额度前，先确认没有运行、重启中或暂停的下游容器；若发现并发实例，报告错误并保留现场。
2. 数据锚点成立且 Bitcoin 尚未完全追平时，持久化 `overlap` 切换意图，正常停止 Bitcoin，
   等待刷盘退出后写入新配置并启动。核对实际容器 memory、memory+swap、image、dbcache
   与 profile，并重新通过数据锚点检查后，才启动 snapshot-loader。
3. loader 成功退出后，以当前计划启动 BH；registry installer 独立进行。长时间导入期间
   controller 继续轮询，不把 Bitcoin 降档挂在导入完成之后。
4. 完整 Bitcoin readiness 连续通过三次后进入 `steady`。完整判断包括网络、非 pruned、
   IBD、blocks/headers、txindex 高度、tip 时间和 peer 状态。若下游尚未开始时已经满足该条件，
   直接跳过交叠阶段。阶段只向前推进，短暂断网或普通重组不会重新启用高内存 IBD 档。
5. 切换时先按依赖顺序正常停止已运行的 control-plane、chain、indexer、BH，保留正在进行的
   snapshot 导入。Bitcoin 刷盘退出后重建并核对新配置，再恢复下游。BH 扩大的应用缓存随重启加载。
6. BH 达到 origin 且 query-ready 后启动 indexer；所有完整 readiness 和已有的 chain-init、
   paired-checkpoint recovery 检查通过后才启动 chain。资源阶段也完成后 controller 正常退出。

每次资源交接都先释放旧额度，再启用新额度；停机刷盘期间按旧额度计算。停机只使用正常关闭或
SIGTERM，并等待退出，不添加 SIGKILL 超时。底层自动启动路径使用 `--no-deps` 明确启动当前
阶段，避免因 BH 预算变化让 Compose 重新运行已经完成的 snapshot-loader。

## 5. 中断恢复和观察

与 `node.env` 同目录的 `node.resources.json` 记录目标阶段、计划摘要、切换是否未完成及待恢复
的下游服务。文件原子写入并保持私有权限。必须先记录意图，再停服务；旧容器使用的配置不会
因仅修改配置文件而被当成已经生效。

controller 中断后，已正常停止的容器保持关闭自动重启的状态。重试根据 Docker 的实际参数
恢复切换：如果新 Bitcoin 容器已经采用目标配置，不会仅因 journal 尚未提交而再次重启它。
新 release 或预算在未完成的切换期间发生变化时停止并要求检查。正常资源切换不执行数据迁移、
重建或删除。

`status --watch` 显示当前资源阶段，并将 journal 记录的计划停机标记为等待托管重启；重启已
提交后出现的进程错误仍作为错误显示。`status --json` 保留 resources 检查，未完成的资源切换
不能仅因部分 RPC 已恢复就报告整个启动流程完成。

本版测试覆盖 32/64/256 GiB 和真实非整数 GiB 内存、封顶覆盖、cgroup 父级限制、缓存冲突、
交叠预算、停机/启动后的中断恢复、实际容器参数不符、长时间导入期间降档，以及已追平时跳过
交叠阶段。生产规模下的峰值内存和追块吞吐仍需随发布后的实际运行验证。
