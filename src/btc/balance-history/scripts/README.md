# Balance-History Regtest Scripts

本目录包含 `balance-history` 的 shell 级端到端测试。每个场景会启动隔离的 Bitcoin Core regtest 节点，启动一个或多个 `balance-history` 服务实例，构造链上交易/区块，并通过 JSON-RPC 验证结果。

当前 `run_regtest_suite.sh` 支持 smoke 和 stable-lag reorg boundary 两个子集；更大的 reorg/snapshot 套件仍保留为手工入口。

真实本地 BTC 数据测试使用单独入口 `run_real_btc_tests.sh`，它不会启动 bitcoind，而是连接调用者显式指定的本机节点和 blk datadir。

## 前置依赖

- `cargo`
- `curl`
- `python3`
- Bitcoin Core `bitcoind` 和 `bitcoin-cli`

脚本默认优先查找：

```bash
/home/bucky/btc/bitcoin-28.1/bin
```

如需覆盖：

```bash
BITCOIN_BIN_DIR=/path/to/bitcoin/bin bash src/btc/balance-history/scripts/regtest_smoke.sh
```

如果 `BITCOIN_BIN_DIR` 下没有完整的 `bitcoind` 和 `bitcoin-cli`，共享库会回退到 `PATH`。

## 快速开始

从仓库根目录执行：

```bash
bash src/btc/balance-history/scripts/regtest_smoke.sh
bash src/btc/balance-history/scripts/regtest_rpc_semantics.sh
```

也可以直接运行当前推荐 smoke 子集：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke
```

Stable-lag reorg 深度边界矩阵：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh stable-lag-reorg
```

脚本默认在 `/tmp` 下创建临时工作目录，并在退出时清理。失败时会自动打印 bitcoind 和 balance-history 日志尾部。

## 常用环境变量

| 变量 | 用途 | 默认值 |
| --- | --- | --- |
| `WORK_DIR` | 单次场景运行的根目录 | `mktemp -d /tmp/usdb-bh-...` |
| `BITCOIN_DIR` | Bitcoin Core datadir | `$WORK_DIR/bitcoin` |
| `BITCOIN_BIN_DIR` | `bitcoind` 和 `bitcoin-cli` 所在目录 | `/home/bucky/btc/bitcoin-28.1/bin` |
| `BALANCE_HISTORY_ROOT` | balance-history 服务 root dir | `$WORK_DIR/balance-history` |
| `BTC_RPC_PORT` | bitcoind RPC 端口 | 场景自带默认值 |
| `BTC_P2P_PORT` | bitcoind P2P 端口 | 场景自带默认值 |
| `BH_RPC_PORT` | balance-history JSON-RPC 端口 | 场景自带默认值 |
| `WALLET_NAME` | regtest 钱包名 | 场景自带默认值 |
| `SYNC_TIMEOUT_SEC` | 等待同步/readiness 的超时时间 | 通常为 `120` |
| `BALANCE_HISTORY_LOG_FILE` | service stdout/stderr 捕获文件 | `$WORK_DIR/balance-history.log` |
| `REGTEST_DIAG_TAIL_LINES` | 失败时打印日志行数 | `120` |

## 脚本清单

| 脚本 | 分层 | 默认端口 `btc-rpc/p2p/bh-rpc` | 目标 |
| --- | --- | --- | --- |
| `run_regtest_suite.sh` | Runner | N/A | 运行预定义 regtest 套件，当前支持 `smoke`、`stable-lag-reorg` |
| `run_real_btc_tests.sh` | Real BTC | N/A | 显式 `USDB_BH_REAL_BTC=1` 后运行本机真实 blk/RPC 测试 |
| `regtest_smoke.sh` | Smoke | `28132/28133/28110` | 基础同步、网络类型、地址余额查询 |
| `regtest_rpc_semantics.sh` | Smoke/query | `29032/29033/29010` | latest/exact/range balance、delta、batch 顺序、live UTXO 语义 |
| `regtest_reorg_smoke.sh` | Reorg | `28232/28233/28210` | 基础 reorg rollback 和 block commit 恢复 |
| `regtest_multi_reorg_smoke.sh` | Reorg | `28332/28333/28310` | 多轮连续 reorg |
| `regtest_deep_reorg_smoke.sh` | Reorg | `28432/28433/28410` | 更深 rollback 覆盖 |
| `regtest_restart_reorg_smoke.sh` | Reorg/restart | `28532/28533/28510` | 服务离线 reorg 后重启恢复 |
| `regtest_restart_multi_reorg_smoke.sh` | Reorg/restart | `28632/28633/28610` | 多轮离线 reorg |
| `regtest_restart_hybrid_reorg_smoke.sh` | Reorg/restart | `28732/28733/28710` | 在线/离线混合 reorg |
| `regtest_stable_lag_smoke.sh` | Readiness | `29832/29833/29810` | stable lag 和 consensus-ready 行为 |
| `regtest_stable_lag_reorg_depth_case.sh` | Stable-lag/reorg | `30732/30733/30710-30712` | 单深度下 online、offline restart、fresh joiner 对拍 |
| `regtest_stable_lag_reorg_depth_matrix.sh` | Stable-lag/reorg | `30710-30813` | `depth=lag-1/lag/lag+1` 完整边界矩阵 |
| `regtest_history_balance_oracle.sh` | Oracle | `28932/28933/28910` | 用独立 Python oracle 对拍随机地址历史余额 |
| `regtest_spend_graph_queries.sh` | Query | `30532/30533/30510` | 多地址 spend graph 查询一致性 |
| `regtest_multi_input_same_block_queries.sh` | Query | `30632/30633/30610` | 同块多输入聚合和 batch delta 查询 |
| `regtest_restart_same_block_aggregate_reorg.sh` | Query/reorg | `30652/30653/30630` | 离线 reorg 后同块聚合状态 |
| `regtest_undo_retention_reorg.sh` | Undo retention | `30332/30333/30310` | retained undo window 内的 reorg |
| `regtest_undo_retention_same_block_aggregate_reorg.sh` | Undo retention | `30672/30673/30650` | retained-window reorg 加同块聚合 delta |
| `regtest_loader_switch.sh` | Loader | `30432/30433/30410` | RPC loader 与 local loader 阈值切换行为 |
| `regtest_snapshot_recovery.sh` | Snapshot | `29232/29233/29210` | snapshot 创建、安装和继续查询 |
| `regtest_snapshot_restart_recovery.sh` | Snapshot | `29632/29633/29610` | snapshot recovery 后重启 |
| `regtest_snapshot_install_repeat.sh` | Snapshot | `30132/30133/30110` | 重复安装幂等性 |
| `regtest_snapshot_install_retry.sh` | Snapshot | `29732/29733/29710` | 安装失败后的重试 |
| `regtest_snapshot_install_failure.sh` | Snapshot | `29432/29433/29410` | 安装失败不污染 live state |
| `regtest_snapshot_install_corrupt.sh` | Snapshot | `30232/30233/30210` | 损坏 snapshot 拒绝安装 |
| `regtest_snapshot_install_downgrade.sh` | Snapshot | `30032/30033/30010` | 旧 snapshot/downgrade 安装保护 |
| `regtest_exact_height_snapshot_tool.sh` | Snapshot | `29832/29833/29810` | exact-height 初次创建、幂等重放和增量 H+1 |
| `regtest_exact_height_snapshot_restart.sh` | Snapshot | `29932/29933/29910` | 所有 durable checkpoint 的跨进程恢复 |
| `regtest_exact_height_snapshot_same_height_reorg.sh` | Snapshot/reorg | `30032/30033/30010` | 同高度分支替换、歧义拒绝和新分支继续同步 |
| `regtest_exact_height_snapshot_install_spend.sh` | Snapshot/install | `30132/30133/30110-30111` | 安装后花费快照前 UTXO 并继续追块 |
| `regtest_exact_height_snapshot_failure_paths.sh` | Snapshot | `30332/30333/N/A` | exact-height 参数拒绝、发布前失败、恢复和发布后篡改 |
| `regtest_exact_height_snapshot_signed_install.sh` | Snapshot | `30432/30433/30411-30413` | 签名篡改、非信任 signer 和受信任安装 provenance |
| `regtest_exact_height_snapshot_capacity.sh` | Snapshot/capacity | `30932/30933/N/A` | 参数化 1K/10K/100K UTXO 导出、校验和安装指标 |
| `mainnet_exact_height_snapshot.sh` | Snapshot/production | N/A | 主网 exact-height snapshot 的初始化、预检、断点续跑、signed validation 和打包入口 |

## 推荐手工套件

### Smoke

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke
```

当前 runner 的 `smoke` 子集包含：

- `regtest_smoke.sh`
- `regtest_rpc_semantics.sh`
- `regtest_reorg_smoke.sh`
- `regtest_snapshot_install_repeat.sh`
- `regtest_history_balance_oracle.sh`

可用以下命令查看实际脚本列表：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke --list
```

### Core

```bash
bash src/btc/balance-history/scripts/regtest_smoke.sh
bash src/btc/balance-history/scripts/regtest_rpc_semantics.sh
bash src/btc/balance-history/scripts/regtest_reorg_smoke.sh
bash src/btc/balance-history/scripts/regtest_snapshot_install_repeat.sh
bash src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

### Stable-Lag Reorg Boundary

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh stable-lag-reorg
```

该 suite 在三个隔离 regtest 上依次验证 `depth=lag-1`、`depth=lag` 和
`depth=lag+1`。每个案例交叉比较 online、offline restart、fresh joiner 的 snapshot、
historical state-ref、block commit 和 tracked address balance。详细说明见
[balance-history-regtest-stable-lag-reorg-depth-matrix.md](/home/bucky/work/usdb/doc/balance-history/balance-history-regtest-stable-lag-reorg-depth-matrix.md)。

### Real BTC Data

这些测试读取本机真实 Bitcoin Core datadir，并连接真实 bitcoind RPC。默认 `cargo test` 不会编译它们，必须显式 opt-in：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh correctness
```

横向/纵向切片示例：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh loader-index --size tiny

USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh block-cache --size small
```

性能/profile 入口：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-cache --size tiny
```

预设数据段 profile：

```bash
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-early --size tiny

USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-mid --size small

USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile-recent --size small
```

runner 默认会把每个 filter 的耗时和 profile 指标追加到 `target/balance-history-real-btc/metrics.jsonl`。如需写到固定文件：

```bash
USDB_BH_REAL_BTC_METRICS_FILE=/tmp/usdb-bh-real-btc.jsonl \
USDB_BH_REAL_BTC=1 \
BTC_DATA_DIR=/home/bucky/.bitcoin \
BTC_RPC_URL=http://127.0.0.1:8332 \
BTC_COOKIE_FILE=/home/bucky/.bitcoin/.cookie \
bash src/btc/balance-history/scripts/run_real_btc_tests.sh profile --size tiny
```

查看指标汇总：

```bash
python3 src/btc/balance-history/scripts/summarize_real_btc_metrics.py
python3 src/btc/balance-history/scripts/summarize_real_btc_metrics.py /tmp/usdb-bh-real-btc.jsonl --last 20
```

详细说明见 [balance-history-real-btc-tests.md](/home/bucky/work/usdb/doc/balance-history/balance-history-real-btc-tests.md)。

### Snapshot Full

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

容量测试单独运行，默认 1K UTXO；100K 和 advisory cold-cache 必须显式选择：

```bash
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh

SNAPSHOT_CAPACITY_UTXOS=100000 SNAPSHOT_CAPACITY_COLD_CACHE=1 \
bash src/btc/balance-history/scripts/regtest_exact_height_snapshot_capacity.sh
```

指标解释见
[balance-history-exact-height-snapshot-capacity.md](/home/bucky/work/usdb/doc/balance-history/balance-history-exact-height-snapshot-capacity.md)。

在正式硬件上制作并签名分发主网 checkpoint 时，使用
`mainnet_exact_height_snapshot.sh` 封装的
`init/preflight/create/resume-verify/status/verify/finalize` 流程，并按
[balance-history-mainnet-exact-height-snapshot-operations.md](/home/bucky/work/usdb/doc/balance-history/balance-history-mainnet-exact-height-snapshot-operations.md)
完成目录规划、artifact 复核和接收方安装。脚本默认使用独立的
`~/.usdb/balance-history-snapshot-mainnet`，不会复用在线 `~/.usdb/balance-history`。
脚本通过 snapshot tool 的 cgroup-aware `memory-plan` 固化显式 cache limit；默认两类 cache
合计使用有效内存的 `66%`，并在整机/cgroup 使用率达到 `80%` 时开始缩减。OOM 或人工停止后
可刷新这三个运行时内存字段并从 durable height 恢复，不需要删除 builder workspace。
当 job 已进入 `verifying` 时使用 `resume-verify`，复用现有 SQLite artifact，并在不打开
RocksDB/indexer 的独立进程中继续校验和发布。

## 排障

### Port already allocated

每个脚本都有独立默认端口，但本地 dev stack 仍可能占用同一批端口。单次运行可覆盖三个端口：

```bash
BTC_RPC_PORT=39132 BTC_P2P_PORT=39133 BH_RPC_PORT=39110 \
bash src/btc/balance-history/scripts/regtest_smoke.sh
```

### bitcoind not found

显式设置 `BITCOIN_BIN_DIR`：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
bash src/btc/balance-history/scripts/regtest_smoke.sh
```

### Service did not become ready

先看失败输出。共享 cleanup handler 会打印日志尾部。如果覆盖了 `WORK_DIR` 并保留现场，可继续检查：

```bash
$BALANCE_HISTORY_LOG_FILE
$BALANCE_HISTORY_ROOT/logs/balance-history_rCURRENT.log
$BITCOIN_DIR/regtest/debug.log
```

## 新增场景约定

1. 选择一组不冲突的默认端口。
2. 如默认值不够，先设置 `WORK_DIR`、`BITCOIN_DIR`、`BALANCE_HISTORY_ROOT`、`BTC_RPC_PORT`、`BTC_P2P_PORT`、`BH_RPC_PORT`、`WALLET_NAME` 和 `REGTEST_LOG_PREFIX`。
3. `source regtest_lib.sh`。
4. 使用共享生命周期 helper 启停 bitcoind、初始化钱包、生成配置、启动服务、等待 readiness、清理现场。
5. 脚本内只保留场景特有链操作和 RPC 断言。
6. 同步新增或更新 `doc/balance-history/` 下的对应文档。
7. 如果新增重复断言 helper，优先移动到 `regtest_lib.sh`，不要继续复制到多个脚本。

## 已知缺口

- `run_regtest_suite.sh` 当前支持 `smoke`、`stable-lag-reorg`，还未扩展 `core`、`reorg-full`、`snapshot-full`。
- 聚合 RPC `get_address_balance_summary`、`get_address_balance_timeseries`、`get_address_flow_buckets` 已有 Rust unit 覆盖，但还没有 regtest 脚本覆盖。
- `resolve_script_hashes` 已有 Rust unit 覆盖，但还没有基于完整 indexed data 的 regtest 覆盖。
- 多个脚本仍有本地 JSON assertion helper，后续应收敛到 `regtest_lib.sh`。
