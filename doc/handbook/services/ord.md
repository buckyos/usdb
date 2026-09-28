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

默认提交间隔为 5,000 个区块，其他提交条件可能让批次更短。比如已提交高度 589,999 不变时，
内部可能正在处理后续区块或提交该批次。`60%` 是区块高度覆盖率，不能据此估算剩余时间：后期区块的数据量不同。
缺少阶段或读写数据时表示没有相应观测，不代表读写量为零。

## 资源分配与旧节点升级

新配置使用整机自动预算：Ord 取扣除外部服务预留后主机内存约四分之一，默认最高 16 GiB；缓存为 Ord 额度的一半。
通常 32 GiB 主机约为 8 GiB 内存 / 4 GiB 缓存，64 GiB 主机约为 16 GiB / 8 GiB。
剩余预算用于系统、Bitcoin、BH 和其他服务，配置检查拒绝超过整机额度的方案。关闭 Ord 时不预留这份资源。

已有配置不会仅因升级而改变。安装包含此改进的版本后，在原运维账号下执行：

```bash
usdb-node down
usdb-node activate-release
usdb-node set-resource-policy --mode auto
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

使用 `usdb-node down`。新版先发送一次正常停止请求，等待 Ord 的当前批次结束，并每 15 秒显示等待信息；
Bitcoin 在此期间继续运行。该路径不设自动强杀截止时间。

若需要离开当前终端，可按 Ctrl+C；这只停止客户端等待，已经发送的停止请求仍然有效。回来后重新执行 `usdb-node down`。
不要在等待期间执行升级激活、断电或反复重启来加速提交。

长时间没有已提交高度变化时，查看当前处理/提交阶段、最近读写、数据盘空间及错误日志。
有持续 I/O 时可能仍在提交；有 I/O 也不等于一定健康，应同时关注处理高度与错误。无活动或重复报错时保留上述日志再诊断。
异常退出时 `down` 会停在 Ord 检查处并保留其容器；查看日志后，再次执行 `down` 可继续移除已停止的容器。

直接使用 `docker compose down`、`docker stop` 或主机关机不经过这条专门等待流程，仍可能触发各自的强制停止超时。
