# Balance-History Real BTC Data Tests

本文档记录基于本机真实 Bitcoin Core 节点和本地 blk 文件的 `balance-history` 测试入口。它和 regtest smoke 的目标不同：regtest 负责可重复的小链行为验证，real BTC data tests 负责验证主网/真实数据形态下的 local blk loader、block file cache 和 RPC 对齐。

## 执行方式

默认 `cargo test -p balance-history` 不会编译这些测试。只有显式设置 `USDB_BH_REAL_BTC=1` 时，`balance-history/build.rs` 才会打开 `cfg(usdb_bh_real_btc)`。

小切片正确性测试：

```bash
cd /home/bucky/work/usdb
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh correctness --size tiny
```

小切片性能/profile：

```bash
cd /home/bucky/work/usdb
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile --size tiny
```

可选环境变量：

| 变量 | 用途 |
| --- | --- |
| `BTC_COOKIE_FILE` | Bitcoin Core cookie 文件。未设置时默认使用 `$BTC_DATA_DIR/.cookie` |
| `BTC_RPC_USER` / `BTC_RPC_PASSWORD` | 使用 user/pass auth 时替代 cookie |
| `BTC_NETWORK` | `bitcoin`、`testnet`、`regtest`、`signet`、`testnet4`，默认 `bitcoin` |
| `BTC_BLOCK_MAGIC` | 覆盖 blk 文件 magic，例如 `0xD9B4BEF9` |
| `USDB_BH_REAL_BTC_SUBSET_FILE_COUNT` | correctness local-loader 子集 blk 文件数量。必须从 `blk00000.dat` 开始，保证链连续 |
| `USDB_BH_REAL_BTC_PROFILE_START_FILE` | profile 起始 blk 文件编号，可用于横向抽样最新或中间文件 |
| `USDB_BH_REAL_BTC_PROFILE_FILE_COUNT` | profile 读取的 blk 文件数量 |
| `USDB_BH_REAL_BTC_CACHE_SLEEP_MS` | profile cache 每次读取后的等待时间，用于观察 prefetch |

## Suite 与 Size

runner 支持两种切片维度：

- 横向 suite：只跑某一类能力，例如 `loader-index`、`loader-restore`、`blk-reader`、`block-cache`、`latest-rpc`、`profile-reader`、`profile-cache`。
- 纵向 size：控制真实数据规模，例如 `tiny`、`small`、`medium`、`large`、`full`。

| Size | correctness subset | profile 文件数 | 典型用途 |
| --- | --- | --- | --- |
| `tiny` | 2 个从 genesis 开始的 blk 文件 | 1 个 blk 文件 | 日常快速冒烟。local-loader 会排除最后一个可能仍在写入的 blk 文件，所以 correctness 最小为 2 |
| `small` | 4 个从 genesis 开始的 blk 文件 | 4 个 blk 文件 | 默认人工检查 |
| `medium` | 8 个从 genesis 开始的 blk 文件 | 16 个 blk 文件 | 局部性能观察 |
| `large` | 16 个从 genesis 开始的 blk 文件 | 64 个 blk 文件 | 长一些的性能/兼容性检查 |
| `full` | 32 个从 genesis 开始的 blk 文件 | 256 个 blk 文件 | 手工长跑，不建议日常默认使用 |

查看某个 suite 会执行哪些 test filter：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh loader-restore --size tiny --list
```

推荐日常命令：

```bash
USDB_BH_REAL_BTC=1 BTC_DATA_DIR=/home/bucky/.bitcoin BTC_RPC_URL=http://127.0.0.1:8332 BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh loader-index --size tiny

USDB_BH_REAL_BTC=1 BTC_DATA_DIR=/home/bucky/.bitcoin BTC_RPC_URL=http://127.0.0.1:8332 BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-cache --size tiny
```

抽样最新附近 blk 文件做 profile：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
USDB_BH_REAL_BTC_PROFILE_START_FILE=3600 \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-cache --size small
```

## 当前覆盖

| 范围 | 测试名过滤 | 验证点 |
| --- | --- | --- |
| local loader 与 RPC 对齐 | `real_btc_correctness_local_loader_build_index_matches_rpc_on_sample_heights` | 从真实 blk 子集构建 index，并在 genesis、中点、tip 子集高度上对齐 RPC hash/body |
| 持久化 index 恢复 | `real_btc_correctness_restore_block_index_from_db` | local loader 从 DB 恢复 block index 后仍可对齐 RPC |
| 脏持久化 index 重建 | `real_btc_correctness_build_index_rebuilds_after_corrupted_persisted_state` | 写入不连续 block heights 后，loader 能清理并重建 |
| blk reader 一致性 | `real_btc_correctness_read_blk_blocks_matches_direct_reader_on_subset_files` | record reader 与 block loader 对同一 blk 文件给出一致首尾 block |
| 最新完整 blk 文件对齐 | `real_btc_correctness_latest_complete_blk_file_blocks_are_available_via_rpc` | 本地最新完整 blk 文件样本可通过 RPC 按 hash 取回 |
| block file cache | `real_btc_correctness_block_file_cache_*` | cache 重复读取和跨文件读取与 reader 一致 |
| profile | `real_btc_profile_*` | 观察 blk reader 内存和 block file cache prefetch 行为 |

## 现状评估

这些测试适合作为“真实数据兼容性”和“性能回归调查”的人工 gate，但还不适合作为 CI gate。主要原因是它们依赖本机 Bitcoin Core datadir、RPC auth、节点同步状态和磁盘布局。

正确性方面，目前最有价值的是 local loader 与 RPC 对齐、持久化 block index 恢复和脏状态重建。这些路径直接影响初次索引和重启恢复。

性能方面，目前只有手工 profile 入口。后续如果要做稳定性能报告，应把指标输出结构化，例如：blk 文件数、block 数、读取耗时、峰值 RSS、cache hit/miss、prefetch 队列命中率。否则只能依赖日志肉眼判断。

## 后续建议

1. 增加 fixture/regtest-generated blk subset，使 CI 可以覆盖小规模 local-loader 路径，不依赖开发者主网数据。
2. 把 profile 输出改成 JSONL，便于后续多次运行对比。
3. 增加大区间 benchmark runner，固定 block file 范围和 batch size，观察吞吐和内存。
4. 如果重新启用 `btc/batch.rs` 的 batch RPC client，需要先把该模块纳入 crate module tree，并明确它和当前 `BTCRpcClient` 的职责边界。
