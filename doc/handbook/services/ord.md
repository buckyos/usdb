# Ord 索引后端

[返回手册首页](../README.md) · [可选本机铸造后端](control-plane.md#可选的本机铸造后端)

Ord 为本机铸造功能提供铭文和地址索引。它有独立的同步进度；Bitcoin、BH、USDB 已就绪时，Ord 仍可能在索引。
Ord 未就绪不阻止节点同步、验证或挖矿。这里的新资源、阶段日志及停止提示需要同时更新 node kit 和服务镜像；r40 不包含这些改进。

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

默认提交间隔为 **100 个区块**，其他提交条件可能让批次更短。原先 5,000 的批次在地址索引阶段可能积累数百万条待写入记录，
一次提交甚至耗时数小时。缩小批次主要控制内存峰值和停止等待时间，实际总吞吐量仍需根据磁盘实测。
如果原配置显式设置了 `ORD_COMMIT_INTERVAL`，升级保留该值；停机后可在 `node.env` 中改成 `ORD_COMMIT_INTERVAL=100`。
比如已提交高度 589,999 不变时，
内部可能正在处理后续区块或提交该批次。`60%` 是区块高度覆盖率，不能据此估算剩余时间：后期区块的数据量不同。
缺少阶段或读写数据时表示没有相应观测，不代表读写量为零。

## 资源分配与旧节点升级

包含[磁盘资源档位](../node/resources.md)的新版本，新配置在 `bitcoin` / `overlap` 阶段只给 Ord 监督进程 512 MiB，
显示 `WAITING_RESOURCES`，不启动索引或探测 Core。进入 steady、主节点采用对应预算后，再分配完整 Ord 额度。
新 setup 或显式重算自动资源策略时启用 `adaptive-v1`：

- 进入节点 `steady` 阶段后，Ord 可使用已完成启动的 BH 超出 4 GiB 的预算及未分配额度，目标上限为扣除外部预留后内存的一半，受 `--ord-memory-cap` 限制。新配置默认上限 32 GiB；原来显式或已保存的上限继续保留。
- 保留 Bitcoin、其他服务及系统的预算，整机各服务上限合计不能超额。关闭 Ord 时不预留这份资源。
- **追块档 `catchup`**：应用缓存为 Ord 容器额度的四分之一，最高 8 GiB；其余空间供 UTXO 批次、其他内存和文件缓存使用。
- **追平档 `steady`**：连续 60 秒通过当前 Bitcoin 锚点的链一致性检查后，应用缓存降到最多 1 GiB。Ord 只能在启动时设置缓存，因此这里会等待干净退出，再仅重启 Ord 子进程；本机铸造后端短暂不可用，Bitcoin 和 USDB 服务继续运行。
- 追平后只有连续 5 分钟落后至少 1,000 blocks 才切回追块档。短暂 RPC 错误、普通新块到来不会切换；观察中断会重新计时。上次缓存档位作为下次启动的提示保存，不能代替就绪检查。

容器**最大额度仍保留**，不会在追平时强行压低内存上限、驱逐文件缓存。缓存档位降低减少的是应用内存使用；
保留额度不是实际占用，也不会把同一份预算同时分配给其他服务。`usdb-node resources` 显示两档缓存和容器预算，
`status` / `minting-status --json` 显示实际缓存档位。手工资源模式使用固定配置。

分配完整预算后仍需通过原有 Bitcoin 历史验证和 txindex 就绪检查。`WAITING_RESOURCES` 是计划中的等待，不是索引失败。
未重算的旧配置继续按原算法和固定缓存运行。

已有配置不会仅因升级而改变。安装包含此改进的版本后，在原运维账号下执行：

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
