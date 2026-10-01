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

新节点可预选：

```bash
usdb-node setup --storage-profile slow-disk --memory-percent 90
```

已有节点在升级后保持原预算算法，`resources` 中显示 `legacy`；这不是数据格式问题。需要采用新档位时，先完整停机：

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
3. 按所选百分比计算节点内存池，同时保证系统预留**至少 4 GiB**，取两者中更保守的额度。
4. 在内存池内分配各阶段的服务预算，应用各服务上限；不为了用满内存而突破上限。

例如可见内存约 30.3 GiB、没有外部服务预留时，即使选择 90%，节点内存池也只有约 26.3 GiB，系统仍留出 4 GiB。Swap 不计入这个 RAM 池。

这里没有直接使用 `free` 输出的瞬时 `available`：它会随页缓存、节点负载变化。预算在配置时计算并保存，避免重启时越分越少或运行期间反复调整。如果必须运行其他服务，给它们设置明确预留，不能仅根据当时进程占用很小就忽略它们。

## 各阶段的分配

新档位继续使用 `bitcoin → overlap → steady` 三个阶段，按既有就绪条件推进。Core 后台历史验证未完成，不会单独阻止 BH 启动或进入 mining。

| 阶段 | 主要工作 | 内存安排 |
| --- | --- | --- |
| `bitcoin` | Core、快照准备 | BH 等数据服务尚未启动，Core 可使用扣除当前辅助服务后的内存池，受自身上限约束 |
| `overlap` | Core 前台/后台同步，与 BH 导入、回放、验证并行 | 新档位保证 BH 至少 8 GiB；慢盘档将更多其余额度分给 Core |
| `steady` | 主链运行；Core 仍可能补历史 | BH 至少 4 GiB；保留 Core 文件缓存空间，不因单次 RPC 成功或失败自动降档 |

开启 Ord 后，前两个阶段只给监督进程 **512 MiB**，显示 `WAITING_RESOURCES`，不会启动 Ord 索引或额外轮询 Core。进入 `steady` 后，控制器先应用主节点预算，再重建 Ord 容器、分配完整额度；Ord 仍需等待 Core 历史验证和 txindex 就绪。

完整 Ord 预算：`balanced` 约占扣除外部预留后内存的四分之一，`slow-disk` 约八分之一，默认至少 4 GiB、最高 16 GiB；显式设置较低上限时可以降到 2 GiB。索引缓存不超过 Ord 额度的一半。所有这些额度均包含在节点内存池中，无法同时容纳时拒绝配置。

以约 30.3 GiB、90%、慢盘档、开启 Ord、无外部预留的机器为例：

| 阶段 | Core 容器上限 | BH 容器上限 | Ord 容器上限 |
| --- | ---: | ---: | ---: |
| `bitcoin` | 约 24.9 GiB | 尚未启动 | 512 MiB（等待） |
| `overlap` | 约 11.2 GiB | 8 GiB | 512 MiB（等待） |
| `steady` | 约 11.7 GiB | 4 GiB | 4 GiB |

系统及其余服务另有池内预算；以本机 `usdb-node resources` 结果为准。表格是预算试算，尚不是该机器的性能实测结果。

Core 容器额度与 `dbcache` 是两个不同限制。增加容器额度主要为文件页缓存和其他分配留空间；`dbcache` 单独按主机容量和上限计算，不随慢盘档增加的额度等比例扩大，也不保证剩余部分全部成为文件缓存。

## 上限、观察与调整

| 默认上限 | `balanced` | `slow-disk` |
| --- | ---: | ---: |
| Core：`bitcoin` | 32 GiB | 64 GiB |
| Core：`overlap` | 16 GiB | 32 GiB |
| Core：`steady`（AssumeUTXO） | 16 GiB | 32 GiB |
| BH | 64 GiB | 64 GiB |
| Ord | 16 GiB | 16 GiB |

上限不是固定分配量。停机后可通过 `--bitcoin-ibd-memory-cap`、`--bitcoin-overlap-memory-cap`、`--bitcoin-steady-memory-cap`、`--bh-memory-cap`、`--ord-memory-cap` 调整；例如：

```bash
usdb-node set-resource-policy --mode auto --storage-profile slow-disk --memory-percent 85 --bitcoin-overlap-memory-cap 16g
```

`status` 显示当前档位和预算比例；`resources` 显示各阶段额度、系统预留和因上限留下的未分配空间，`resources --json` 可用于采集。资源历史会记录所用档位，便于比较调整前后同一阶段的 RPC 超时、Core/BH 推进速度及 I/O 压力。

慢盘档不能保证消除所有 RPC 超时，也不会把 HDD 变成 SSD。如果持续没有区块进展，应按[资源历史与内存压力排查](resource-history.md)检查连续样本，再决定是否进一步调整或迁移数据盘。
