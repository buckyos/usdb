# MinerPass V2 nightly/weekly 矩阵迁移

本批基于 USDB `e8c73d9`、Go `aea0a7af9`，仅修改测试工具、夹具和 CI 入口。测试使用 Core 28.1、Ord 0.29.0、隔离 regtest 数据目录和 `miner-pass-v2-fixture` catalog。未修改生产执行规则、embedded registry、公开网络 genesis 或在线节点。

## 覆盖与入口

| 入口 | 本批覆盖 |
| --- | --- |
| `tests/run_miner_pass_v2_security_matrix.sh` | 非参与持币地址强制赠予、既有 pass 强制替换/消费、归零旧 owner 不可再次免来源开户、Dormant 跨地址继承；两种协作引用在 Leader 跨地址轮换后不跟随，显式重绑恢复贡献；重组、崩溃重开、空数据库重放 |
| `run_regression.sh` | 存在性检查后的 V2 Rust 精确测试、snapshot anchor/checkpoint WAL、4 个 smoke、5 类 Ord 操作、UIP prev 异常矩阵及 Ord/原生索引一致性；通过 `RUN_MINER_PASS_V2_MATRIX=1` 启用新矩阵 |
| `run_reorg_regression.sh` | 重组、重启与双库 pending 恢复；历史上下文；候选集、协作明细、篡改、版本契约、payload 升级、重启及崩溃恢复 |
| `regtest_world_sim.sh` | V2 铸造与协作/继承、按真实 owner cardinal UTXO 选币、来源币不足时不调度 mint、重组后清理来源缓存、同规则空数据库完整重放 |
| Go `run_long_ci.sh` | nightly `indexer-protocol` 接入新安全矩阵；`indexer-reorg`/`indexer-validator` 和 weekly `world-soak` 通过上述共用入口使用 V2 |

nightly 将新增安全矩阵放入 `$WORK_ROOT/miner-pass-v2-security`，JSON 快照、日志和 TSV 案例清单纳入 `diagnostics/` artifact；归档复制路径已实际检查。

安全矩阵保留 `cases.tsv`、mint JSON、实际选币证据、Ord commit/reveal 输出、audit，以及 `after-reorg.json`、`after-crash-reopen.json`、`full-replay.json`。后三个文件逐字比较，包含系统身份及每个案例的 snapshot/audit；完整重放仅重建 indexer，复用规范链及 balance-history。world replay 则重新建立 balance-history 和 indexer 数据库，并在每个重组检查点及最终高度比较规范状态摘要。

旧测试假设同步调整：能量初值在实际 mint 高度核验，避免将确认期间增长误报为继承错误；零能量惩罚用例保留原断言，额外来源币在该边界之后提供。index origin 是数据集身份，修改它应拒绝启动；恢复原配置后历史仍可查，origin 以下返回 `STATE_NOT_RETAINED`。这不是运行时裁剪功能的验收。

## 复现

在 USDB 根目录执行，`ORD_BIN`、`BITCOIN_BIN_DIR` 指向本机测试二进制。默认新建临时工作目录；如指定端口，使用空闲且低于系统 ephemeral 范围的端口，不复用在线服务。

```bash
export ORD_BIN=/path/to/ord
export BITCOIN_BIN_DIR=/path/to/bitcoin/bin
export ORD_POLLING_INTERVAL=200ms

RUN_REGTEST_SMOKE=1 RUN_LIVE_ORD_REALWORLD_SUITE=1 \
  RUN_UIP0001_0004_LIVE_MATRIX=1 RUN_MINER_PASS_V2_MATRIX=1 \
  bash src/btc/usdb-indexer/scripts/run_regression.sh

bash src/btc/usdb-indexer/scripts/run_reorg_regression.sh
```

weekly 正式入口继续使用原有覆盖门槛，不以减少轮数后的结果冒充完整长跑：

```bash
WORLD_SOAK_BLOCKS=2500 WORLD_SOAK_SEEDS="41 42 43" \
  bash src/btc/usdb-indexer/scripts/run_regtest_world_soak_matrix.sh
```

短程演练可以直接调用 `regtest_world_sim.sh`：6 个 agent、24 轮、economic bootstrap 开启、每 8 轮重组（depth=2，最多两次）、每 4 轮候选集抽样及篡改验证、完整 replay 开启。此配置不满足正式 soak 的覆盖门槛，只用于迁移验收。

## 本地实测记录

2026-10-03（US/Pacific），上述两个基线提交加本批工作区改动。最终 protocol 完整入口与 validator 完整入口均退出 0；protocol 的 `source-state.json` 保存两仓库 HEAD 及 46 个测试脚本哈希，运行结束复核没有变化。其余重组/历史场景与 world 演练按分组运行：

| 已完成部分 | 结果 | 本机临时产物 |
| --- | --- | --- |
| V2 Rust 精确协议 / snapshot anchor / checkpoint WAL | 21 passed、1 ignored；另有空的 doc-test target，不计为功能用例 | `/tmp/usdb-v2-protocol-final-GNCmF9/runner.log` |
| smoke | 4 个场景通过 | 同上 |
| 真实 Ord 操作 | transfer/remint、invalid mint、passive transfer、same-owner replacement、duplicate prev 均通过 | `/tmp/usdb-v2-protocol-final-GNCmF9/runner.log` |
| prev 异常矩阵 | 缺失/无效状态/错误 owner/重复/已消费/烧毁/多 prev；Ord 与原生索引的状态一致 | `/tmp/usdb-v2-protocol-final-GNCmF9/runner.log` |
| 新安全/协作矩阵 | 12 条铭文记录；重组、崩溃重开、完整重放快照相同 | `/tmp/usdb-v2-protocol-final-GNCmF9/miner-pass-v2-security` |
| reorg/pending | 11 个场景通过 | `/tmp/usdb-v2-reorg-regression-W10Yk2/runner.log` |
| 历史验证 | 4 个场景通过 | `/tmp/usdb-v2-history-validator-JLpYmW/runner.log` |
| 验证接口完整矩阵 | 27 个场景通过，整个 runner 退出 0 | `/tmp/usdb-v2-validator-w0qidz/runner.log` |
| world 迁移演练 | seeds 41/42/43 各 24 轮、2 次重组、6 次候选集与历史/篡改验证、3 个重放检查点；所有 fail 计数为 0，replay comparison 为 ok | `/tmp/usdb-v2-world-seeds-9IW0Bw/seed-41`、`/tmp/usdb-v2-world-matrix-Ocz9TI`、`/tmp/usdb-v2-world-seed43-JsUoc2` |
| 快速回归 | USDB 106 项（含真实 Core/Ord 钱包在 raw depth 10/11/13 重组后恢复的 3 项）、Go V2 registry 3 项通过 | 终端测试结果 |

表中仅列成功结果，不将早期失败的 runner 总退出码称为通过。发现并修正的测试问题包括旧 origin 变更模拟、确认高度/能量假设和 fresh replay 未复制 external catalog；另有隔离测试端口占用，改用独立且低于 ephemeral 范围的端口后重跑。一次在脚本执行期间修改归档路径引发了 shell 读取结尾错误；冻结脚本后完整重跑成功。失败产物保留用于诊断，仅上述明确列出的成功运行属于通过证据。没有因此修改生产共识代码。

## 验收边界

本批不等于整个 nightly/weekly 或 testnet-v1 发布资格。完整 2500 轮多种子 soak、Go activation/upstream/release 等其余 shard、完整 AssumeUTXO 管线和签名 checkpoint 服务验收仍需按计划完成；此前证据层的 `txindex=0` 和签名种类测试不自动升级为这些服务场景已验收。control-plane 钱包引导继续后移。
