# USDB-Indexer 下一阶段综合测试计划

## 1. 目标

当前 `usdb-indexer` 已经把这些测试面打通：

- reorg / restart / pending recovery
- 历史 `state ref`
- ETHW 风格历史 context 校验
- validator block-body
- multi-pass / tamper / real-energy advantage
- world-sim / deterministic reorg / determinism / soak

下一阶段的重点不再是继续横向加很多相似脚本，而是补“跨功能组合”的高价值测试层，验证这些能力叠加后仍然稳定。

## 2. 当前共识

### 2.1 retention 现状

当前 `STATE_NOT_RETAINED` 仍是简化语义：

- 唯一下界 = `genesis_block_height`
- 系统内部还没有真实 prune / retention floor bump 机制
- 因此“历史未保留”当前本质上等价于“低于协议起点”

所以短期内不应把“真实 prune 回归”排到最高优先级；等真实 retention feature 出现后，再引入新的 floor 元数据和专项回归。

### 2.2 下一阶段最有价值的方向

1. `world-sim × validator payload sampled validation`
2. 更贴近最终 ETHW 选择逻辑的 `3~5 pass candidate-set`
3. `version mismatch / upgrade` 组合场景
4. `restart / crash consistency` × 历史 context

## 3. 分阶段计划

### Phase A: World-Sim × Validator Sampled Validation

目标：

- 在长时间随机业务流中，定期采样一份 validator 风格历史 payload
- 等 head 继续前进后，再按历史 context 回查同一张 pass
- 证明 world-sim 不只是“当前态随机校验”，也能覆盖历史 validator 语义

第一版范围：

- 单 `pass` sampled validation
- 不强依赖 reorg
- 低频采样，避免拖慢 world-sim 主循环

当前状态：

- 第一版已完成
- 第二步已补上 `world-sim + deterministic reorg`
- 现在 sampled validator 路径已经能同时覆盖：
  - head 前进后历史样本仍可验证
  - 落在 replacement 区间内的旧样本稳定返回 `SNAPSHOT_ID_MISMATCH`
  - 默认 reorg wrapper 已收敛到稳定命中 `expected_mismatch` 的参数组合

完成标准：

- 新增 world-sim validator sampled wrapper
- 报告中出现 `validator_sample_capture / validator_sample_validation`
- `final_metrics.validator_sample_fail = 0`

### Phase B: 3~5 Pass Candidate-Set

目标：

- 不只验证单 winner 和双 pass 竞争
- 让 payload 显式携带 `candidate_set`
- validator 本地重算 winner，并验证排序规则

关注点：

- 同一历史 context 下多张 pass 的相对关系
- winner 重算与 payload 记录一致
- 当前 head 前进后历史关系仍可重放

### Phase C: Version Mismatch / Upgrade

目标：

- 补 `snapshot_id / system_state_id / protocol_version / semantics_version` 的升级边界
- 验证历史 payload 在版本变化后稳定落到 `VERSION_MISMATCH`

### Phase D: Restart / Crash Consistency

目标：

- payload 生成后服务重启，再做历史校验
- 历史辅助索引写入过程被打断后的恢复行为
- `rpc_alive=true` 但 `consensus_ready=false` 窗口的 validator 行为

## 4. 当前执行顺序

当前按下面顺序推进：

1. 先落 `Phase A` 第一版
2. 稳定后再扩到 `Phase B`
3. 之后才补 `Phase C / D`

## 5. 备注

如果未来真实 prune / retention floor 演进上线，需要新开一个独立阶段，把：

- per-domain retention floor
- durable floor metadata
- retention bump 后的历史 payload 行为

纳入新的综合回归计划，而不是继续沿用当前 `genesis_block_height` 的简化模型。
