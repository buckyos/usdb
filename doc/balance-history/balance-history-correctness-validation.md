# Balance-History 正确性验证方案

本文档定义 `balance-history` 的正确性验证分层。目标是让日常回归不依赖 Electrs，
同时保留主网外部数据源对拍作为发布前审计手段。

## 基本原则

1. 测试 oracle 必须独立于 `balance-history` 的 RocksDB、缓存和查询实现。
2. 余额、delta、UTXO、script registry 和 block commit 必须由同一条确定性状态线交叉验证。
3. 默认 CI 不依赖开发者本机数据库、Electrs 或主网 snapshot。
4. 主网抽样用于发现真实数据兼容问题，不能替代可重复的构造场景。
5. BIP30、Core unspendable、rollback 原子性和 snapshot retention floor 等低频规则，
   必须有可精确断言的 fake-chain/Rust 测试，不能期待随机 regtest 自然命中。
6. 同高度旧 snapshot 与当前全量重放库的跨版本差异，应使用独立只读 comparator 分类，不能把旧
   schema 安装进当前服务来完成对拍。

## 四层验证

| 层级 | 数据源 | 入口 | 主要职责 |
| --- | --- | --- | --- |
| L1 独立模型单测 | 手工构造 block JSON | `test_regtest_balance_oracle.py` | 验证 oracle 自身的输入、输出、花费、zero-net movement 和非连续区块拒绝 |
| L2 Rust fake-chain | 内存构造 BTC block/分叉 | `cargo test -p balance-history` | 覆盖 BIP30 generation、Core unspendable、batch linkage、rollback/restart、DB identity 和配置负例 |
| L3 bitcoind regtest | 真实 Bitcoin Core regtest | `run_regtest_suite.sh correctness` | 黑盒验证 RPC、spend graph、同块聚合、stable lag，以及完整历史状态线 |
| L4 主网外部审计 | Bitcoin Core、可选 Electrs、只读 blk 文件 | 手工发布前任务 | 抽样验证真实脚本/UTXO/历史，并发现 parser 或上游数据兼容差异 |

## Regtest 独立状态线

`regtest_balance_oracle.py` 只读取 Bitcoin Core 返回的完整 block JSON，自行维护：

- 每个受跟踪 script/address 的逐高度余额；
- 每个被触及高度的 exact delta，包括净变化为零的 movement；
- live UTXO 与 spent UTXO；
- 可查询的 movement history 和区间 summary；
- 已出现 script 集合。

`regtest_history_balance_oracle.sh` 在 stable frontier 上对真实服务执行以下交叉校验：

1. 检查点随机历史高度与当前稳定高度余额。
2. 每个受跟踪地址、每个场景高度的完整 balance/delta 状态线。
3. 完整 movement range 与 aggregate summary。
4. live/spent UTXO 集。
5. script hash 到 script/address 的 registry 查询。
6. 每个检查点的 block commit hash 与 Bitcoin Core canonical hash。

该场景不把 BTC tip 当成可查询高度；所有断言均遵守 registry 冻结的
`stable_lag_blocks`。

## 执行档位

快速正确性回归：

```bash
bash src/btc/balance-history/scripts/run_regtest_suite.sh correctness
```

Oracle 默认档位为 `8` 个受跟踪地址、`4` 个未跟踪地址、`18` 个事件块、每块
`3` 笔转账，适合开发期反复运行。

中等规模历史状态线：

```bash
ADDRESS_COUNT=16 UNTRACKED_ADDRESS_COUNT=8 \
BLOCK_COUNT=50 TXS_PER_BLOCK=6 CHECK_INTERVAL=5 \
bash src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

Nightly/soak 建议档位：

```bash
ADDRESS_COUNT=32 UNTRACKED_ADDRESS_COUNT=16 \
BLOCK_COUNT=120 TXS_PER_BLOCK=8 CHECK_INTERVAL=10 \
SYNC_TIMEOUT_SEC=300 \
bash src/btc/balance-history/scripts/regtest_history_balance_oracle.sh
```

Nightly 档位产生约 `960` 笔转账和至少 `3,840` 个事件 address-height 点；按当前
`stable_lag=5`，计入初始高度和确认区间后的完整状态线为 `4,032` 点。每个点同时验证
balance 与 exact delta，结束时再验证完整 range、summary、UTXO 和 registry。

## 场景覆盖边界

完整 correctness gate 不是单一脚本。下列规则由对应专项层负责：

| 规则 | 主要覆盖 |
| --- | --- |
| BIP30 duplicate coinbase generation | Rust fake-chain 与 block index 单元测试 |
| `OP_RETURN` / script 大于 10,000 bytes | Rust fake-chain、共享 helper 与 verifier 单元测试 |
| same-block multi-input / zero-net aggregate | Rust fake-chain、oracle 单测、`regtest_multi_input_same_block_queries.sh` |
| spend graph / live-spent UTXO | oracle、`regtest_spend_graph_queries.sh`、`regtest_rpc_semantics.sh` |
| stable lag / restart / lag-window replacement | `regtest_stable_lag_smoke.sh` 与 stable-lag reorg depth suite |
| rollback 原子恢复 / DB identity | Rust restart/failure-injection tests 与 reorg suites |
| snapshot retention floor / install identity | snapshot Rust tests 与 snapshot regtest suites |

## 主网外部审计

本地 Electrs 数据库当前不作为这批测试的前置条件。重建后也建议把它降级为发布前
抽样审计，而不是日常 correctness 的唯一 oracle。

### 最新状态

优先使用 Bitcoin Core 自身的 UTXO 集：

1. 从 stable frontier 之前的确定性随机区块独立抽取 spendable output script，不依赖
   balance-history script registry。
2. 将样本 scriptPubKey 编码为 `raw(...)` descriptor，调用 `scantxoutset`，并采用其返回的
   `height/bestblock` 作为实际扫描锚点。
3. 排除在 `(stable_height, scan_height]` 内发生过收付的脚本，再将 Core UTXO 汇总与
   stable height 对应的 balance-history 余额对拍。
4. 对有界 live outpoint 样本调用 `gettxout(include_mempool=false)`，交叉核对 value 与
   scriptPubKey。
5. 结束前复查 Core tip、stable-height canonical hash 和 balance-history block commit，
   任何重组或断面推进都 fail closed。

该路径不需要 Electrs history 查询，适合验证当前断面的余额和 UTXO。
实现、主网命令和边界说明见
[balance-history-bitcoin-core-utxo-audit.md](balance-history-bitcoin-core-utxo-audit.md)。

### 跨版本全状态对拍

保留的旧主网 snapshot 可以作为一次性的历史实现参考，但不能直接安装到当前 schema。当前
RocksDB 从创世块重放到完全相同的高度并停止服务后，使用独立 comparator 对拍最新余额、完整
UTXO 和 rolling block commit；通过后再追加十亿级 script registry。已冻结的 BIP30 和 Bitcoin
Core unspendable 语义差异单独计数，其余差异一律 fail closed。操作说明见
[balance-history-legacy-snapshot-semantic-comparison.md](balance-history-legacy-snapshot-semantic-comparison.md)。

### 历史状态

Electrs 没有直接的任意高度余额接口。旧流程按地址逐个读取 history、再逐笔读取交易，
会产生大量重复 I/O。恢复 Electrs 后应至少加入：

- 固定 seed 的分层样本清单和可审计输出；
- 全局 txid 去重缓存，而不是每个地址重复下载同一交易；
- 有界并发、批量请求、重试和 checkpoint/resume；
- 将 latest balance 快速审计与 historical replay 慢审计拆成两个任务。

更稳妥的中期方案是实现一个只读 sampled block oracle：给定一组 script hash，按区块顺序
只扫描一次本地 blk/Bitcoin Core block，维护这些样本的 outpoint 与余额。它的成本主要随
扫描区块数增长，不会随每个地址的 Electrs 历史长度重复放大，而且与生产 RocksDB 写入路径
保持独立。

## 发布前验收

1. 默认 Rust 测试和 `correctness` regtest suite 全部通过。
2. 涉及 rollback/snapshot 时追加对应 reorg 和 snapshot suite。
3. Nightly 档位保存 seed、参数、耗时和失败现场。
4. 主网至少完成一次 Bitcoin Core UTXO 分层抽样；Electrs 恢复后再执行历史慢审计。
5. 任一 external oracle mismatch 都必须保存 script、height、block hash、outpoint 和双方结果，
   不能只输出地址与最终余额。
