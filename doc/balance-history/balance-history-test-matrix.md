# Balance-History 测试矩阵

本文档定义 `balance-history` 的测试分层、当前覆盖状态和后续补齐顺序。它的目标不是替代单个 regtest 场景文档，而是给开发者一个统一入口：改动某类逻辑时，应该跑哪些测试、哪些测试现在还只是手工验证、哪些缺口需要优先补。

## 范围

`balance-history` 负责从 BTC 链构建以下状态：

- address/script-hash balance history
- live UTXO cache
- block commit chain
- snapshot export/install/recovery
- readiness and consensus state reference RPCs
- auxiliary script registry
- local blk file loader acceleration

测试覆盖必须同时关注三条路径：

- 纯逻辑：不依赖 bitcoind，默认 `cargo test` 能跑。
- regtest 端到端：启动真实 bitcoind regtest，验证服务 RPC、reorg、snapshot、oracle 对拍。
- 真实本地数据：读取本机 BTC blk 文件和真实 RPC，用于验证 local loader 与主网数据兼容性。

## 当前测试分层

| 分层 | 当前入口 | 默认执行 | 外部服务 | 主要覆盖 | 当前状态 |
| --- | --- | --- | --- | --- | --- |
| Rust unit tests | `cargo test -p balance-history -p balance-history-snapshot-tool` | 是 | 无 | DB primitives、RPC 语义、block commit helpers、rollback metadata、snapshot helpers、exact-height builder state、readiness、script registry unit paths | 本地可运行并已通过 |
| Real BTC data tests | `USDB_BH_REAL_BTC=1 ... bash src/btc/balance-history/scripts/run_real_btc_tests.sh loader-index --size tiny` | 否 | 本机 bitcoind 和本机 blk 文件 | local loader、block file reader/cache、真实 blk/RPC 对齐 | 显式 env-gated，支持 suite/size 切片 |
| Regtest scripts | `bash src/btc/balance-history/scripts/run_regtest_suite.sh <suite>` | 否 | 本机 bitcoind binary | 端到端 smoke、独立历史状态线、stable-lag reorg boundary、snapshot install/recovery、RPC 语义 | 已有 `smoke`、`correctness` 和 `stable-lag-reorg` runner；更大套件仍为手工入口 |
| Web/browser consumers | `web/balance-history-browser` via hosted console or Vite | 否 | balance-history RPC proxy/service | UI 侧使用 summary/timeseries/flow/resolve RPC | 不作为服务正确性 gate |
| Performance/manual profiling | `USDB_BH_REAL_BTC=1 ... bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-cache --size tiny` | 否 | 本机 blk 文件或 full node data | local loader 内存/吞吐、block file cache prefetch | 仅手工使用，支持横向抽样 |

## 基线命令

普通逻辑改动至少执行：

```bash
cd /home/bucky/work/usdb/src/btc
cargo test -p balance-history
cargo clippy -p balance-history --all-targets
```

修改 shell 脚本或 regtest 可见行为时执行：

```bash
cd /home/bucky/work/usdb
bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke
bash src/btc/balance-history/scripts/run_regtest_suite.sh correctness
```

修改 reorg、rollback、snapshot 或 local-loader 行为时执行：

```bash
cd /home/bucky/work/usdb
bash src/btc/balance-history/scripts/run_regtest_suite.sh stable-lag-reorg
bash src/btc/balance-history/scripts/regtest_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh
bash src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

## Regtest 分层

| 分层 | 使用场景 | 脚本 |
| --- | --- | --- |
| Smoke | 普通服务/RPC 改动后的快速信心测试 | `regtest_smoke.sh`, `regtest_rpc_semantics.sh` |
| Reorg smoke | canonical rollback 和 reorg detection 检查 | `regtest_reorg_smoke.sh`, `regtest_multi_reorg_smoke.sh`, `regtest_deep_reorg_smoke.sh` |
| Stable-lag reorg boundary | `depth=lag-1/lag/lag+1` 下 online、offline restart、fresh joiner 收敛 | `regtest_stable_lag_reorg_depth_matrix.sh` |
| Restart/recovery reorg | 服务离线或重启后的 reorg 恢复 | `regtest_restart_reorg_smoke.sh`, `regtest_restart_multi_reorg_smoke.sh`, `regtest_restart_hybrid_reorg_smoke.sh` |
| Query semantics | balance、delta、batch query、spend graph、same-block aggregation | `regtest_spend_graph_queries.sh`, `regtest_multi_input_same_block_queries.sh`, `regtest_restart_same_block_aggregate_reorg.sh` |
| Undo retention | retained undo window 内的 reorg 行为 | `regtest_undo_retention_reorg.sh`, `regtest_undo_retention_same_block_aggregate_reorg.sh` |
| Snapshot | snapshot export/install/recovery/failure、exact-height resume/reorg/continued sync、签名信任与容量语义 | `regtest_snapshot_recovery.sh`, `regtest_snapshot_restart_recovery.sh`, `regtest_snapshot_install_repeat.sh`, `regtest_snapshot_install_retry.sh`, `regtest_snapshot_install_failure.sh`, `regtest_snapshot_install_corrupt.sh`, `regtest_snapshot_install_downgrade.sh`, `regtest_exact_height_snapshot_tool.sh`, `regtest_exact_height_snapshot_restart.sh`, `regtest_exact_height_snapshot_same_height_reorg.sh`, `regtest_exact_height_snapshot_install_spend.sh`, `regtest_exact_height_snapshot_failure_paths.sh`, `regtest_exact_height_snapshot_signed_install.sh`, `regtest_exact_height_snapshot_capacity.sh` |
| Oracle | 用独立 oracle 对拍生成的 regtest block 历史余额 | `regtest_history_balance_oracle.sh` |
| Loader threshold | RPC/local-loader 切换行为 | `regtest_loader_switch.sh` |

## 推荐套件

当前已有最小版 `run_regtest_suite.sh`，先收敛 `smoke` 子集。其它更大套件仍按下面的手工命令执行。

### `smoke`

用于普通 RPC、UI 可见接口、基础 reorg、snapshot repeat install 和 oracle balance 对拍：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke
```

### `stable-lag-reorg`

用于涉及 stable target、reorg wake-up、startup reconciliation、historical state-ref 或
fresh bootstrap 的改动：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh stable-lag-reorg
```

该 suite 覆盖 `depth=lag-1/lag/lag+1`，每个深度交叉验证 online、offline restart 和
fresh joiner。详细测试契约见
[balance-history-regtest-stable-lag-reorg-depth-matrix.md](./balance-history-regtest-stable-lag-reorg-depth-matrix.md)。

### `correctness`

用于涉及 balance/delta、UTXO、script registry、block commit、stable-lag 查询边界或
历史 RPC 的改动：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh correctness
```

该 suite 先运行独立 oracle 单测，再使用真实 bitcoind 执行 RPC、spend graph、同块聚合、
完整历史状态线和 stable-lag smoke。编译在 suite 开始前单独完成，不计入服务 readiness。
更大的 32-address/120-block 状态线和主网外部审计流程见
[balance-history-correctness-validation.md](./balance-history-correctness-validation.md)。

### `core`

用于涉及 DB 写入、RPC 查询语义、block commit 或 readiness 的改动：

```bash
bash src/btc/balance-history/scripts/regtest_smoke.sh
bash src/btc/balance-history/scripts/regtest_rpc_semantics.sh
bash src/btc/balance-history/scripts/regtest_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh
bash src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

### `reorg-full`

用于涉及 rollback、undo retention、block commit chain 或 local sync loop 的改动：

```bash
bash src/btc/balance-history/scripts/regtest_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_multi_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_deep_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_restart_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_restart_multi_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_restart_hybrid_reorg_smoke.sh
bash src/btc/balance-history/scripts/run_regtest_suite.sh stable-lag-reorg
bash src/btc/balance-history/scripts/regtest_undo_retention_reorg.sh
bash src/btc/balance-history/scripts/regtest_undo_retention_same_block_aggregate_reorg.sh
```

### `snapshot-full`

用于涉及 snapshot metadata、manifest、install、readiness 或 recovery 的改动：

```bash
bash src/btc/balance-history/scripts/regtest_snapshot_recovery.sh
bash src/btc/balance-history/scripts/regtest_snapshot_restart_recovery.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_retry.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_failure.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_corrupt.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_downgrade.sh
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_tool.sh
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_restart.sh
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_same_height_reorg.sh
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_install_spend.sh
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_failure_paths.sh
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_signed_install.sh
```

Snapshot scripts derive the confirmation count from the running service's `stable_lag` value.
Installer tests use the adjacent manifest, or an explicitly modified manifest for negative cases;
the removed legacy `--hash` option is not part of the test contract.

容量入口不属于默认 `snapshot-full` 回归；按数据档位单独执行：

```bash
SNAPSHOT_CAPACITY_UTXOS=1000 \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh

SNAPSHOT_CAPACITY_UTXOS=100000 SNAPSHOT_CAPACITY_COLD_CACHE=1 \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh
```

详细指标约束见
[balance-history-exact-height-snapshot-capacity.md](./balance-history-exact-height-snapshot-capacity.md)。

## 真实 BTC 数据测试

以下 Rust 测试有意不进入默认套件，因为它们依赖本机 blk 文件和/或本机 bitcoind RPC。它们不再使用 `#[ignore]`，而是由 `USDB_BH_REAL_BTC=1` 打开 `cfg(usdb_bh_real_btc)` 后才编译。

| 范围 | 测试 | 依赖 |
| --- | --- | --- |
| Local loader index | `real_btc_correctness_local_loader_build_index_matches_rpc_on_sample_heights` | 本机 bitcoind RPC + 本机 blk 文件 |
| Persisted local-loader index | `real_btc_correctness_restore_block_index_from_db`, `real_btc_correctness_build_index_rebuilds_after_corrupted_persisted_state` | 本机 bitcoind RPC + 本机 blk 文件 |
| Block file reader/cache | `real_btc_correctness_read_blk_blocks_matches_direct_reader_on_subset_files`, `real_btc_correctness_block_file_cache_*` | 本机 blk 文件 |
| Latest complete blk RPC parity | `real_btc_correctness_latest_complete_blk_file_blocks_are_available_via_rpc` | 本机 bitcoind RPC + 本机 blk 文件 |
| Manual profiling | `real_btc_profile_blk_file_reader_memory_usage`, `real_btc_profile_block_file_cache_prefetch_sample_range` | 本机 blk 文件 + 手工解读 |

快速 correctness 命令：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh loader-index --size tiny
```

快速 profile 命令：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-cache --size tiny
```

这些命令要求显式传入 `BTC_DATA_DIR` 和 `BTC_RPC_URL`，避免静默读取开发者默认配置。`BTC_COOKIE_FILE` 可替换为 `BTC_RPC_USER` / `BTC_RPC_PASSWORD`。`run_real_btc_tests.sh` 支持 `--size tiny|small|medium|large|full`，其中 correctness 子集始终从 `blk00000.dat` 开始以保证链连续；profile 可通过 `USDB_BH_REAL_BTC_PROFILE_START_FILE` 横向抽样任意 blk 文件段。

## 当前覆盖缺口

| 缺口 | 风险 | 建议修复 |
| --- | --- | --- |
| 统一 regtest runner 仍不完整 | 当前收敛了 `smoke`、`correctness` 和 `stable-lag-reorg`，其它更大套件仍需手工执行 | 扩展 `scripts/run_regtest_suite.sh`，继续支持 `core`、`reorg-full`、`snapshot-full` |
| Exact-height snapshot 尚无生产硬件结果与磁盘故障注入 | 已有 1K 可重复容量入口、签名篡改/非信任 signer、发布前失败和恢复覆盖，但尚未记录 100K 正式硬件结果 | 在目标硬件跑 1K/10K/100K warm/cold-advisory 矩阵，并补磁盘满与设备级物理 I/O 采样 |
| 没有 crate-level integration tests | 多模块流程嵌在大型生产文件的 unit tests 中 | 从 lib 导出核心模块，并增加 `src/btc/balance-history/tests/` |
| timeseries/flow bucket 聚合仍缺 regtest 覆盖 | Oracle 已对拍 movement range 和 summary，但浏览器使用的 bucket 形状仍主要靠 Rust 测试 | 扩展 oracle 或新增 aggregate RPC 场景 |
| Bitcoin Core UTXO 抽样尚无主网留档 | 工具的单元测试和真实 regtest 已通过，但本批未在同时运行的主网 Core 与 balance-history 上生成正式报告 | 两个主网服务 query-ready 后用固定 seed 执行一次并保存 JSON 报告、耗时和节点版本 |
| 主网任意高度历史审计仍依赖慢 Electrs 路径 | Bitcoin Core `scantxoutset/gettxout` 当前断面抽样已落地，但不能证明任意历史高度 movement/history | Electrs 恢复后增加全局 tx cache、批量/并发和 resume，再评估 sampled block replay oracle |
| 真实 BTC local loader 测试仍需人工提供节点 | local blk 加速路径可能在无日常信号下退化 | 已有显式 real-data test mode；下一步补 fixture/regtest-generated blk subset，让 CI 也能覆盖 local-loader 子集 |
| shell helper 重复 | 共享库已有 JSON assertion helper，但部分旧脚本仍保留本地副本 | 后续触及对应旧脚本时删除本地副本并复用 `regtest_lib.sh` |
| 大模块 ownership 不清晰 | DB/server/snapshot/block 文件过大，review 与补测成本高 | lib export 后拆分 helper，并把共享 test builders 移入 `tests/common` |

## 建议落地顺序

1. 将 `correctness` 默认档位接入 fast/manual gate，scale 档位接入后续 nightly/soak。
2. 增加 timeseries/flow bucket 的独立 oracle 对拍。
3. 扩展 runner 的 `reorg-full` 与 `snapshot-full` 套件。
4. 发布前运行 Bitcoin Core UTXO 抽样；恢复 Electrs 后优化历史外部审计。
5. 增加 fixture/regtest-generated blk subset，降低真实主网数据测试对本机节点的依赖。

## 验收标准

第一个稳定测试里程碑应满足：

- `cargo test -p balance-history` 仍然是默认快速检查。
- `scripts/run_regtest_suite.sh smoke` 可以无手工端口编辑地执行文档化子集：`regtest_smoke.sh`、`regtest_rpc_semantics.sh`、`regtest_reorg_smoke.sh`、`regtest_snapshot_install_repeat.sh`、`regtest_history_balance_oracle.sh`。
- `scripts/run_regtest_suite.sh stable-lag-reorg` 可以确定性覆盖 `depth=lag-1/lag/lag+1`，并要求 online、offline restart、fresh joiner 的最终状态一致。
- `scripts/run_regtest_suite.sh correctness` 可以确定性对拍 balance/delta/range/summary、
  live/spent UTXO、script registry 和 block commit，并实际验证 Bitcoin Core
  `scantxoutset/gettxout` 当前断面抽样。
- `scripts/run_regtest_suite.sh core` 覆盖普通同步、RPC 语义、一次 reorg、一次 snapshot install 和 oracle balance comparison。
- 每个新增 balance-history RPC 至少有一个 unit test 和一个 regtest-level consumer test。
- 真实 BTC 数据测试必须显式 opt-in，不能意外依赖开发者默认配置。
