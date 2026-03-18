# USDB-Indexer Regtest World-Sim Live Reorg Soak

本文档描述 `world-sim + deterministic reorg` 的长时间 soak 入口。它的目标不是最短路径复现，而是让随机业务流在更接近真实压力的持续运行中，反复经历 replacement chain 后仍保持一致性。

## 入口脚本

- [run_live_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live_reorg.sh)
- [run_live.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live.sh)
- [regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)
- [regtest_world_sim_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_reorg.sh)
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
