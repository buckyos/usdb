# USDB-Indexer Regtest World-Sim Validator Sampled Validation With Reorg

## 1. 目标

这条场景在 `world-sim validator sampled validation` 基础上继续覆盖 deterministic reorg。

目标是把历史 validator sample 分成两类：

1. 没有落入 replacement 区间的样本
   - 后续仍应按历史 context 校验通过
2. 落入 same-height replacement 区间的样本
   - 后续不应再被当成“历史仍成立”
   - 应稳定返回 `SNAPSHOT_ID_MISMATCH`

## 2. 脚本入口

- [regtest_world_sim_validator_context_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_context_reorg.sh)

默认参数已经收敛到一组稳定能命中 replacement 样本的组合：

- `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=7`
- `SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE=2`
- `SIM_REORG_INTERVAL_BLOCKS=8`
- `SIM_REORG_DEPTH=2`
- `SIM_REORG_MAX_EVENTS=1`

这组默认值会先在 `tick=7 / height=129` 捕获 sample，再在 `tick=8` 用 `depth=2` 的 deterministic reorg 覆盖 `129..130`，从而稳定打到 `expected_mismatch` 分支。

## 3. 核心机制

world-sim simulator 在执行 deterministic reorg 后，会把尚未验证、且高度落在 replacement 区间内的 validator sample 标记成：

- `expected_consensus_error = SNAPSHOT_ID_MISMATCH`

后续 head 前进到最小回查延迟后：

- 未被 reorg 触及的 sample 继续走正常历史回查
- 被 replacement 覆盖的 sample 改为“预期 mismatch 成功”

## 4. 通过标准

日志和报告中应同时看到：

- `event = "reorg"`
- `event = "validator_sample_capture"`
- `event = "validator_sample_validation"`

其中 `validator_sample_validation.result` 允许两种成功类型：

- `ok`
- `expected_mismatch`

最终：

- `final_metrics.validator_sample_fail = 0`
- `final_metrics.reorg_fail = 0`

默认 smoke 下还应看到：

- `reorg.validator_sample_invalidated_ids` 非空
- 至少一条 `validator_sample_validation.result = "expected_mismatch"`

## 5. 当前边界

这条场景当前只把 deterministic reorg 引进单 `pass` sampled validation。

后续如果要继续推进：

- 可以把 sampled payload 扩到 multi-pass candidate-set
- 也可以把 mismatch 类型从当前单一的 `SNAPSHOT_ID_MISMATCH` 扩成更细分的历史状态分流
