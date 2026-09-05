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

reorg replacement 在回滚前保存断链区块中的交易，按原高度和依赖顺序重放，并核对实际入块结果及 mempool 清零；它不依赖 Bitcoin Core 是否把断链交易放回 mempool。canonical rebuild 允许 Dormant/Consumed/Burned/Invalid pass 落在未建模 external owner 下继续审计，但任何 external Active pass 都 fail closed。

`spend_balance` 不再依赖钱包自动选币。simulator 从 `ord wallet outputs` 选择属于目标 owner 且不包含 inscription/rune 的 UTXO，通过显式 input 和回到同一 owner 的 change 构造交易；确认块后 owner balance 必须至少下降发送金额，不能再用 warning 放宽。

### Weekly 时间预算与连续状态

weekly matrix 仍为 seeds `41/42/43`，每个独立 job 连续运行 `2500` tick。先采用 Ord/indexer `200ms` 轮询压缩等待，保留 stable lag、动作配置、自检、历史验证和重组频率；Go workflow 的 world-soak 执行步骤最多运行 300 分钟，为 360 分钟 job 上限内的准备和日志上传留出空间。

每条 `tick` 报告新增 `tick_elapsed_ms` 和 `phase_elapsed_ms`，分别记录动作、挖矿、同步、结果校验、视图刷新、自检、全局校验、重组、历史验证和摘要查询。计时不包括该轮最后的报告/恢复快照写入及显式 sleep；完整运行耗时以 matrix 的 `duration_seconds` 为准。成功的 seed summary 还汇总均值、P95、最后 100 轮均值和各阶段累计耗时；恢复运行的计时只汇总最后一个 simulator session。

本地同 seed、24 个初始 active agent、40 tick、两次 depth-3 重组的对照中，旧轮询配置累计 tick 耗时为 `226.600s`，200ms 配置为 `54.188s`，约加速 `4.18x`；两组均零失败。独立钱包使两次运行的交易 ID 和具体链状态不同，该短测仅用于评估等待时间收益，不代表完整 2500 tick 在 GitHub runner 上的预算验收。

随后按 weekly 默认 agent 增长、校验和重组配置完成 seed 41 的 `600` tick 模拟：服务启动到正常停止共 `1085s`，tick 均值 `1762.63ms`、P95 `2803ms`、最后 100 轮均值 `2129.09ms`；609 次结果验证、1236 次 agent 自检、20 次全局校验、5 次历史验证及 5 次 tamper 检查均零失败。第 500 轮重组收敛且继续运行至 600，但该次断链区间无业务交易；含交易的重放由前述 40 轮重组和专项钱包测试覆盖。

这次 600 轮的 simulator、服务清理及 seed summary 成功；外层 matrix 因运行期间修改脚本文本而在末尾出现 Bash 解析错误，已从保留的 seed report 单独验证并生成汇总。固定脚本后的完整 weekly 入口短测（含钱包预检及 matrix 汇总）成功。上述记录不等于完整 GitHub Weekly gate 通过；2500×3 轮仍需在提交并同步 CI revision lock 后验证。

若后续仍需分批，需区分两种覆盖：从新目录启动多组较短测试互不继承状态，覆盖多个随机样本，但不能替代持续积累的长历史；同一条链跨批次延续则必须一致保存 Bitcoin 区块/钱包、Ord 索引、balance-history、indexer 数据库及 simulator 恢复状态。仅上传 recovery JSON 无法跨 job 恢复这组状态。目前不引入跨 job checkpoint，也不减少轮次。

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
