# USDB-Indexer Regtest World-Sim Validator Sampled Validation

## 1. 目标

这条场景把 world-sim 的随机业务流和 validator 历史 context 校验接在一起。

目标是把 world-sim 的随机业务流和 validator 历史 context 校验接在一起，并分两步扩展 sampled validation：

1. `single` 模式
   - 在高度 `H` 从 UIP-0006 canonical candidate view 采样一张 active standard pass
   - 固定该高度的 `external_state`
   - 等 head 继续前进若干块
   - 再按历史 context 回查该 pass 的 `state / owner / raw_energy / collab_contribution / effective_energy`
2. `candidate_set` 模式
   - 在高度 `H` 从 UIP-0006 canonical candidate view 采样一组 active standard passes
   - 固定该高度的 `external_state`
   - 在采样时按 `effective_energy DESC + inscription_id ASC` 重算 winner
   - 等 head 继续前进若干块
   - 再按历史 context 回查所有 candidates，并重新计算 winner

这样可以验证：

- 随机业务流里的历史 payload 仍能被稳定重放
- 当前 head 前进不会污染旧历史上下文
- 多张 pass 的相对关系也能在同一历史视图下稳定重放

## 2. 脚本入口

- [regtest_world_sim_validator_context.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_context.sh)
- [regtest_world_sim_validator_candidate_set.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_world_sim_validator_candidate_set.sh)

## 3. 核心机制

world-sim simulator 新增了一层低频 validator sample：

- `validator_sample_capture`
  - 每隔 `SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS` 块触发
  - 分页读取 `get_candidate_set_view`，再从 canonical active standard candidate 中抽样 `SIM_VALIDATOR_SAMPLE_SIZE` 张
  - `single` 模式记录：
    - `block_height`
    - `inscription_id`
    - `owner`
    - `state`
    - `pass_kind`
    - `raw_energy`
    - `collab_contribution`
    - `effective_energy`
    - `level`
    - `difficulty_factor_bps`
    - `snapshot_id`
    - `stable_block_hash`
    - `local_state_commit`
    - `system_state_id`
  - `candidate_set` 模式额外记录：
    - `candidate_ids`
    - `winner_inscription_id`
    - 每张 candidate 在 `H` 时的 `owner / state / raw_energy / collab_contribution / effective_energy`

- `validator_sample_validation`
  - 当 head 至少前进 `SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE` 块后触发
  - 复用采样时固定的 `ConsensusQueryContext`
  - `single` 模式重新调用 `get_state_ref_at_height` 和 `get_pass_economic_profile`
  - `candidate_set` 模式重新分页读取 `get_candidate_set_view`，并用 `get_pass_economic_profile` 交叉验证 sampled candidates
  - context 固定 snapshot/commit/system id 以及 API/semantics/protocol/formula version
  - 要求返回值与采样时完全一致；`candidate_set` 模式还要求重算出的 winner 与采样时一致

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

- `single` 模式 sampled validation
- `candidate_set` 模式 sampled validation
- head 前进后的历史上下文稳定性

还不覆盖：

- 更高阶的 selection proof / candidate-set commit
- 真实 retention bump

same-height reorg 的 sampled payload 分流已由：

- [usdb-indexer-regtest-world-sim-validator-sampled-reorg.md](/home/bucky/work/usdb/doc/usdb-indexer/usdb-indexer-regtest-world-sim-validator-sampled-reorg.md)

继续覆盖。
