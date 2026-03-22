# USDB-Indexer Regtest World-Sim Validator Sampled Validation

## 1. 目标

这条场景把 world-sim 的随机业务流和 validator 历史 context 校验接在一起。

目标不是在 world-sim 里完整模拟 multi-pass candidate-set，而是先做一层低频 sampled validation：

1. 在高度 `H` 采样一张当前 active pass
2. 固定该高度的 `external_state`
3. 等 head 继续前进若干块
4. 再按历史 context 回查该 pass 的 `state / owner / energy`

这样可以验证：

- 随机业务流里的历史 payload 仍能被稳定重放
- 当前 head 前进不会污染旧历史上下文

## 2. 脚本入口

- [regtest_world_sim_validator_context.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_context.sh)

## 3. 核心机制

world-sim simulator 新增了一层低频 validator sample：

- `validator_sample_capture`
  - 每隔 `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS` 块触发
  - 从当前 active pass 集合里抽样 `SIM_VALIDATOR_SAMPLE_SIZE` 张
  - 记录：
    - `block_height`
    - `inscription_id`
    - `owner`
    - `state`
    - `energy`
    - `snapshot_id`
    - `stable_block_hash`
    - `local_state_commit`
    - `system_state_id`

- `validator_sample_validation`
  - 当 head 至少前进 `SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE` 块后触发
  - 复用采样时固定的 `ConsensusQueryContext`
  - 重新调用：
    - `get_state_ref_at_height`
    - `get_pass_snapshot`
    - `get_pass_energy`
  - 要求返回值与采样时完全一致

## 4. 通过标准

日志中应看到：

- `validator_sample_captured > 0`
- `validator_sample_checked > 0`
- `validator_sample_failed = 0`

报告文件里应包含：

- `event = "validator_sample_capture"`
- `event = "validator_sample_validation"`

最终 `session_end.final_metrics` 中：

- `validator_sample_fail = 0`

## 5. 当前边界

这条场景当前只覆盖：

- 单 `pass` sampled validation
- head 前进后的历史上下文稳定性

还不覆盖：

- same-height reorg 下 sampled payload 的分流
- multi-pass candidate-set 重算
- 真实 retention bump

这些会在后续综合测试计划的下一阶段继续扩展。
