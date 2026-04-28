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
| Rust unit tests | `cargo test -p balance-history` | 是 | 无 | DB primitives、RPC 语义、block commit helpers、rollback metadata、snapshot helpers、readiness、script registry unit paths | 本地可运行并已通过 |
| Real BTC data tests | `USDB_BH_REAL_BTC=1 ... bash src/btc/balance-history/scripts/run_real_btc_tests.sh loader-index --size tiny` | 否 | 本机 bitcoind 和本机 blk 文件 | local loader、block file reader/cache、真实 blk/RPC 对齐 | 显式 env-gated，支持 suite/size 切片 |
| Regtest scripts | `bash src/btc/balance-history/scripts/regtest_*.sh` | 否 | 本机 bitcoind binary | 端到端 smoke、reorg、snapshot install/recovery、RPC 语义、oracle balance 对拍 | 已存在，但还没有统一 runner |
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
```

修改 reorg、rollback、snapshot 或 local-loader 行为时执行：

```bash
cd /home/bucky/work/usdb
bash src/btc/balance-history/scripts/regtest_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh
bash src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

## Regtest 分层

| 分层 | 使用场景 | 脚本 |
| --- | --- | --- |
| Smoke | 普通服务/RPC 改动后的快速信心测试 | `regtest_smoke.sh`, `regtest_rpc_semantics.sh` |
| Reorg smoke | canonical rollback 和 reorg detection 检查 | `regtest_reorg_smoke.sh`, `regtest_multi_reorg_smoke.sh`, `regtest_deep_reorg_smoke.sh` |
| Restart/recovery reorg | 服务离线或重启后的 reorg 恢复 | `regtest_restart_reorg_smoke.sh`, `regtest_restart_multi_reorg_smoke.sh`, `regtest_restart_hybrid_reorg_smoke.sh` |
| Query semantics | balance、delta、batch query、spend graph、same-block aggregation | `regtest_spend_graph_queries.sh`, `regtest_multi_input_same_block_queries.sh`, `regtest_restart_same_block_aggregate_reorg.sh` |
| Undo retention | retained undo window 内的 reorg 行为 | `regtest_undo_retention_reorg.sh`, `regtest_undo_retention_same_block_aggregate_reorg.sh` |
| Snapshot | snapshot export/install/recovery/failure 语义 | `regtest_snapshot_recovery.sh`, `regtest_snapshot_restart_recovery.sh`, `regtest_snapshot_install_repeat.sh`, `regtest_snapshot_install_retry.sh`, `regtest_snapshot_install_failure.sh`, `regtest_snapshot_install_corrupt.sh`, `regtest_snapshot_install_downgrade.sh` |
| Oracle | 用独立 oracle 对拍生成的 regtest block 历史余额 | `regtest_history_balance_oracle.sh` |
| Loader threshold | RPC/local-loader 切换行为 | `regtest_loader_switch.sh` |

## 推荐套件

当前已有最小版 `run_regtest_suite.sh`，先收敛 `smoke` 子集。其它更大套件仍按下面的手工命令执行。

### `smoke`

用于普通 RPC、UI 可见接口、基础 reorg、snapshot repeat install 和 oracle balance 对拍：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke
```

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
```

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
| 统一 regtest runner 仍不完整 | 当前只收敛了 `smoke` 子集，更大套件仍需手工执行 | 扩展 `scripts/run_regtest_suite.sh`，继续支持 `core`、`reorg-full`、`snapshot-full` |
| 没有 crate-level integration tests | 多模块流程嵌在大型生产文件的 unit tests 中 | 从 lib 导出核心模块，并增加 `src/btc/balance-history/tests/` |
| 聚合 RPC 缺少 regtest 覆盖 | 浏览器依赖 summary/timeseries/flow，但 shell E2E 没有验证 | 扩展 `regtest_rpc_semantics.sh` 或新增 `regtest_aggregate_rpc_semantics.sh` |
| `resolve_script_hashes` 缺少 regtest 覆盖 | script registry 单测能通过，但完整 indexed data 路径可能失效 | 增加挖出可花费输出、调用 `resolve_script_hashes`、校验 address recovery 的 regtest |
| 真实 BTC local loader 测试仍需人工提供节点 | local blk 加速路径可能在无日常信号下退化 | 已有显式 real-data test mode；下一步补 fixture/regtest-generated blk subset，让 CI 也能覆盖 local-loader 子集 |
| shell helper 重复 | 多个脚本重复定义 JSON assertion helper | 把通用 JSON assertion helper 移入 `regtest_lib.sh` |
| 大模块 ownership 不清晰 | DB/server/snapshot/block 文件过大，review 与补测成本高 | lib export 后拆分 helper，并把共享 test builders 移入 `tests/common` |

## 建议落地顺序

1. 整理当前脚本和测试分层文档。
2. 增加最小版 `run_regtest_suite.sh`，先收敛 `smoke` 子集，不改现有脚本内部实现。
3. 把通用 shell assertion helper 移入 `regtest_lib.sh`。
4. 重构 crate export，让 integration tests 可以调用核心模块，而不是依赖 `main.rs` 承载模块。
5. 增加生成式小链 Rust integration tests：same-block spends、multi-input spends、OP_RETURN output ignore、block commit continuity、reorg rollback。
6. 增加 aggregate RPC 和 `resolve_script_hashes` 的 regtest 覆盖。
7. 增加 local blk loader 与 BTC RPC 对齐的显式 real-data test mode。
8. 增加 fixture/regtest-generated blk subset，降低真实主网数据测试对本机节点的依赖。

## 验收标准

第一个稳定测试里程碑应满足：

- `cargo test -p balance-history` 仍然是默认快速检查。
- `scripts/run_regtest_suite.sh smoke` 可以无手工端口编辑地执行文档化子集：`regtest_smoke.sh`、`regtest_rpc_semantics.sh`、`regtest_reorg_smoke.sh`、`regtest_snapshot_install_repeat.sh`、`regtest_history_balance_oracle.sh`。
- `scripts/run_regtest_suite.sh core` 覆盖普通同步、RPC 语义、一次 reorg、一次 snapshot install 和 oracle balance comparison。
- 每个新增 balance-history RPC 至少有一个 unit test 和一个 regtest-level consumer test。
- 真实 BTC 数据测试必须显式 opt-in，不能意外依赖开发者默认配置。
