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
边界。

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
- 构造链上负载所需时间和 warm/cold-advisory 模式。

`peak_rss_kib_sampled` 是采样值，不是内核提供的完整进程树 high-water mark。脚本直接运行
预构建二进制，避免把 Cargo 编译开销计入阶段指标；它也不会统计 bitcoind 自身资源。

## 验收建议

同一硬件至少依次运行 1K、10K、100K 三档，并固定代码 revision、Bitcoin Core 版本、输出
批大小、存储设备和 cache mode。100K 正式结果应同时记录磁盘型号、文件系统、可用内存、
CPU、物理读写字节和运行前缓存状态。当前脚本提供可重复负载与应用层指标，但不会自行冻结
上线阈值。
