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

目前这份计划里的 `Phase A-D` 已经基本完成，因此本文档同时承担两层角色：

- 作为这一轮综合测试升级的备案与收口
- 作为下一轮增强测试的优先级入口

## 2. 当前共识

### 2.1 retention 现状

当前 `STATE_NOT_RETAINED` 仍是简化语义：

- 唯一下界 = `genesis_block_height`
- 系统内部还没有真实 prune / retention floor bump 机制
- 因此“历史未保留”当前本质上等价于“低于协议起点”

所以短期内不应把“真实 prune 回归”排到最高优先级；等真实 retention feature 出现后，再引入新的 floor 元数据和专项回归。

### 2.2 本轮综合测试结果

当前这轮综合测试已经完成：

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
- `candidate-set` sampled validation 第一批已完成：
  - 同一采样点固定 `winner + candidate_passes`
  - validator 在历史 context 下重查多张 pass，并按 `max_energy + inscription_id` 重算 winner
  - 已补普通 wrapper 和 `deterministic reorg` wrapper
  - 已验证 world-sim 下 `candidate_set` 样本既可正常回放，也能在 replacement 区间内稳定落到 `expected_mismatch`
  - 当前又补上：
    - wrong-winner / tamper 检测
    - 更长时段 `candidate_set sampled soak` 入口

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

执行任务：

1. `3-pass candidate-set happy-path`
   - payload 显式记录 `winner + candidate_passes`
   - validator 在同一历史 context 下重查 3 张 pass，并重算 winner
   - 后续块让当前 winner 真实发生状态变化，旧 payload 仍按历史视图成立
2. `5-pass candidate-set tamper`
   - 扩到 5 张候选 pass
   - 篡改 payload 中记录的 winner
   - validator 通过本地重算识别 payload 被篡改
3. `5-pass candidate-set reorg`
   - same-height replacement 覆盖 candidate-set payload 所在高度
   - 历史 state ref、winner、candidate_passes 在同一历史 context 下稳定返回 `SNAPSHOT_ID_MISMATCH`

当前状态：

- 第一批 3 条 candidate-set 场景已完成并接入 `run_reorg_regression.sh`
- 当前已覆盖：
  - `3-pass candidate-set happy-path`
  - `5-pass candidate-set tamper`
  - `5-pass candidate-set reorg`
- 现阶段这些脚本已经证明：
  - validator 可在同一历史 context 下重查 `winner + candidate_passes`
  - winner 篡改可被本地重算识别
  - same-height replacement 可稳定使旧 candidate-set payload 落到 `SNAPSHOT_ID_MISMATCH`

### Phase C: Version Mismatch / Upgrade

目标：

- 补 `snapshot_id / system_state_id / protocol_version / semantics_version` 的升级边界
- 验证历史 payload 在版本变化后稳定落到 `VERSION_MISMATCH`

执行任务：

1. `single-pass protocol version mismatch`
   - 篡改 validator payload 的 `usdb_index_protocol_version`
   - `get_state_ref_at_height / get_pass_snapshot / get_pass_energy` 必须统一返回 `VERSION_MISMATCH`
2. `single-pass semantics version mismatch`
   - 篡改 validator payload 的 `balance_history_semantics_version`
   - 历史 context 路径必须稳定返回 `VERSION_MISMATCH`
3. `candidate-set protocol version mismatch`
   - 在多 pass `winner + candidate_passes` payload 上重复版本篡改
   - 不只覆盖单 pass，还覆盖 candidate-set 的批量历史校验路径
4. `candidate-set semantics version mismatch`
   - 在多 pass `winner + candidate_passes` payload 上重复语义版本篡改
   - 验证批量历史校验路径对 `balance_history_semantics_version` 同样 fail-closed
5. `balance-history API version mismatch`
   - 篡改 validator payload 的 `balance_history_api_version`
   - `state ref / pass snapshot / pass energy` 必须统一返回 `VERSION_MISMATCH`
6. `version matrix after head advance`
   - 在同一历史 payload 上同时构造 `api / semantics / protocol` 三类版本篡改
   - BTC head 前进后，原 payload 仍通过，三类 tampered payload 仍稳定返回 `VERSION_MISMATCH`
7. `payload-version upgrade coexistence`
   - 同一条链上先生成 `v1.0` 单 pass payload，再生成 `v1.1` candidate-set payload
   - validator 必须在同一升级窗口内接受两代 schema 的历史回放
8. `payload-version upgrade restart`
   - mixed payload 生成后重启 `balance-history / usdb-indexer`
   - 历史 `state ref` 与两代 payload replay 都必须保持成立
9. `payload-version upgrade reorg`
   - same-height replacement 只覆盖新 `v1.1` payload 所在高度
   - 旧 `v1.0` payload 仍通过，新 `v1.1` payload 稳定落到 `SNAPSHOT_ID_MISMATCH`

### Phase D: Restart / Crash Consistency

目标：

- payload 生成后服务重启，再做历史校验
- 历史辅助索引写入过程被打断后的恢复行为
- `rpc_alive=true` 但 `consensus_ready=false` 窗口的 validator 行为

当前第一批已覆盖：

1. `validator block-body restart consistency`
2. `validator block-body not-ready window`
3. `candidate-set crash recovery`

## 4. 当前收口结论

按当前实现与回归脚本状态，这一轮 `Phase A-D` 可以认为已经完成，当前已经具备：

1. 单 `pass` 与 `candidate-set` 的历史 `state ref` / validator payload 回放
2. head 前进、same-height reorg、restart、crash、not-ready window 下的历史校验
3. `protocol / semantics / api` 版本不匹配与 mixed payload upgrade path
4. world-sim 下的 sampled validator replay、`candidate-set` sampled validation、tamper 检测与 soak

换句话说，当前缺的已经不再是“这一轮计划里的核心能力”，而是下一轮更大规模、更接近真实 ETHW 使用方式的增强层。

## 5. Next Wave

如果继续补充，下一轮更值得投入的方向是：

1. `world-sim × candidate-set` 更深组合
   - 更大规模 `candidate_set sampled soak`
   - 更复杂 winner 选择逻辑
   - 更贴近真实 ETHW validator 的 sampled payload 结构
2. 更贴近最终 ETHW block body 的选择证明
   - 不只是明文 `winner + candidate_passes`
   - 而是更接近 `candidate_set_commit / selection proof` 一类结构
3. 更大规模和更长时段的性能 / 稳定性矩阵
   - `candidate-set` 数量扩大
   - validator replay 开销评估
   - 长时 soak 下的稳定性观察
4. 未来真实 prune / retention feature 上线后的新专项
   - 真实 retention floor
   - floor bump 后的历史 payload 行为
   - 与 `STATE_NOT_RETAINED / HISTORY_NOT_AVAILABLE` 的边界重新收口

## 6. 备注

如果未来真实 prune / retention floor 演进上线，需要新开一个独立阶段，把：

- per-domain retention floor
- durable floor metadata
- retention bump 后的历史 payload 行为

纳入新的综合回归计划，而不是继续沿用当前 `genesis_block_height` 的简化模型。
