# USDB Economic View 规模评估

状态：2026-07-27 已完成 `100 / 1K / 10K / 100K` deterministic release v4
复核，并补齐 cold-cache、物理 I/O、并发分页和 multi-leader topology。

## 1. 目标与范围

本评估面向 UIP-0004 至 UIP-0006 的三类只读经济视图：

- `get_candidate_set_view`
- `get_collab_breakdown`
- `get_pass_economic_profile`

每个规模 `N` 都构造 `N` 张 active standard pass 和 `N` 张 active collab pass。
默认所有 collab 集中指向同一张 standard Leader，其中一半使用固定
`leader_pass_id`，另一半使用 `leader_btc_addr`。v4 还支持把 collab
round-robin 分散到指定数量的 Leader，用于比较单 Leader 热点和多 Leader 拓扑。

测试使用真实 SQLite pass/history storage 和 RocksDB energy storage，不 mock 查询层。默认 `limit=100`，每个规模运行在独立 release test 进程中。

## 2. 一致性断言

规模测试不是只记录耗时；以下条件任一失败都会使测试失败：

- candidate 总数严格等于 active standard 数，所有 collab 均被排除。
- candidate 全分页满足 `effective_energy DESC, pass_id ASC`，Leader contribution 与独立公式计算一致。
- breakdown 两种排序遍历得到相同 canonical item 集合，逐项 contribution 之和等于完整 aggregate。
- fixed/address collab 数量与 fixture 一致。
- Leader profile、candidate item、breakdown aggregate/count 交叉一致。
- 同一冻结 `external_state` 重放的有序摘要完全一致。
- 有序摘要按 item 长度分帧编码，不依赖分页边界；10K 数据在 `limit=20/100/500` 下得到相同 candidate/breakdown digest。
- 关闭并重新打开 SQLite/RocksDB 后，candidate 与 breakdown 摘要仍完全一致。

## 3. 观测口径

- latency：test 进程内 `Instant` wall time，单位为微秒，报告展示为毫秒。
- memory：Linux `/proc/self/status` 的 `VmRSS` 与 `VmHWM`。
- database size：fixture 完成后测试 root 下 SQLite、RocksDB 和配置文件总大小。
- SQLite：trace v2 记录 statement、result row、VM step、full-scan step 和 sort 数量。
- RocksDB：测试专用计数器记录 point get、iterator seek 和成功 decode 数量。
- process I/O：Linux `/proc/self/io` 记录 syscall/char 和 physical
  `read_bytes/write_bytes` delta。
- cold-cache：关闭 storage、`sync_all` 后对 fixture 文件执行 GNU
  `dd iflag=nocache`，重新打开服务再遍历；报告同时记录实际 eviction 文件数和字节数。
- concurrency：多个 barrier-synchronized client 对同一冻结 state 并发遍历
  candidate 和热点 Leader breakdown，逐 client 校验 digest。

SQLite/RocksDB 计数均为逻辑操作或 VM 工作量。只有 v4 cold-cache 报告中的
`read_bytes` 是内核计数的物理读取；它仍受宿主机、虚拟化块设备和并行负载影响，
不能直接作为生产 SLA。

## 4. 执行环境

- Linux `6.1.0-26-amd64`，x86_64 虚拟化环境
- Intel Core i7-13700KF，12 个可见逻辑 CPU
- Rust `1.91.0`
- v3 base commit：`697c349`
- v4 100K base commit：`b8b1289`，叠加本批尚未提交的 scale-test 改动
- profile：Cargo `--release`

## 5. 首轮结果

`N` 表示 `N standard + N collab`，默认 `limit=100`。

| N | 总 pass | DB MiB | fixture ms | candidate 首次全分页 ms | candidate cache replay ms | breakdown 首次全分页 ms | breakdown cache replay ms | profile ms | restart candidate ms | restart breakdown ms | peak RSS MiB |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 100 | 200 | 0.43 | 20 | 1.50 | 0.23 | 1.05 | 0.42 | 1.04 | 1.69 | 1.13 | 22.35 |
| 1,000 | 2,000 | 2.95 | 43 | 12.72 | 2.28 | 9.35 | 3.72 | 5.40 | 13.85 | 10.01 | 27.82 |
| 10,000 | 20,000 | 27.96 | 380 | 327.52 | 24.64 | 238.99 | 142.57 | 95.65 | 323.12 | 242.89 | 50.76 |

10K 首次 candidate 派生读取 20K 条 raw-energy record，首次 breakdown 读取 10K 条；相应 cache replay 的 RocksDB 读取均为 0。10K candidate 首次派生额外 RSS 约 10.20 MiB。

10K 查询没有触发 SQLite full-scan step。candidate 首次全分页记录约 8.32M VM steps，breakdown 约 1.85M VM steps；主要剩余数据库成本来自历史 snapshot 的分页扫描和每个 continuation page 的 external-state 二次校验。

## 6. 分页敏感性

10K 数据在不同 `limit` 下均得到相同有序摘要、canonical breakdown 集合和 aggregate：

| limit | 页数 | candidate 首次 ms | candidate replay ms | replay SQLite statements | breakdown 首次 ms | breakdown replay ms |
| ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 20 | 500 | 382.71 | 91.32 | 14,500 | 762.02 | 658.59 |
| 100 | 100 | 327.52 | 24.64 | 2,900 | 238.99 | 142.57 |
| 500 | 20 | 297.19 | 10.95 | 580 | 133.01 | 37.54 |

页数增加时，cache replay 仍需逐页验证 cursor 绑定的完整 external state，因此 statement 数和延迟随页数增长。这是 fail-closed historical consistency 的预期成本；服务没有重新派生完整 candidate/breakdown 数据集。

10K 三种页长共同得到：

- candidate ordered digest：`96607fd1889b8d0658c589ab8ef309f8120e226d9b1b1ee450af657d18e5f271`
- breakdown ordered digest：`be90af588f03ab48fac27333c5bf9ffb571eb3b87594c39a19f8384046c65b63`
- breakdown canonical digest：`5a5311954206af4723c0b9eb906e562595f223b0834b5f56f0fb94cd7a4c6154`
- aggregate contribution：`50005000`

## 7. 本轮发现与修正

原实现每个 cursor page 都完整重建并排序数据集，且 candidate 对每张 standard 分别查询两类 collab 关系。1K 基线中：

- candidate 全分页约 16.16 秒、40,288 条 SQLite read statement、20K 次 RocksDB seek。
- breakdown 全分页约 334.40 毫秒、10,320 条 SQLite read statement、10K 次 RocksDB seek。

本轮改为：

- 派生层一次加载 active standard/collab snapshot，建立固定 pass-id 与 address-owner Leader map。
- address Leader 仍按当前 BTC network 规范化，并与存储的 owner relation 做 invariant 校验。
- 每张 standard/collab 只读取一次 raw-energy record，并保持状态/缺失记录 fail closed。
- breakdown 直接使用已经按 Leader relation 过滤的历史结果，不再为每个 collab 重查 Leader。
- candidate 与已排序 breakdown 使用完整 `EconomicExternalState` 作为 cache key；cache 有界且只在派生后 state-ref 二次校验通过时写入。
- continuation 仍在每页前后校验冻结 state；same-height reorg 不会命中旧状态数据。

优化后 1K candidate 全分页约 13.02 毫秒，RocksDB seek 降为 2K；breakdown 约 9.86 毫秒，RocksDB seek 降为 1K。

## 8. 100K、Cold Cache 与并发结果

`N=100,000` 表示 100K standard + 100K collab，共 200K pass；本轮统一
`limit=500`。

| topology | DB MiB | warm candidate ms | restart candidate ms | cold candidate ms | warm breakdown ms | restart breakdown ms | cold breakdown ms | peak RSS MiB |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 Leader | 284.62 | 22,232 | 22,176 | 27,849 | 7,260 | 7,280 | 12,296 | 177.57 |
| 1,000 Leaders | 287.88 | 22,505 | 22,444 | - | 258 (100-item hotspot) | - | - | 179.14 |

单 Leader cold candidate eviction 约 276.79 MiB fixture 文件，并产生约
224.07 MiB physical read；cold breakdown 产生约 218.00 MiB physical read。
warm/restart/cold 三条路径的 ordered digest 完全一致，SQLite fullscan 均为 0。

1,000-Leader 拓扑中每个 Leader 恰好关联 100 张 collab。candidate 聚合仍覆盖
全部 100K collab，贡献总和与单 Leader 拓扑一致；热点 breakdown 只遍历 100
items，因此从 7.26 秒下降到约 258 毫秒。

8 个并发 client、每 client 2 次完整 traversal 共读取 1,601,600 items：

- wall time：约 1.229 秒；
- candidate traversal p50/p95：约 607/624 毫秒；
- hotspot breakdown p50/p95：约 1.79/3.28 毫秒；
- 16 组 candidate 和 breakdown digest 全部一致；
- cache-hit 路径 RocksDB seek/decode 为 0，额外 RSS 约 2.82 MiB。

## 9. 运行方式

默认运行全部三档：

```bash
src/btc/usdb-indexer/scripts/run_economic_scale_eval.sh
```

指定规模和页长：

```bash
USDB_ECONOMIC_SCALE_PAGE_LIMIT=500 \
  src/btc/usdb-indexer/scripts/run_economic_scale_eval.sh 10000
```

100K 单 Leader cold-cache：

```bash
USDB_ECONOMIC_SCALE_PAGE_LIMIT=500 \
USDB_ECONOMIC_SCALE_COLD_CACHE=1 \
src/btc/usdb-indexer/scripts/run_economic_scale_eval.sh 100000
```

100K、1,000 Leaders、8 个并发 client：

```bash
USDB_ECONOMIC_SCALE_PAGE_LIMIT=500 \
USDB_ECONOMIC_SCALE_LEADER_COUNT=1000 \
USDB_ECONOMIC_SCALE_CONCURRENT_CLIENTS=8 \
USDB_ECONOMIC_SCALE_CONCURRENT_ITERATIONS=2 \
src/btc/usdb-indexer/scripts/run_economic_scale_eval.sh 100000
```

JSON 默认写入 `src/btc/target/economic-scale/`。测试本身标记为 ignored，不进入普通单元测试时延预算。

## 10. 后续评估边界

容量基线和跨页长重放一致性已经建立，但生产容量结论仍需要补充：

1. 多个 historical height/Leader 交错查询和 bounded-cache eviction。
2. 100K 以上离线容量点，以及 30-60 分钟 replay/查询 soak。
3. 大规模 consume/remint/reorg 后重新打开服务，再验证同一历史 context 与新 canonical context。
4. 在目标部署磁盘上用 cgroup/`fio` 隔离背景负载，重复 cold-cache 物理 I/O。

这些项目属于容量与运行稳定性评估，不阻塞 UIP-0001 至 UIP-0006 当前协议行为对齐结论。
