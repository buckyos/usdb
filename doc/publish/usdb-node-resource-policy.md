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
| `bitcoin` | 50% | 32 GiB | 暂不启动；预生成交叠配置 | 64 GiB |
| `overlap` | 25% | 16 GiB | 37.5% | 64 GiB |
| `steady` | 12.5% | 8 GiB | 50% | 64 GiB |

`steady` 表示 Bitcoin 已完成同步；balance-history 可以仍在追赶历史区块。

| 有效主机内存 | 独立阶段 BTC | 交叠阶段 BTC / BH | 稳态阶段 BTC / BH |
| --- | ---: | ---: | ---: |
| 32 GiB | 16 GiB | 8 / 12 GiB | 4 / 16 GiB |
| 64 GiB | 32 GiB | 16 / 24 GiB | 8 / 32 GiB |
| 256 GiB | 32 GiB | 16 / 64 GiB | 8 / 64 GiB |

实际 `MemTotal` 比标称 64 GiB 小时，额度随之降低，不向上套用标称档位。

管理员可通过 setup/configure/set-resource-policy 的参数指定封顶值：

| CLI 参数 | 持久配置字段 | 默认值 |
| --- | --- | --- |
| `--bh-memory-cap` | `USDB_BH_MEMORY_CAP` | `64g` |
| `--bitcoin-ibd-memory-cap` | `USDB_BTC_IBD_MEMORY_CAP` | `32g` |
| `--bitcoin-overlap-memory-cap` | `USDB_BTC_OVERLAP_MEMORY_CAP` | `16g` |
| `--bitcoin-steady-memory-cap` | `USDB_BTC_STEADY_MEMORY_CAP` | `8g` |

其他服务按 64 GiB 基准同比缩放并分别封顶：indexer 4 GiB、chain 5 GiB、control-plane
1 GiB、registry installer 2 GiB、paired-checkpoint verification 1 GiB。即使它们尚未启动，
计划仍保留其额度；chain-init 与 chain 顺序运行，共用这一预算槽位。系统预留为有效内存的
15.625%，且不低于 4 GiB。包含预留的 64 GiB 交叠/稳态计划合计为 63 GiB。

snapshot-loader 与 balance-history 顺序运行，使用相同额度。实际容器检查仍按同时运行的
实例逐个计数；若两者意外并发，不会凭逻辑上的顺序关系少算一次内存。

## 3. 缓存预算

balance-history 应用缓存合计为其容器额度的 **62.5%**，其中 UTXO 占 25%，余额占 75%。
例如 32 GiB 容器分配 UTXO 5 GiB、余额 15 GiB。`BH_SYNC_MAX_MEMORY_PERCENT=80` 是主动
缩减缓存的触发阈值，容器内其余空间留给 RocksDB、批处理、文件缓存和分配器开销。

Bitcoin 独立同步阶段 dbcache 按其容器额度的 62.5% 计算；交叠/稳态阶段为 50%，并统一封顶到
当前固定的 [Bitcoin Core 28.1 支持的 16 GiB](https://github.com/bitcoin/bitcoin/blob/v28.1/src/txdb.h#L25-L28)。Bitcoin 的
memory+swap 上限仅额外允许最多 2 GiB 或容器额度的八分之一，BH 额外允许 2 GiB。

自动计划中的具体字节字段应由配置工具生成。手工修改自动模式下的容器额度或缓存而未保持
整份计划一致，会在 preflight 被拒绝。manual 模式同样执行与 Rust 启动端一致的缓存检查：

```text
UTXO cache + balance cache <= BH memory limit × (pressure threshold - 10) / 100
```

因此，旧配置中的 12 GiB 容器、8 GiB 缓存和 75% 压力阈值会在 snapshot 导入前报错。

## 4. controller 的切换顺序

1. 启动 Bitcoin，周期性读取完整 readiness 和数据启动锚点。锚点仍为 snapshot 高度
   （无 snapshot 时为 index origin）加 stable lag，并校验配置的 BTC block hash。
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
