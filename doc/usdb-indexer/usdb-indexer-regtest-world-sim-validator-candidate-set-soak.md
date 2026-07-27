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

## 6. 长跑诊断与恢复

`regtest_world_sim.sh` 默认同时保存三类 simulator artifact：

- `world-simulator.log`：完整 stdout/stderr，不依赖调用终端保留。
- `world-sim-report.jsonl`：结构化 session、tick、reorg、validator sample 和 failure 事件。
- `world-sim-recovery-state.json`：每个 action receipt 和 tick 边界的原子 snapshot；成功结束后删除，失败时保留。

`session_start` 记录 seed、全部 action probability、agent growth、page limit、fail-fast、reorg 和 validator sample 参数，保证结果可重放。恢复 snapshot 会校验 agent id、wallet、receive address 和 owner script hash；不同 workspace/wallet identity 不允许复用。该恢复文件面向“同一组仍在运行的 regtest 服务和钱包”上的 simulator 重启，不替代完整 wrapper 的服务重建。

reorg replacement 会先把断链后返回 mempool 的交易打入 replacement 首块，再挖空剩余 replacement block，并要求 mempool 严格清零。canonical rebuild 允许 Dormant/Consumed/Burned/Invalid pass 落在未建模 external owner 下继续审计，但任何 external Active pass 都 fail closed。

`spend_balance` 不再依赖钱包自动选币。simulator 从 `ord wallet outputs` 选择属于目标 owner 且不包含 inscription/rune 的 UTXO，通过显式 input 和回到同一 owner 的 change 构造交易；确认块后 owner balance 必须至少下降发送金额，不能再用 warning 放宽。

## 7. 2026-07-26 执行结果

完成一轮独立真实服务栈的 `120 agents / 300 ticks` 长跑：

- 每块最多 6 个动作，agent 从 24 个开始、每 20 tick 增加 6 个；最终高度 `530`、active agent `114`。
- deterministic UIP-0001 至 UIP-0006 bootstrap 后，累计完成 182 次 standard mint、86 次 collab mint、105 次三类 remint、10 次 invalid mint、113 次 transfer、284 次增资和 112 次支出。
- 892 次动作结果验证、4050 次 agent energy/balance oracle 自检、23 次 profile/candidate/breakdown 全局交叉校验全部通过。
- 完成 19 次 historical candidate-set replay 和 19 次 wrong-winner tamper negative check，全部按预期通过或拒绝。
- tick `100/200` 各执行一次 depth-3 reorg，分别重放 14/17 个断链交易；两次 replacement 后 mempool 都为 0，服务 stable hash 一致，external Active owner 都为 0。
- 最终数据库视图包含 383 张 pass、80 张 Active、11 张 Invalid；全部 `*_fail`、`verify_fail`、`agent_self_check_fail`、`global_cross_check_fail`、`reorg_fail`、`validator_sample_fail` 和 `validator_sample_tamper_fail` 均为 0。
- simulator 用时约 `1569s`，清理前 workspace 约 `284 MiB`。tick 224 的 warm steady-state RSS 抽样约为 bitcoind `202 MiB`、ord `84 MiB`、balance-history `47 MiB`、usdb-indexer `40 MiB`、simulator `36 MiB`；这是单点 RSS，不是 peak/HWM 或生产容量 SLA。
- 正常结束后 recovery state 已删除，隔离 regtest bitcoind/ord/balance-history/usdb-indexer 进程均已停止；本机正式网 bitcoind 未被使用或停止。

上述 300-tick 进程在 strict owner-UTXO 改动加载前已经启动，因此其中 112 次支出只计入随机工作负载，不作为“owner delta 被严格命中”的证据。改动后另执行 `6 agents / 18 ticks` 的 strict-spend 聚焦 live smoke，完成 13 次 explicit-owner-input 支出、1 次 depth-3 reorg、31 次结果验证、108 次 agent 自检、14 次全局交叉校验和 4 次 validator replay，所有失败指标为 0，且日志中没有 relaxed spend warning。
