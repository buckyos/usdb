# USDB-Indexer Regtest World-Sim Validator Candidate-Set Sampled Validation

## 1. 目标

这条场景把 `world-sim` 的随机业务流继续向 validator 选择逻辑推进一层。

相比单 `pass` sampled validation，这里每次采样固定的是：

1. 同一高度 `H` 的 `external_state`
2. 一组 sampled `candidate_passes`
3. 按 `effective_energy DESC + inscription_id ASC` 规则重算出来的 `winner`

随后等 head 前进若干块，再按同一历史 context 回查所有 candidates，并重算 winner。

## 2. 脚本入口

- [regtest_world_sim_validator_candidate_set.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set.sh)
- [regtest_world_sim_validator_candidate_set_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set_reorg.sh)

## 3. 核心机制

world-sim simulator 在 `candidate_set` 模式下会：

1. 每隔 `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS` 块触发一次采样
2. 通过 UIP-0006 `get_candidate_set_view` 读取 canonical active standard candidate，再抽样 `SIM_VALIDATOR_SAMPLE_SIZE` 张
3. 固定该高度的：
   - `snapshot_id`
   - `stable_block_hash`
   - `local_state_commit`
   - `system_state_id`
   - API / semantics / protocol / formula version
4. 为每张 sampled candidate 记录：
   - `inscription_id`
   - `owner`
   - `state`
   - `pass_kind`
   - `raw_energy`
   - `collab_contribution`
   - `effective_energy`
   - `level`
   - `difficulty_factor_bps`
5. 按 `effective_energy DESC + inscription_id ASC` 规则计算 `winner_inscription_id`

延迟验证时会：

1. 先按包含完整 version identity 的历史 `context` 调 `get_state_ref_at_height`
2. 重新分页读取 `get_candidate_set_view`，确认 sampled pass 仍属于同一 canonical candidate set
3. 对 sampled pass 调 `get_pass_economic_profile`，交叉验证 candidate/profile 的 owner、kind、三字段能量、level 和 factor
4. 要求重放结果与采样时一致，并再次本地重算 winner
5. 如果启用 tamper 检测，再构造一个 wrong-winner 版本并要求本地重算识别篡改

candidate 来源不再使用 `get_active_passes_at_height`，因此 active collab pass 不会进入 sampled validator candidate set。

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
