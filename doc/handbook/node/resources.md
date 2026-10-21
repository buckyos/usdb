# 选择磁盘与内存策略

[返回手册首页](../README.md) · [资源历史与排查](resource-history.md)

本节适用于包含 `--storage-profile` 和 `--memory-percent` 的新版工具与配套服务镜像。尚未包含这些参数的旧发布包不能直接执行示例；先按[升级流程](maintenance.md#升级节点)更新。

## 选择档位

新安装在 `setup` 选择数据目录后，会显示磁盘类型提示，并询问整机资源档位和内存预算比例。

| 选项 | 适用情况 | 行为 |
| --- | --- | --- |
| `auto`（默认） | 使用系统提供的磁盘类型提示 | 检测到旋转盘时选择 `slow-disk`，非旋转盘选择 `balanced`；无法识别时提示未知并使用 `balanced` |
| `balanced` | SSD、NVMe 或一般部署 | 为 Core 和 BH 分配较均衡的内存预算 |
| `slow-disk` | HDD，或已确认随机读写较慢的虚拟盘 | 为 Core 留出更多文件缓存空间，减少它与 BH 同步时的资源竞争 |

系统的旋转盘标记只是设备提示，不是性能测试；虚拟磁盘、网络盘、独立挂载的数据子目录可能需要手工选择档位。`auto` 在配置时解析并保存结果，运行期间不会反复检测、切档或触发重启。

开启 Ord 尤其需要关注持续随机读写性能：较差的数据盘即使容量足够，也可能让首次索引持续多天。
优先考虑性能充足的本地 NVMe SSD；SATA 或虚拟磁盘按实际后端和负载评估。
增加缓存和使用 `slow-disk` 可以缓解部分竞争，但不能替代存储升级，见 [Ord 磁盘选型与排查](../services/ord.md#磁盘性能与同步时间)。

新节点可预选：

```bash
usdb-node setup --storage-profile slow-disk --memory-percent 90
```

安装新工具包保留运行中节点的原预算。新版自动策略使用 `bitcoin → overlap → catchup → steady`；旧配置没有策略版本字段时仍按旧规则运行。需要采用新档位时，先完整停机：

```bash
usdb-node down
usdb-node set-resource-policy --mode auto --storage-profile slow-disk --memory-percent 90
usdb-node resources
usdb-node doctor
usdb-node up
```

也可以停机后通过 `setup` 编辑；`keep` 保留已有策略。不能用 `down --keep-bitcoin` 代替完整停机。更换策略保留数据、身份和已有自动资源阶段，不需要重新导入快照。

更换档位时，原档位的默认上限随新档位调整，已有自定义上限和外部服务预留保留；同一命令中显式指定的上限优先。若旧值恰好等于原默认值，工具无法区分它是否曾被手工设置；需要固定该值时，请在命令中再次指定。只修改百分比或单个上限时，其余现有上限保留。

## 百分比怎样计算

推荐节点独占机器。默认比例为 **90%**，可选择 **80%～90%**，这是整套节点服务的内存池上限，不是 Core 单独使用的比例。

计算顺序：

1. 取操作系统物理内存和当前 cgroup 限额中的较小值。
2. 扣除明确配置的其他服务预算 `--external-memory-budget`；默认 0。
3. 按所选百分比计算节点内存池，同时保证初始同步系统预留**至少 4 GiB**，compact 运行预算**至少 2 GiB**，取两者中更保守的额度。
4. 在内存池内分配各阶段的服务预算，应用各服务上限；不为了用满内存而突破上限。

例如可见内存约 30.3 GiB、没有外部服务预留时，即使选择 90%，节点内存池也只有约 26.3 GiB，系统仍留出 4 GiB。Swap 不计入这个 RAM 池。

这里没有直接使用 `free` 输出的瞬时 `available`：它会随页缓存、节点负载变化。预算在配置时计算并保存，避免重启时越分越少或运行期间反复调整。如果必须运行其他服务，给它们设置明确预留，不能仅根据当时进程占用很小就忽略它们。

## 各阶段的分配

新策略区分前台可用和全部同步完成。Core 后台历史验证未完成，不会单独阻止 BH 启动或进入 mining；此时使用 `catchup` 阶段。

| 阶段 | 主要工作 | 内存安排 |
| --- | --- | --- |
| `bitcoin` | Core、快照准备 | BH 等数据服务尚未启动，Core 可使用扣除当前辅助服务后的内存池，受自身上限约束 |
| `overlap` | Core 前台/后台同步，与 BH 导入、回放、验证并行 | 新档位保证 BH 至少 8 GiB；慢盘档将更多其余额度分给 Core |
| `catchup` | 主链可运行；Core、启用的 txindex/Ord 或 USDB 链仍可能追赶 | 保留后台追赶额度；Core 先完成时可独立释放其预算 |
| `steady` | Core 历史、启用的索引、数据服务和 USDB 链均持续就绪 | 采用 compact 运行预算；短暂落后不反复切档 |

开启 Ord 后，前两个阶段只给监督进程 **512 MiB**，显示 `WAITING_RESOURCES`，不会启动 Ord 索引或额外轮询 Core。进入 `catchup` 后，控制器先应用主节点预算，再重建 Ord 容器、分配追赶额度；Ord 仍需等待 Core 历史验证和 txindex 就绪。

32 GB 及以上首次追赶时保留 Core 后台验证预算。新版 Ord 自动策略 `adaptive-v2` 在 Core 历史和 txindex 持续完成后，
为其他服务预留运行额度，再将余量分给 Ord；目标最高为扣除外部预留后内存的一半，受 `--ord-memory-cap` 约束（新配置默认 32 GiB）。
Core 完成后的 Ord 缓存为其容器额度的三分之一、最高 16 GiB。全部同步完成后，compact 阶段使用 2～4 GiB 的 Ord 容器预算、最多 1 GiB 缓存。
所有额度均包含在节点内存池中，无法同时容纳时拒绝配置。旧 `adaptive-v1` 配置保留原预算，停机后显式重算才采用新策略。

以约 30.3 GiB、90%、慢盘档、开启 Ord、无外部预留的机器为例：

| 阶段 | Core 容器上限 | BH 容器上限 | Ord 容器上限 |
| --- | ---: | ---: | ---: |
| `bitcoin` | 约 24.9 GiB | 尚未启动 | 512 MiB（等待） |
| `overlap` | 约 11.2 GiB | 8 GiB | 512 MiB（等待） |
| `catchup` | 约 11.7 GiB | 4 GiB | 约 4 GiB |

系统及其余服务另有池内预算；以本机 `usdb-node resources` 结果为准。表格是预算试算，尚不是该机器的性能实测结果。

Core 容器额度与 `dbcache` 是两个不同限制。增加容器额度主要为文件页缓存和其他分配留空间；`dbcache` 单独按主机容量和上限计算，不随慢盘档增加的额度等比例扩大，也不保证剩余部分全部成为文件缓存。

小内存恢复采用独立的 `core-catchup` 预算：保留 compact 的其他服务预算、系统及外部预留，
将未分配额度优先加给 Core，最高 **8 GiB**，同时遵守 `--bitcoin-steady-memory-cap`。
`dbcache` 保持 compact 的大小，新增额度用于文件缓存和其他内存分配。
例如有效内存 20.49 GiB、90%、Ord 关闭时，Core 可从 2.56 GiB 增至 8 GiB，BH 仍为 5.12 GiB。
容量紧张或开启 Ord 时，Core 实际所得可能低于 8 GiB；不会挤占其他服务最低预算。

Core 前台、后台验证及启用的 txindex 持续就绪 60 秒后，预算进入 `runtime-ready`，
释放额外的 Core 内存；`adaptive-v2` 同时按剩余容量扩大 Ord 的追块预算，阶段仍可保持 `catchup` 等待 Ord 或其他服务。
例如有效容量 20 GiB、90%、开启 Ord、无外部预留时，Core 为 2.5 GiB、BH 为 5 GiB，Ord 为约 5.5 GiB（缓存约 1.83 GiB）。
64 GiB 同条件下，Core 为 4 GiB、BH 为 8 GiB，Ord 为 32 GiB（缓存约 10.7 GiB）。大内存首次启动的 `core-ready` 使用同一分配规则。
这些数值是配置预算，长期负载仍需观察；完整示例见 [Ord 索引后端](../services/ord.md)。
所有同步工作完成后才进入 `steady`。短暂 RPC 不可用不会触发降额。
旧版已经保存的 `catchup / compact` 在新版 `up` 时迁移，涉及的服务正常停止后恢复；
安装工具包或只读查询本身不会改变容器额度。

## 上限、观察与调整

| 默认上限 | `balanced` | `slow-disk` |
| --- | ---: | ---: |
| Core：`bitcoin` | 32 GiB | 64 GiB |
| Core：`overlap` | 16 GiB | 32 GiB |
| Core：首次 `catchup`（AssumeUTXO） | 16 GiB | 32 GiB |
| BH | 64 GiB | 64 GiB |
| Ord 追赶 | 32 GiB | 32 GiB |

上限不是固定分配量。停机后可通过 `--bitcoin-ibd-memory-cap`、`--bitcoin-overlap-memory-cap`、`--bitcoin-steady-memory-cap`、`--bh-memory-cap`、`--ord-memory-cap` 调整；例如：

```bash
usdb-node set-resource-policy --mode auto --storage-profile slow-disk --memory-percent 85 --bitcoin-overlap-memory-cap 16g
```

`status --watch` 显示当前阶段、具体预算名称、各服务容器内存上限、系统预留和未分配额度；
这些是预算而非当前使用量，进程实际占用可用 `usdb-node top` 查看。
`resources` 显示各阶段额度，`resources --json` 可用于采集。资源历史会记录所用档位，便于比较调整前后同一阶段的 RPC 超时、Core/BH 推进速度及 I/O 压力。

慢盘档不能保证消除所有 RPC 超时，也不会把 HDD 变成 SSD。如果持续没有区块进展，应按[资源历史与内存压力排查](resource-history.md)检查连续样本，再决定是否进一步调整或迁移数据盘。

## 同步完成后的运行预算与 VM 缩容

新版 `steady` 要求当前启用的同步任务连续 60 秒完成。controller 在前台可用后继续观察，
状态查询本身不调整资源；普通新块或短暂 RPC 故障不会反复重启服务。
降低容器额度或更改缓存时，先正常停止涉及的服务及其依赖、等待刷盘，再恢复运行。

首次部署仍要求有效容量至少 **32 GB**；已进入运行阶段的节点可按 **16 GB** 的最低边界
重算 compact 预算。默认 90%、无外部预留时，16 GB 主机的 Core/BH/indexer/chain 上限为
2/4/1/2 GiB，系统至少预留 2 GiB；开启 Ord 再预算 2 GiB。辅助作业预算也计入总额，
无法容纳时仍拒绝配置。16 GB 是本轮配置支持边界，主网长期峰值和可选负载还需实际验证。

建议确认所有同步完成、没有资源切换后，先 `usdb-node down`，再关闭 VM 调整内存。
恢复 VM 后，实际 `up` 会在容器已停止的条件下按当前容量重算并重新检查同步；旧的 `steady`
按 `catchup` 重新观察，已有 compact 预算冷启动时先采用 `core-catchup`，重新确认 Core 同步后释放额度。已有数据和身份继续由原有门禁检查。
只读 `doctor` 不会重算，若希望先检查再启动，可在停机状态执行
`set-resource-policy --mode auto → resources → doctor → up`。

完整预算、阶段判定和中断恢复见[资源策略说明](../../publish/usdb-node-resource-policy.md)。
