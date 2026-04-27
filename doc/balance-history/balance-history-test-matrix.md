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
| Ignored Rust real-data tests | `cargo test -p balance-history -- --ignored` | 否 | 本机 bitcoind 和本机 blk 文件 | BTC batch RPC、local loader、block file cache、真实 blk/RPC 对齐 | 已存在，但仍是手工、环境敏感测试 |
| Regtest scripts | `bash src/btc/balance-history/scripts/regtest_*.sh` | 否 | 本机 bitcoind binary | 端到端 smoke、reorg、snapshot install/recovery、RPC 语义、oracle balance 对拍 | 已存在，但还没有统一 runner |
| Web/browser consumers | `web/balance-history-browser` via hosted console or Vite | 否 | balance-history RPC proxy/service | UI 侧使用 summary/timeseries/flow/resolve RPC | 不作为服务正确性 gate |
| Performance/manual profiling | ignored Rust tests and runtime logs | 否 | 本机 blk 文件或 full node data | local loader 内存/吞吐、batch preloader timing counters | 仅手工使用 |

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
bash src/btc/balance-history/scripts/regtest_smoke.sh
bash src/btc/balance-history/scripts/regtest_rpc_semantics.sh
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

## 推荐手工套件

在统一 runner 落地前，按下面的手工套件执行。

### `smoke`

用于普通 RPC、UI 可见接口或非 reorg 服务改动：

```bash
bash src/btc/balance-history/scripts/regtest_smoke.sh
bash src/btc/balance-history/scripts/regtest_rpc_semantics.sh
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

以下 Rust 测试有意不进入默认套件，因为它们依赖本机 blk 文件和/或本机 bitcoind RPC：

| 范围 | 测试 | 依赖 |
| --- | --- | --- |
| BTC batch RPC | `btc::batch::tests::test_batch_get_blocks` | 本机 bitcoind RPC |
| Local loader index | `btc::local_loader::*_real` | 本机 bitcoind RPC + 本机 blk 文件 |
| Block file reader/cache | `btc::local_loader::*blk*`, `cache::block_file::tests::test_block_file_cache` | 本机 blk 文件 |
| Manual profiling | `btc::local_loader::tests::test_profile_blk_file_reader_memory_usage` | 本机 blk 文件 + 手工解读 |

当前问题：这些测试会从默认服务配置推导环境，尚未形成稳定命令契约。下一步应引入显式 env-gated 命令，例如：

```bash
USDB_BH_REAL_BTC=1 \
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
cargo test -p balance-history -- --ignored
```

实现上应在未设置 `USDB_BH_REAL_BTC` 时给出明确 skip 信息，并要求显式传入 `BTC_DATA_DIR`/RPC 设置，避免静默读取无关默认配置。

## 当前覆盖缺口

| 缺口 | 风险 | 建议修复 |
| --- | --- | --- |
| 没有统一 regtest runner | 脚本已存在，但不容易作为稳定套件执行 | 增加 `scripts/run_regtest_suite.sh`，支持 `smoke`、`core`、`reorg-full`、`snapshot-full` |
| 没有 crate-level integration tests | 多模块流程嵌在大型生产文件的 unit tests 中 | 从 lib 导出核心模块，并增加 `src/btc/balance-history/tests/` |
| 聚合 RPC 缺少 regtest 覆盖 | 浏览器依赖 summary/timeseries/flow，但 shell E2E 没有验证 | 扩展 `regtest_rpc_semantics.sh` 或新增 `regtest_aggregate_rpc_semantics.sh` |
| `resolve_script_hashes` 缺少 regtest 覆盖 | script registry 单测能通过，但完整 indexed data 路径可能失效 | 增加挖出可花费输出、调用 `resolve_script_hashes`、校验 address recovery 的 regtest |
| 真实 BTC local loader 测试仍是手工 | local blk 加速路径可能在无日常信号下退化 | 增加显式 real-data test mode，以及 fixture/regtest-generated blk subset 测试 |
| shell helper 重复 | 多个脚本重复定义 JSON assertion helper | 把通用 JSON assertion helper 移入 `regtest_lib.sh` |
| 大模块 ownership 不清晰 | DB/server/snapshot/block 文件过大，review 与补测成本高 | lib export 后拆分 helper，并把共享 test builders 移入 `tests/common` |

## 建议落地顺序

1. 整理当前脚本和测试分层文档。
2. 增加最小版 `run_regtest_suite.sh`，先执行现有脚本，不改内部实现。
3. 把通用 shell assertion helper 移入 `regtest_lib.sh`。
4. 重构 crate export，让 integration tests 可以调用核心模块，而不是依赖 `main.rs` 承载模块。
5. 增加生成式小链 Rust integration tests：same-block spends、multi-input spends、OP_RETURN output ignore、block commit continuity、reorg rollback。
6. 增加 aggregate RPC 和 `resolve_script_hashes` 的 regtest 覆盖。
7. 增加 local blk loader 与 BTC RPC 对齐的显式 real-data test mode。

## 验收标准

第一个稳定测试里程碑应满足：

- `cargo test -p balance-history` 仍然是默认快速检查。
- `scripts/run_regtest_suite.sh smoke` 可以无手工端口编辑地执行文档化子集。
- `scripts/run_regtest_suite.sh core` 覆盖普通同步、RPC 语义、一次 reorg、一次 snapshot install 和 oracle balance comparison。
- 每个新增 balance-history RPC 至少有一个 unit test 和一个 regtest-level consumer test。
- 真实 BTC 数据测试必须显式 opt-in，不能意外依赖开发者默认配置。
