# BH / indexer 同步调度与 RPC 重试

同步调度针对常态新块传播延迟，以及 Bitcoin Core 繁忙时短暂 RPC 失败造成的批次重做。稳定高度、确认数、canonical hash 校验和数据库提交边界保持原有协议语义。

## 正常观察与失败退避

| 参数 / 路径 | 默认行为 |
| --- | --- |
| BH `[sync].poll_interval_ms` | 250ms，合法范围 100–60000ms |
| indexer `usdb.upstream_poll_interval_ms` | 250ms，合法范围 100–60000ms；BH 状态采集周期取该值与 1000ms 的较小者 |
| indexer Bitcoin 展示信息 | 独立任务，每次查询完成后等待 1 秒 |
| BH / indexer 外层失败 | 1、2、4、8、15 秒递增，加入 ±20% 抖动，实际等待不超过 15 秒 |
| indexer BH 状态采集 | 一轮总超时 10 秒；失败清除缓存的 snapshot 和 readiness |
| indexer BH commit 查询 | 单次请求超时 10 秒 |

BH 在有积压时连续处理批次，正常轮询间隔仅用于等待新块。成功观察或成功完成同步会重置相应的失败计数；等待过程中检查停止信号。

indexer 的 Bitcoin 查询通过阻塞工作线程执行，与 BH 状态采集互不阻塞，也不会在上一个 Core 查询尚未完成时不断创建新查询。BH 高度、hash、commit、readiness 的相关变化通过进程内 `watch` 通知唤醒同步线程；定期校验仍然保留，不能只根据高度变化判断 reorg。

Docker / node.env 对应变量：

- `BH_SYNC_POLL_INTERVAL_MS`：BH 的空闲轮询周期。
- `USDB_INDEXER_UPSTREAM_POLL_INTERVAL_MS`：indexer 的上游观察周期。

未设置时使用新默认值；显式配置仍优先。旧容器中的二进制不会因为单独更新 node kit 而获得 Rust 行为改动，需要包含新服务镜像的 release。

## 单次读取的有限重试

普通 BH 和 indexer 主同步 BTC 客户端对只读高度、hash、区块以及历史输入证据读取启用运行期重试。应用层最多 3 次尝试，失败后分别等待 1、2 秒。BH 空闲轮询使用单次读取，将调度交给外层退避。

仅重试已分类的可用性错误：连接中断、超时、Core warmup，以及 HTTP 429/502/503/504。认证、非法参数、协议解析等错误不在单次读取层重试。canonical hash 改变会正常触发原有拒绝或恢复路径。

同一客户端的克隆共享重试入口。一个读取进行恢复时，其余工作线程排队；耗尽重试后的短暂冷却让排队请求尽快返回，交由外层退避，避免每个线程都重复耗尽超时。队列和后续尝试共享 60 秒的准入预算，等待可以取消。

**60 秒不是整个同步请求的硬截止时间。** 已进入同步 HTTP transport 的调用仍受其 socket 超时控制；底层重连和 IO 可以延长最后一次尝试。计数中的 `rpc_attempts` 是应用层尝试次数，不包含 transport 自身的重发。

native bootstrap 保持原有独立策略：最多 6 次尝试，等待 2、4、8、16、20 秒。

## 批次提交与观测

`sync.batch_size` / `BH_SYNC_BATCH_SIZE` 默认仍为 128。它是上限：只增加一个块时就处理一个，不等待凑满。节点上的显式 16 配置继续作为上限生效。

第二轮增加 `sync.batch_mode` / `BH_SYNC_BATCH_MODE`，允许 `adaptive`（默认）和 `fixed`：

| 情况 | adaptive 行为 |
| --- | --- |
| 进程启动 | 从 `min(32, batch_size)` 开始 |
| 批内发生 RPC 重试或耗尽重试 | 下一批减半，最低 `min(16, batch_size)`；恢复成功的读取同样计入压力 |
| 成功批次总耗时 ≥30 秒 | 下一批减半，使用同样下限 |
| 连续 4 个完整批次无重试、每批 ≤10 秒提交成功 | 下一批翻倍，不超过 `batch_size` |
| 尾部不足一批、耗时在 10～30 秒之间，或非 RPC 失败 | 保持当前大小，清零升档计数 |

调整状态在同一进程的同步轮次和失败重试之间保留，重启后重新从初始值开始。不依赖节点全局 CATCHUP/STEADY，也不将 reorg、数据库错误归类为可重试的 RPC 故障。首次失败后的外层重试仍从数据库已提交高度继续；即使提交后的维护步骤失败，也不会根据内存中的批次进度重做已提交前缀。

`fixed` 始终使用配置上限（尾部仍可小于上限）。需要保持旧版固定大小或进行对照测试时，在 `node.env` 中设置：

```dotenv
BH_SYNC_BATCH_MODE=fixed
BH_SYNC_BATCH_SIZE=128
```

这些升降档阈值是首版保守策略，经过确定性故障测试，尚需实机吞吐和延迟数据校准。新字段省略时启用 adaptive；已有批次大小仍受尊重，但大于 16 的旧配置若要求始终固定，需要显式选择 fixed。

提交前最终 canonical hash 查询如果暂时失败，会先重试该读取。已经加载和计算的批次仍在内存中，恢复后继续校验；hash 确实改变时拒绝提交。读取耗尽重试后，未提交批次才回到外层重新处理，之前已提交的批次不会因此回滚。

批次日志包含：

- 高度范围、块数和 `elapsed_ms`；
- `rpc_attempts`、`rpc_retries`；
- 选定上限 `selected_batch_size`，以及大小改变时的 `from`、`to`、`reason` 和 `rpc_exhausted`；
- 失败阶段以及具体 RPC 操作、高度/hash、尝试次数和等待时间。

这些数据用于后续比较成功提交吞吐、单块传播延迟、重复处理范围和 RPC 长尾。

## 整批读取

生产预加载器使用同步的 `load_block_range` 入口。native canonical loader 每批只观察一次积压、选择一次来源、刷新一次本地索引；区块的高度与 hash 仍来自 Core RPC，本地 blk 文件仅提供 payload。近 tip 或本地文件不可用时走 RPC，不扫描已关闭的本地来源。

有 N 个区块的本地批次，来源判断从 N 次高度查询降为 1 次；来源阶段的 hash 查询从逐块的约 3N 次降为 N+2 次（起点父块、每块 hash、末尾复核），提交前另保留最终 hash 校验。近 tip 的批次也共享来源判断。一个块的尾部批次仍立即处理，不为节省查询延迟新块。

RPC payload 路径按高度顺序读取，使用现有串行 transport 和重试入口，不额外排队 N 个阻塞任务。读取失败或取消时停止后续读取；本地旧 loader 的默认实现仍受 Rayon 线程池限制。已载入批次继续执行长度、父子链接、payload 承诺、最终 canonical hash 校验后才进入原有原子提交。

## 后续验证

应在包含新 BH 镜像的 release 上比较 fixed=16、fixed=128 和 adaptive 的成功提交吞吐、批次长尾、新块延迟与失败重做范围。RPC 数量减少不等于已测得实机加速。跨批次缓存、增加 RPC 并发连接，以及 native baseline 导入/回放的批次策略仍需独立验证，本轮未修改这些路径的调度策略。
