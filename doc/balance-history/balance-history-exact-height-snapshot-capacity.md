# Balance-History Exact-Height Snapshot 容量评估

## 目标

`regtest_exact_height_snapshot_capacity.sh` 在隔离 Bitcoin Core regtest 上构造指定数量的
真实未花费输出，并分别测量 exact-height snapshot 的同步、导出、校验和安装阶段。该入口
用于比较不同数据量、机器和存储设备，不作为默认快速回归测试。

测试使用一个临时 descriptor wallet 派生唯一收款地址，随后卸载该 wallet，避免 funding
wallet 在后续批次中误花已生成的容量 UTXO。每批交易确认后才生成下一批，因此 100K 档位
不会依赖长未确认交易链。

## 运行

默认生成 1K UTXO，每笔交易最多 1K 输出：

```bash
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh
```

常用档位：

```bash
SNAPSHOT_CAPACITY_UTXOS=10000 \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh

SNAPSHOT_CAPACITY_UTXOS=100000 \
SNAPSHOT_CAPACITY_OUTPUTS_PER_TX=1000 \
METRICS_FILE=/data/bench/snapshot-100k.json \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh
```

`SNAPSHOT_CAPACITY_OUTPUTS_PER_TX` 上限为 2000，防止单笔测试交易接近标准交易重量限制。
`SNAPSHOT_CAPACITY_OUTPUT_BTC` 默认是 `0.00001000` BTC，必须高于当前测试输出的 dust
边界。大容量运行默认每 10 个交易批次输出一次进度；可通过
`SNAPSHOT_CAPACITY_PROGRESS_EVERY` 调整，首批和末批始终输出。

脚本自动创建的临时工作目录会在成功后删除，失败时保留并打印路径用于诊断；调用方显式设置
`WORK_DIR` 时，脚本不会自动删除该目录。默认 metrics 会发布到 `/tmp` 下带时间戳和进程号的
独立 JSON 文件，因此成功清理 workspace 不会丢失测试结果。正式测试建议始终显式设置
`METRICS_FILE`，把结果放入长期保存的 benchmark 目录。

### 执行模式

默认 `debug_split` 使用测试专用 sealed checkpoint 中止，把 sync 和 export 拆开测量，保持与
历史基线兼容：

```bash
SNAPSHOT_CAPACITY_EXECUTION_PROFILE=debug_split \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh
```

正式容量评估应使用 `release_e2e`。该模式构建和运行 release binaries，并把同步、seal、导出
和内部校验作为一个真实 `create` 阶段测量，不依赖 debug-only 中止点：

```bash
SNAPSHOT_CAPACITY_EXECUTION_PROFILE=release_e2e \
SNAPSHOT_CAPACITY_UTXOS=5000000 \
SNAPSHOT_CAPACITY_PROGRESS_EVERY=100 \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh
```

`release_e2e` 不能在 sync/export 之间执行 advisory eviction，因此只支持 warm。需要拆分阶段
或测试 `POSIX_FADV_DONTNEED` 时继续使用 `debug_split`。

## 冷缓存模式

设置以下变量后，脚本会在 workspace 完成 exact-height seal 且进程关闭后，对其中每个文件
调用 `POSIX_FADV_DONTNEED`，再开始导出：

```bash
SNAPSHOT_CAPACITY_COLD_CACHE=1 \
SNAPSHOT_CAPACITY_UTXOS=100000 \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh
```

该模式在指标中记为 `cold_advisory`。它只是无特权的内核回收提示，不保证页面已从系统
缓存完全移除，也不等价于重启机器、清理设备缓存或以 root 写入 `drop_caches`。正式硬件
的物理 I/O 验收必须结合独立设备监控和一次真正冷启动运行。

## 指标

脚本输出一个 JSON 文件，包含：

- requested/actual UTXO 数量、交易数和 snapshot 文件大小；
- snapshot 与 verify 报告的 SHA-256，用于重放一致性检查；
- `sync`、`export`、`verify`、`install` 的 wall-clock 秒数；
- 各目标进程从 `/proc/<pid>/status` 以 20ms 周期采样得到的峰值 RSS；
- 各目标进程从 `/proc/<pid>/io` 采样得到的 physical read/write bytes；
- `WORK_DIR` 所在块设备在 chain load 和各阶段前后的 I/O operation、sector 和 I/O time 差值；
- 构造链上负载所需时间和 warm/cold-advisory 模式。

`peak_rss_kib_sampled` 是采样值，不是内核提供的完整进程树 high-water mark。脚本直接运行
预构建二进制，避免把 Cargo 编译开销计入阶段指标；它也不会统计 bitcoind 自身资源。

metrics schema v2 会记录 `execution_profile`、`shared_block_device`、阶段级
`process_*_bytes_sampled` 和 `shared_block_device_io`。进程计数可能漏掉退出前最后一次 flush；
块设备计数则会包含同一磁盘上其他进程的并发 I/O。两者应交叉使用，不能把 shared-device
delta 解释为目标进程的精确独占用量。

## 当前目标硬件 100K 基线

2026-08-23 在当前目标机器、代码 revision `db2da3f`、Bitcoin Core `28.1` 上完成了 100K
warm 和 cold-advisory 两轮隔离 regtest。机器为 12 logical CPU 的 Intel i7-13700KF 虚拟化
环境、约 62 GiB 内存和 ext4 virtual disk：

| 指标 | Warm | Cold advisory |
| --- | ---: | ---: |
| requested UTXO | 100,000 | 100,000 |
| actual snapshot UTXO | 100,152 | 100,152 |
| snapshot bytes | 40,005,632 | 40,001,536 |
| chain load | 11.58s | 11.78s |
| sync | 2.07s / 131.4 MiB | 2.05s / 131.2 MiB |
| export | 3.90s / 121.2 MiB | 3.88s / 122.2 MiB |
| verify | 0.67s / 24.2 MiB | 0.66s / 24.0 MiB |
| install | 1.42s / 140.1 MiB | 1.42s / 136.6 MiB |

阶段单元格格式为 `elapsed / peak_rss_sampled`。两轮 create/verify 的 SHA-256 均分别一致。
原始指标保存在：

```text
/tmp/usdb-bh-snapshot-capacity-100k-metrics.json
/tmp/usdb-bh-snapshot-capacity-100k-cold-metrics.json
```

40 MB 测试 artifact 能完整覆盖 100K 逻辑路径，但不足以让 advisory eviction 稳定表现为真实
物理 I/O，不能从 warm/cold 数值接近推导主网冷缓存代价很低。

该 harness 依赖 debug-only `sealed` checkpoint 中止来拆分 sync 和 export，因此表中耗时来自
debug 二进制，只能作为当前 revision 的功能/趋势基线，不能作为 release 主网性能 SLA。
正式主网构建必须使用 release 二进制，并独立记录端到端 wall time、峰值 RSS 和设备 I/O。

## 当前目标硬件 250K/1M 扩展基线

在释放旧 `~/.usdb/balance-history` 数据、目标文件系统恢复到约 `550 GiB` 可用空间后，继续在
同一机器运行了 250K warm、1M warm 和 1M cold-advisory。运行代码 revision 为 `a53c95f`，
每笔交易仍固定 1,000 个输出，以便和 100K 基线比较：

| 指标 | 250K warm | 1M warm | 1M cold advisory |
| --- | ---: | ---: | ---: |
| requested UTXO | 250,000 | 1,000,000 | 1,000,000 |
| actual snapshot UTXO | 250,227 | 1,000,602 | 1,000,602 |
| snapshot bytes | 107,106,304 | 445,177,856 | 446,169,088 |
| chain load | 41.33s | 230.79s | 224.73s |
| sync | 4.52s / 198.6 MiB | 17.53s / 331.4 MiB | 17.37s / 337.1 MiB |
| export | 9.28s / 192.2 MiB | 31.30s / 283.8 MiB | 31.26s / 286.4 MiB |
| verify | 1.76s / 24.6 MiB | 7.34s / 24.5 MiB | 7.44s / 24.0 MiB |
| install | 3.33s / 201.9 MiB | 12.80s / 389.9 MiB | 12.78s / 392.5 MiB |

三轮 create/verify 的 SHA-256 均在各自运行内一致，install 全部成功。原始指标保存在：

```text
/tmp/usdb-bh-snapshot-capacity-250k-metrics.json
/tmp/usdb-bh-snapshot-capacity-1m-metrics.json
/tmp/usdb-bh-snapshot-capacity-1m-cold-metrics.json
```

相对 100K warm，250K 的 artifact/export/install 分别增长到约 `2.68x/2.38x/2.35x`；1M
warm 分别增长到约 `11.13x/8.02x/9.03x`。在当前 1M 档位没有观察到超线性的 export/install
恶化，峰值采样 RSS 仍低于 `400 MiB`。

1M warm/cold-advisory 的 export 分别为 `31.30s` 和 `31.26s`，几乎没有差异。这不表示物理冷
I/O 成本很低，而是约 445 MB 的测试状态仍不足以让 `POSIX_FADV_DONTNEED` 在这台 62 GiB
内存机器上构造可靠的设备冷读。真正的物理 I/O 评估仍需更大状态、独立块设备计数器，或在
机器重启后的受控首次运行中完成。

## 当前目标硬件 1M/5M release E2E 基线

2026-08-24 在 revision `a53c95f` 之上的未提交容量 harness 上，使用 release binaries 和
每笔 2,000 输出完成了 1M 控制组与 5M 下一档测试。`release_e2e` 的 `create` 是完整的
sync、seal、export 和内部校验流程；所有 artifact 都通过独立 verify、install 和 SHA-256
一致性校验。

| 指标 | 1M release | 5M release | 5M / 1M |
| --- | ---: | ---: | ---: |
| actual snapshot UTXO | 1,000,352 | 5,001,689 | 5.00x |
| snapshot bytes | 446,332,928 | 2,271,088,640 | 5.09x |
| chain load | 130.86s | 1,750.25s | 13.37x |
| create | 8.95s / 476.0 MiB | 48.22s / 572.9 MiB | 5.39x / 1.20x |
| verify | 1.23s / 16.6 MiB | 6.30s / 16.6 MiB | 5.14x / 1.00x |
| install | 1.91s / 380.8 MiB | 8.98s / 1,420.4 MiB | 4.70x / 3.73x |
| create process writes | 1.29 GB | 6.86 GB | 5.31x |
| create shared-device writes | 1.26 GB | 6.89 GB | 5.45x |

阶段单元格格式仍为 `elapsed / peak_rss_sampled`。原始 metrics 保存在：

```text
/tmp/usdb-bh-snapshot-capacity-1m-release-control-metrics.json
/tmp/usdb-bh-snapshot-capacity-5m-release-metrics.json
```

snapshot create、verify 和写入量随 UTXO 数量接近线性增长；5M install 峰值 RSS 上升到约
`1.39 GiB`，后续 10M/更大档位必须继续观察。当前主要瓶颈反而是 regtest 负载生成：5M
chain load 相对 1M 达到 `13.37x`，来源是反复调用 wallet funding/coin selection，而不是
snapshot 实现本身。直接扩大到 10M 会主要测量测试数据生成器，因此应先改造成显式管理
funding/change 的确定性交易生成器，再继续更大档位。

大批次最初还暴露了 Linux `ARG_MAX` 限制。容量脚本现已让
`createrawtransaction`、`fundrawtransaction`、`signrawtransactionwithwallet` 和
`sendrawtransaction` 通过 `bitcoin-cli -stdin` 传递大 JSON/hex，并以单笔 2,000 输出的
smoke 和上述 release 测试验证。

## 验收建议

同一硬件至少依次运行 1K、10K、100K、250K 和 1M 档位，并固定代码 revision、Bitcoin Core
版本、输出批大小、存储设备和 cache mode。正式结果应同时记录磁盘型号、文件系统、可用
内存、CPU、物理读写字节和运行前缓存状态。当前脚本提供可重复负载与应用层指标，但不会
自行冻结上线阈值。
