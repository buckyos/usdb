# Ord 索引后端

[返回手册首页](../README.md) · [可选本机铸造后端](control-plane.md#可选的本机铸造后端)

Ord 为本机铸造功能提供铭文和地址索引。它有独立的同步进度；Bitcoin、BH、USDB 已就绪时，Ord 仍可能在索引。
Ord 未就绪不阻止节点同步、验证或挖矿。这里的新资源、阶段日志及停止提示需要同时更新 node kit 和服务镜像；r40 不包含这些改进。

## 磁盘性能与同步时间

**开启主网 Ord，尤其是首次构建地址索引时，建议使用持续随机读写性能较好的本地 NVMe SSD。**
Ord 需要读取 Bitcoin 历史区块，并反复查询、更新和提交本地索引。数据盘的读取延迟、随机读写能力和持续写入表现，
都会影响同步速度；仅满足可用空间门槛，或顺序复制大文件速度很快，不能证明适合这类负载。
`setup` / `doctor` 的容量检查和 `auto` 磁盘档位识别不是磁盘性能验收。

| 存储条件 | 部署建议与预期 |
| --- | --- |
| 本地 NVMe SSD | 优先选择；仍需留足空间，并关注持续负载下的延迟、温度与其他任务竞争 |
| SATA SSD | 可以运行，但不同设备差异很大；按实际索引速度判断，不能仅凭 SATA 接口认定性能不足 |
| 较旧或持续随机读写性能较差的 SSD、机械盘 | 首次索引可能持续多天，后期批次处理和提交明显变慢；对同步时长有要求时应优先改善存储 |
| 虚拟机、共享或网络存储 | 核对宿主机物理后端、共享负载及 I/O 限额；客户机显示 SSD 或非旋转盘不代表后端是 NVMe |

Ord 与 Bitcoin 放在同一物理盘时，历史区块读取和索引写入会竞争 I/O；换成同盘的另一个分区不能消除这类竞争。
早期区块较小、索引较容易被缓存，前几十万块同步很快并不代表后半段也会同样快。
随着索引和区块工作量变化，处理、提交阶段都可能变慢；高度覆盖率不能直接换算剩余时间。
较慢的首次同步也不等于追平后的增量跟踪一定跟不上，但需要分别观察，不能用短期追平表现替代首次索引评估。

当前没有经过主网完整同步验证的统一最低 IOPS 或固定完成时长。需要明确时限时，应在计划使用的实际存储上，
结合相近高度的多个完整批次评估；不要把某台测试机的结果当成所有 NVMe 或 SATA 盘的性能承诺。

## 查看索引进度

```bash
usdb-node minting-status
usdb-node minting-status --json
usdb-node status --watch
usdb-node logs ord-server
```

| 输出 | 含义 |
| --- | --- |
| `WAITING_HISTORY` / `WAITING_TXINDEX` | 等待 Bitcoin 历史验证或交易索引；Ord 尚不能开始完整索引 |
| `committed height` / `ord_height` | 已提交、可供查询的最后一个 Bitcoin 区块高度 |
| `Processing block` / `processing_height` | 当前正在处理的区块，可能尚未提交 |
| `COMMITTING` / `commit_target_height` | 正在写入本批次，目标是使数据库覆盖该高度 |
| `Recent I/O` | 最近采样期间的进程读写字节，包含数据库页面更新；不是文件净增长量，也不保证区块高度已经推进 |
| `RECOVERING` | Ord 报告数据库恢复，完成后才继续索引 |
| `STOPPING` | 已收到停止请求，仍在等待 Ord 完成当前工作并退出 |

默认提交间隔为 **1,000 个区块**，其他提交条件可能让批次更短。这是减少原来 100-block 批次的频繁提交开销、
同时限制 5,000-block 大批次的内存峰值和停止等待时间的初始折中，并非所有磁盘上的最优值。
提交还包括地址索引更新等工作；慢盘上仍可能等待很久，扩大批次不保证按比例加速。
如果原配置显式设置了 `ORD_COMMIT_INTERVAL`，升级保留该值；停机后可在 `node.env` 中改成 `ORD_COMMIT_INTERVAL=1000`。
未设置该项的配置在采用新服务镜像和工具包、重新启动 Ord 后使用新默认值。
比如已提交高度 589,999 不变时，
内部可能正在处理后续区块或提交该批次。`60%` 是区块高度覆盖率，不能据此估算剩余时间：后期区块的数据量不同。
缺少阶段或读写数据时表示没有相应观测，不代表读写量为零。

## 资源分配与旧节点升级

包含[磁盘资源档位](../node/resources.md)的新版本，新配置在 `bitcoin` / `overlap` 阶段只给 Ord 监督进程 512 MiB，
显示 `WAITING_RESOURCES`，不启动索引或探测 Core。新版进入 catchup、主节点采用对应预算后，再分配 Ord 追赶额度。
新 setup 或显式重算自动资源策略时启用 `adaptive-v2`；已保存的 `adaptive-v1` 仍按原公式校验和运行：

- Core 历史验证和 txindex 尚未完成时，保留原有 Core 优先预算；Ord 仍等待依赖。Core 持续就绪 60 秒后进入 `catchup/core-ready` 或 `catchup/runtime-ready`，按运行阶段为 Core、BH、indexer、chain 和辅助作业预留额度，再将未分配内存用于 Ord 追块。
- Ord 目标上限为扣除外部预留后内存的一半，并受剩余内存池和 `--ord-memory-cap` 限制。新配置默认 cap 为 32 GiB；原来显式或已保存的 cap 继续保留。16/20 GB 的恢复节点也适用，不会强行分配大机器额度。
- 保留 Bitcoin、其他服务及系统的预算，整机各服务上限合计不能超额。关闭 Ord 时不预留这份资源。
- **Core 完成后的追块档**：应用缓存为 Ord 容器额度的三分之一，最高 16 GiB，向下取整到 MiB；其余空间供 UTXO 批次、其他内存和文件缓存使用。有效容量 64 GiB、90%、无外部预留时，Ord 可获 32 GiB 容器额度和约 10.7 GiB 缓存；有效容量 20 GiB 时约为 5.5 GiB / 1.83 GiB。这里是预算，不是实际占用或吞吐保证。
- **追平档 `steady`**：连续 60 秒通过当前 Bitcoin 锚点的链一致性检查后，应用缓存降到最多 1 GiB。Ord 只能在启动时设置缓存，因此这里会等待干净退出，再仅重启 Ord 子进程；本机铸造后端短暂不可用，Bitcoin 和 USDB 服务继续运行。
- 追平后只有连续 5 分钟落后至少 1,000 blocks 才切回追块档。短暂 RPC 错误、普通新块到来不会切换；观察中断会重新计时。上次缓存档位作为下次启动的提示保存，不能代替就绪检查。

Ord 子进程的缓存切换本身保留容器额度。新版整机策略在全部同步持续完成后，另行正常停止必要容器并重建，
进入 compact 运行预算，此时 Ord 容器上限为 2～4 GiB、缓存不超过 1 GiB，较小主机可更低。
不会直接降低运行中数据库的硬上限。保留额度不是实际占用，也不会把同一份预算同时分配给其他服务。`usdb-node resources` 显示两档缓存和容器预算，
`status` / `minting-status --json` 显示实际缓存档位。`resources` 还显示 Core 完成后的预算预览，JSON 按 `phase` 和 `runtime_budget` 区分同一阶段的预算。手工资源模式使用固定配置。

分配完整预算后仍需通过原有 Bitcoin 历史验证和 txindex 就绪检查。`WAITING_RESOURCES` 是计划中的等待，不是索引失败。
未重算的旧配置继续按原算法和固定缓存运行。

已有资源配置不会仅因升级而采用新公式。安装包含此改进的版本后，在原运维账号下执行：

```bash
usdb-node down
usdb-node activate-release
usdb-node set-resource-policy --mode auto --storage-profile auto --memory-percent 90 --ord-memory-cap 32g
usdb-node resources
usdb-node doctor
usdb-node up
```

`set-resource-policy --mode auto` 会重算全部节点服务的额度；同为 auto 时保留现有启动阶段，已有 Ord 索引继续使用。
如果有需要保留的自定义内存配置，先查看 `resources`，不要直接重算。manual 模式保留显式设置。
需要降低 Ord 的自动上限时，在停机后使用 `usdb-node set-resource-policy --mode auto --ord-memory-cap 8g`；
上限不是固定分配量。额外同机服务仍应计入 `--external-memory-budget`。

提高缓存通常有助于减少随机磁盘读取，但效果仍取决于数据盘、索引阶段和其他负载。用同一阶段的连续采样对照已提交高度、
处理高度、提交耗时和 I/O 等待；不要把“增大内存”当作固定倍数的加速保证。

## 同步很慢时如何判断和改善

先用 `minting-status` 区分等待依赖与正在索引：`WAITING_HISTORY` / `WAITING_TXINDEX` 应检查 Bitcoin；
`INDEXING` 才需要结合 Ord 的处理高度、提交阶段和磁盘活动判断。已提交高度暂时不变、CPU 较低，单独都不能证明进程卡死。

```bash
usdb-node minting-status --json
usdb-node top
usdb-node resources
usdb-node monitor resources --service ord-server --limit 12
usdb-node logs ord-server
```

资源历史需要 monitor 已启用且已采集；缺失记录不代表没有压力。对照实际容器额度、内存回收/OOM、swap、
批次提交日志和高度增量；主机有空闲内存也不能排除容器额度不足。见[资源历史与内存压力排查](../node/resource-history.md)。

在宿主机确认数据实际落盘位置。以下示例使用默认 Ord 目录；自定义部署应替换为 `usdb-node config` 显示的 `ORD_DATA_HOST_DIR`：

```bash
ord_data_dir="$HOME/.usdb/datasets/ord/btc-mainnet/ord-0.29.0"
findmnt -T "$ord_data_dir" -o TARGET,SOURCE,FSTYPE,OPTIONS
df -h "$ord_data_dir"
lsblk -o NAME,TYPE,SIZE,ROTA,MODEL,MOUNTPOINTS
```

安装了 `sysstat` 的 Linux 主机可在索引期间做只读采样：

```bash
iostat -x -y 5 12
cat /proc/pressure/io
```

关注数据盘的 `r_await` / `w_await`（包含排队的读写延迟）、`aqu-sz`（平均队列长度），以及读写量和 `%util`。
`%util` 持续较高是线索，但对支持并行请求的 SSD、RAID 不能单独用它判定饱和；字段含义见 [sysstat 的 iostat 手册](https://github.com/sysstat/sysstat/blob/master/man/iostat.in)。
`/proc/pressure/io` 反映整台主机的 I/O 等待，不能直接归因于 Ord；部分环境不提供此文件。
虚拟机还需在宿主机核对后端物理盘，同一份 I/O 在分区、逻辑卷和物理盘上的统计不要重复相加。

如果多个批次都推进缓慢，同时索引进程持续等待 I/O、数据盘繁忙，而内存/CPU 限额和 Bitcoin RPC 未显示对应瓶颈，
应优先处理存储性能。小块随机访问即使只有较低 MB/s，也可能主要耗时在 I/O；不能因为远低于接口标称带宽就排除磁盘。
若出现 I/O 错误、设备重置或文件系统只读，应先处理存储故障；SMART 未报错也不能证明性能充足。

建议按以下顺序改善，并用相近高度的多个完整批次比较耗时；重启后的首批单独记录缓存预热影响：

1. 减少同盘备份、扫描、大文件复制及其他虚拟机的 I/O 竞争，核对宿主机和容器是否另设 I/O 限额。
2. 按上一节启用合适的自动资源预算与 1,000-block 批次，为应用缓存、批次数据和文件缓存留空间。`slow-disk` 是内存分配策略，不会改变磁盘的实际能力；单纯提高 CPU 上限也不能解决 I/O 等待。
3. 调整后仍持续受 I/O 限制时，优先考虑更快或负载更低的物理存储。迁移已有索引前正常停机、等待刷盘并备份，保持数据目录契约与权限，核对新挂载后再启动；确认复用了原索引且高度继续推进。不要为测试速度直接删除索引或复制正在写入的数据库。
4. 如果现有存储无法满足预期、暂时不需要本机铸造后端，可按[关闭 Ord 的步骤](control-plane.md#可选的本机铸造后端)保留数据并停用它，继续运行核心节点。

需要进一步诊断时，保留版本、存储后端与挂载信息、资源预算、多个完整批次的高度/耗时及同时段资源样本，
并附上相关错误日志。这样可以区分依赖等待、内存限制、存储竞争与设备故障，避免反复重启或清空数据。

## 持久化诊断日志

Ord 数据目录内的 `ord-events.jsonl` 保存启动、恢复、批次提交开始/完成观测、停止及每分钟活动记录。
路径为 `<数据根目录>/datasets/ord/btc-mainnet/ord-0.29.0/ord-events.jsonl`。每份约 2 MiB，最多保留当前文件和两份轮转文件，
总量约 6 MiB；达到上限后替换最旧记录。逐区块 INFO 输出用于提取进度，不逐条保存。
日志由 Ord 自己的管理进程写入，即使本地 monitor 已停止，仍能记录其退出过程。删除容器或普通升级不会删除这些日志。

例如默认数据根目录为 `~/.usdb` 时：

```bash
tail -n 30 "$HOME/.usdb/datasets/ord/btc-mainnet/ord-0.29.0/ord-events.jsonl"
```

自定义数据目录时替换上述路径。`commit_finished` 的 `evidence` 会说明是观测到已提交高度，还是下一块已开始处理；
时间是观测用时，不能当作数据库内部精确计时。停止后的 `STOPPED` 只在进程实际退出后写入，保留最后观察到的高度。

## 停止时长时间等待

使用 `usdb-node down`。停止顺序为：

1. 停止节点后台编排和观察服务。
2. 按依赖顺序停止 control-plane、USDB chain、indexer、BH，释放这部分负载。
3. 对 Ord 发送正常停止请求，等待当前批次结束，每 15 秒显示一次进度；**Bitcoin 此时继续运行**。
4. Ord 干净退出后清理运行时容器，最后停止 Bitcoin（`down --keep-bitcoin` 除外）。

Ord 等待不设自动强杀截止时间。输出区分 `client wait`（当前命令等待时长）和 `shutdown elapsed`（监督进程记录的停止时长），
同时显示请求时间、已提交高度、处理高度、提交目标及本批提交耗时。重新执行 `down` 不会把真正的停服时长重置为零；
旧镜像缺少这些字段时，仅显示可用信息。

若需要离开当前终端，可按 Ctrl+C；这只停止客户端等待，已经发送的停止请求仍然有效。回来后重新执行 `usdb-node down`。
不要在等待期间执行升级激活、断电或反复重启来加速提交。

长时间没有已提交高度变化时，查看当前处理/提交阶段、最近读写、数据盘空间及错误日志。
有持续 I/O 时可能仍在提交；有 I/O 也不等于一定健康，应同时关注处理高度与错误。无活动或重复报错时保留上述日志再诊断。
异常退出时 `down` 会停在 Ord 检查处并保留其容器和 Bitcoin；这时 USDB chain、indexer、BH 已停止。
查看日志后，再次执行 `down` 可继续移除已停止的容器。

直接使用 `docker compose down`、`docker stop` 或主机关机不经过这条专门等待流程，仍可能触发各自的强制停止超时。
