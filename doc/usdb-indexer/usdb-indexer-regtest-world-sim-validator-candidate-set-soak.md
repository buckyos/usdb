# USDB-Indexer Regtest World-Sim Validator Candidate-Set Soak

## 1. 目标

这条入口把 `candidate_set sampled validation` 从缩小版 smoke 推进到更长时间的 world-sim 长跑。

重点不是最短路径复现，而是让下面几类能力在长时间随机业务流中反复交织：

1. sampled `candidate_set` 历史回放
2. winner 重算
3. wrong-winner / tamper 检测
4. agent 自检
5. 全局 cross-check

## 2. 入口脚本

- [run_live_validator_candidate_set.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live_validator_candidate_set.sh)
- [run_live.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/run_live.sh)
- [regtest_world_sim.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim.sh)

## 3. 默认画像

`run_live_validator_candidate_set.sh` 默认预载：

1. `AGENT_COUNT=120`
2. `SIM_BLOCKS=2500`
3. `SIM_VALIDATOR_SAMPLE_MODE=candidate_set`
4. `SIM_VALIDATOR_SAMPLE_TAMPER_ENABLED=1`
5. `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=30`
6. `SIM_VALIDATOR_SAMPLE_SIZE=5`
7. `SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE=3`

## 4. 验收标准

建议检查：

1. `session_end.final_metrics.validator_sample_fail = 0`
2. `session_end.final_metrics.validator_sample_tamper_fail = 0`
3. `session_end.final_metrics.validator_sample_ok > 0`
4. `session_end.final_metrics.validator_sample_tamper_ok > 0`
5. `session_end.final_metrics.global_cross_check_fail = 0`
6. `session_end.final_metrics.agent_self_check_fail = 0`

## 5. 运行示例

缩小版 smoke：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
RESET_WORK_DIR_FORCE=1 \
AGENT_COUNT=6 \
SIM_BLOCKS=18 \
SIM_MAX_ACTIONS_PER_BLOCK=3 \
SIM_SLEEP_MS_BETWEEN_BLOCKS=0 \
SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=6 \
SIM_VALIDATOR_SAMPLE_SIZE=3 \
bash src/btc/usdb-indexer/scripts/run_live_validator_candidate_set.sh
```

默认长跑：

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/run_live_validator_candidate_set.sh
```
