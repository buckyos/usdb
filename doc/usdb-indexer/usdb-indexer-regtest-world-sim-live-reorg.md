# USDB-Indexer Regtest World-Sim Live Reorg Soak

本文档描述 `world-sim + deterministic reorg` 的长时间 soak 入口。它的目标不是最短路径复现，而是让随机业务流在更接近真实压力的持续运行中，反复经历 replacement chain 后仍保持一致性。

## 入口脚本

- [run_live_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live_reorg.sh)
- [run_live.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live.sh)
- [regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)
- [regtest_world_sim_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh)
- [run_regtest_world_soak_matrix.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_regtest_world_soak_matrix.sh)
- [usdb-indexer-regtest-world-sim-reorg.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-reorg.md)

## 覆盖目标

1. 使用 `adaptive` 策略持续生成较长时间的随机业务流。
2. 周期性注入 deterministic reorg，而不是只跑单次 replacement。
3. 在业务继续流动的同时，持续观察：
   - `verify_fail`
   - `agent_self_check_fail`
   - `global_cross_check_fail`
   - `reorg_fail`
4. 验证多次 reorg 后，服务仍能继续推进到更高 tip，而不是只在第一次 replacement 后成功。

## 默认画像

`run_live_reorg.sh` 默认预载的是一组偏“长跑压测”的参数：

1. `AGENT_COUNT=120`
2. `SIM_BLOCKS=2500`
3. `SIM_POLICY_MODE=adaptive`
4. `SIM_REORG_INTERVAL_BLOCKS=180`
5. `SIM_REORG_DEPTH=3`
6. `SIM_REORG_MAX_EVENTS=8`
7. `SIM_GLOBAL_CROSS_CHECK_INTERVAL_BLOCKS=25`
8. `SIM_AGENT_SELF_CHECK_INTERVAL_BLOCKS=5`

这组默认值不是唯一标准，只是给长期 soak 一个更像线上扰动的起点。

## 运行示例

直接使用长跑预设：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/run_live_reorg.sh
```

运行隔离端口、可并行的 2500-tick 多 seed 矩阵：

```bash
WORLD_SOAK_SEEDS="41 73 109" \
WORLD_SOAK_BLOCKS=2500 \
WORLD_SOAK_AGENT_COUNT=24 \
WORLD_SOAK_PARALLELISM=3 \
WORLD_SOAK_OUTPUT_ROOT=/tmp/usdb-world-soak-matrix \
bash src/btc/usdb-indexer/scripts/run_regtest_world_soak_matrix.sh
```

矩阵固定启用 economic bootstrap、agent/global cross-check、candidate replay/tamper，
并每 500 tick 注入一次 depth-3 reorg，最多 4 次。每个 seed 使用独立
bitcoind/ord/balance-history/indexer 端口和 datadir；任一 seed 失败会保留其 workspace，
所有 seed 成功时默认只保留 report、service log 和汇总。

矩阵同时支持从 `seed-<seed>-recovery.json` 恢复未完成的 seed。恢复过程复用原有
Bitcoin chain、ord index 和服务数据库，不再重复预挖、创建钱包或注资；启动脚本会
重新加载 recovery state 中的 wallet。`usdb-indexer` 启动时必须先从 pass storage
重建 transfer tracker，再开放 RPC 和开始扫块，否则 restart 后的已存量铭文转移会
被漏掉。

ord wallet 命令只对已确认无副作用的 transient error 做有限重试。目前包括
Bitcoin Core 的 wallet rescan 窗口，以及 wallet output 尚未进入 ord server 的同步
窗口；其他错误仍立即失败并保留 workspace，避免把协议或业务错误掩盖成重试。

先跑一条缩小版 smoke，确认环境和端口没有问题：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
RESET_WORK_DIR_FORCE=1 \
AGENT_COUNT=6 \
SIM_BLOCKS=24 \
SIM_MAX_ACTIONS_PER_BLOCK=2 \
SIM_SLEEP_MS_BETWEEN_BLOCKS=0 \
SIM_REORG_INTERVAL_BLOCKS=12 \
SIM_REORG_DEPTH=2 \
SIM_REORG_MAX_EVENTS=1 \
bash src/btc/usdb-indexer/scripts/run_live_reorg.sh
```

## 常用环境变量

1. `WORK_DIR`：默认 `/tmp/usdb-world-live-reorg`。
2. `RESET_WORK_DIR`、`RESET_WORK_DIR_FORCE`：是否重置长跑目录。
3. `AGENT_COUNT`：默认 `120`。
4. `SIM_BLOCKS`：默认 `2500`。
5. `SIM_POLICY_MODE`：默认 `adaptive`。
6. `SIM_REORG_INTERVAL_BLOCKS`：默认 `180`。
7. `SIM_REORG_DEPTH`：默认 `3`。
8. `SIM_REORG_MAX_EVENTS`：默认 `8`。
9. `SIM_REPORT_FILE`：默认 `${WORK_DIR}/world-sim-live-reorg.jsonl`。
10. `SIM_GLOBAL_CROSS_CHECK_INTERVAL_BLOCKS`：默认 `25`。
11. `SIM_AGENT_SELF_CHECK_INTERVAL_BLOCKS`：默认 `5`。

矩阵入口还支持：

1. `WORLD_SOAK_SEEDS`：空格分隔 seed，默认 `41 42 43`。
2. `WORLD_SOAK_PARALLELISM`：同时运行的 seed 数，默认 `1`。
3. `WORLD_SOAK_BASE_PORT` / `WORLD_SOAK_PORT_STRIDE`：隔离端口段。
4. `WORLD_SOAK_KEEP_WORKSPACES=1`：成功后也保留完整 datadir。
5. `WORLD_SOAK_OUTPUT_ROOT`：矩阵 JSON、每 seed report 与日志根目录。
6. `WORLD_SOAK_ORDINAL_OFFSET`：恢复部分 seed 时保留其原始端口序号。

## 验收标准

脚本成功时会输出：

```text
World simulation finished successfully.
```

同时建议检查：

1. `session_end.final_metrics.reorg_ok > 0`
2. `session_end.final_metrics.reorg_fail = 0`
3. `session_end.final_metrics.verify_fail = 0`
4. `session_end.final_metrics.agent_self_check_fail = 0`
5. `session_end.final_metrics.global_cross_check_fail = 0`

如果这些指标长期保持为零，再继续提高 agent 数、block 数和 reorg 频率，才有意义。

## 2026-07-27 三 Seed 结果

本轮运行 seeds `41 / 73 / 109`，每个 seed 2500 tick、24 agents、4 次
depth-3 reorg，总计 7500 tick 和 12 次 reorg。最终 v2 汇总位于
`/tmp/usdb-world-soak-matrix-2500-20260727/matrix-summary.json`：

- `agent_self_check_ok=17388`
- `global_cross_check_ok=174`
- `validator_sample_ok=72`
- `validator_sample_tamper_ok=72`
- 所有 `*_fail` 合计为 `0`

seed 41 是完整 clean run，用时 13384 秒。seed 73/109 在约 1760 tick 时暴露
Bitcoin Core wallet rescan 窗口和 restart 后 transfer tracker 未恢复问题；修正后从
原 chain/database 恢复，分别继续运行 4439/4128 秒至 2500 tick。因此 v2 汇总将后
两者标为 `resumed_from_recovery=true`、`duration_scope=recovery_stage`，其时长不能
与 seed 41 的 `full_run` 直接比较。

本轮故障定位推动了三项实现收敛：

1. ord wallet 只对已知无副作用 transient error 做有限重试。
2. recovery 恢复已有 wallet，不重复预挖、注资或创建身份。
3. `usdb-indexer` 在开放 RPC/扫块前调用 `InscriptionIndexer::init()`，从 storage
   重建 transfer tracker。

当前结果证明 clean 长跑和恢复后继续 reorg/replay 均可收敛。发布门禁仍应在合并后的
干净提交上重跑完整三 seed 矩阵；qualification run 不能替代该 clean release evidence。
