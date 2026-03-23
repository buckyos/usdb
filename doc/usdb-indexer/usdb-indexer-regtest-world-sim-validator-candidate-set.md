# USDB-Indexer Regtest World-Sim Validator Candidate-Set Sampled Validation

## 1. 目标

这条场景把 `world-sim` 的随机业务流继续向 validator 选择逻辑推进一层。

相比单 `pass` sampled validation，这里每次采样固定的是：

1. 同一高度 `H` 的 `external_state`
2. 一组 sampled `candidate_passes`
3. 按 `max_energy + inscription_id` 规则重算出来的 `winner`

随后等 head 前进若干块，再按同一历史 context 回查所有 candidates，并重算 winner。

## 2. 脚本入口

- [regtest_world_sim_validator_candidate_set.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set.sh)
- [regtest_world_sim_validator_candidate_set_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set_reorg.sh)

## 3. 核心机制

world-sim simulator 在 `candidate_set` 模式下会：

1. 每隔 `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS` 块触发一次采样
2. 从当前 active pass 集合里抽样 `SIM_VALIDATOR_SAMPLE_SIZE` 张
3. 固定该高度的：
   - `snapshot_id`
   - `stable_block_hash`
   - `local_state_commit`
   - `system_state_id`
4. 为每张 sampled candidate 记录：
   - `inscription_id`
   - `owner`
   - `state`
   - `energy`
5. 按 `max_energy + inscription_id` 规则计算 `winner_inscription_id`

延迟验证时会：

1. 先按历史 `context` 调 `get_state_ref_at_height`
2. 再逐张调用：
   - `get_pass_snapshot`
   - `get_pass_energy`
3. 要求每张 candidate 的 `owner / state / energy` 都和采样时一致
4. 再次本地重算 winner，要求与采样时一致
5. 如果启用 tamper 检测，再构造一个 wrong-winner 版本的 payload，并要求 validator 本地重算能识别篡改

如果样本落在 deterministic reorg replacement 区间内，则期望返回：

- `SNAPSHOT_ID_MISMATCH`

## 4. 关键参数

- `SIM_VALIDATOR_SAMPLE_MODE=candidate_set`
- `SIM_VALIDATOR_SAMPLE_TAMPER_ENABLED=1`
- `SIM_VALIDATOR_SAMPLE_SIZE=3`
- `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS`
- `SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE`

reorg wrapper 还会额外打开：

- `SIM_REORG_INTERVAL_BLOCKS`
- `SIM_REORG_DEPTH`
- `SIM_REORG_MAX_EVENTS`

## 5. 通过标准

日志和报告中应体现：

- `validator_sample_mode = candidate_set`
- `validator_sample_capture`
- `validator_sample_validation`
- `validator_sample_tamper_validation`
- `winner_inscription_id`

最终 `session_end.final_metrics` 中：

- `validator_sample_fail = 0`
- `validator_sample_tamper_fail = 0`

reorg wrapper 下允许出现：

- `result = "expected_mismatch"`

但不允许出现真正的 `validator_sample_fail`。
