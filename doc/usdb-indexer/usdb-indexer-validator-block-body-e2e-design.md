# USDB Indexer Validator Block-Body E2E 设计

## 1. 目标

这份设计把现有的 historical-context 校验脚本进一步收敛成更贴近 USDB validator 真实消费方式的测试模型。

本文中的 JSON 是链外 regtest 使用的 **validator test envelope**，不是 UIP-0007 定义的链上 `ProfileSelectorPayload`。测试 envelope 可以携带完整 candidate/profile/breakdown 审计数据；链上 selector payload 只携带最小 `pass_id` 选择信息。除非显式写作 `ProfileSelectorPayload`，本文后续的 `payload`、脚本名和 helper 名都只是该 test envelope 的历史实现名称。

目标不是立刻模拟整条 ETHW 链，而是先固定一份更像真实 block body 校验输入的 test envelope，并验证：

1. 出块方可以在 BTC 高度 `H` 生成一份稳定的 validator test envelope。
2. 验证方只依赖 payload 和 BTC RPC，就能按历史上下文重放校验。
3. BTC head 前进、same-height reorg、历史保留窗口变化、历史辅助数据缺失时，错误分流仍然稳定。

## 2. 当前脚手架可复用部分

当前 `usdb-indexer` regtest 栈已经具备这条链路的多数基础能力：

- `balance-history.get_state_ref_at_height`
- `usdb-indexer.get_state_ref_at_height`
- `usdb-indexer.get_pass_economic_profile(context=...)`
- `usdb-indexer.get_candidate_set_view(context=..., cursor=...)`
- `usdb-indexer.get_collab_breakdown(context=..., cursor=...)`
- `ConsensusQueryContext`
- `STATE_NOT_RETAINED / HISTORY_NOT_AVAILABLE / *_MISMATCH`

共享脚手架 [regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh) 已经能复用：

- 服务生命周期与 readiness 等待
- `regtest_wait_usdb_state_ref_available`
- `regtest_get_usdb_state_ref_response`
- `regtest_build_consensus_context_json`
- `regtest_assert_usdb_consensus_error`
- live ord mint / send / reorg / restart helper

## 3. Validator Test Envelope v1

建议统一一份更贴近 ETHW block body 校验输入的链外测试 envelope 结构：

```json
{
  "payload_version": "1.0.0",
  "external_state": {
    "btc_height": 900123,
    "snapshot_id": "snapshot-...",
    "stable_block_hash": "000000...",
    "local_state_commit": "local-...",
    "system_state_id": "system-...",
    "balance_history_api_version": "1.0.0",
    "balance_history_semantics_version": "balance-snapshot-at-or-before:v1",
    "usdb_index_protocol_version": "1.0.0",
    "usdb_index_formula_version": "pass-energy-formula:v1"
  },
  "miner_selection": {
    "inscription_id": "txidi0",
    "owner": "76a914...",
    "state": "active",
    "raw_energy": "100000000",
    "collab_contribution": "23456789",
    "effective_energy": "123456789",
    "resolved_height": 900123,
    "query_block_height": 900123
  }
}
```

### 3.1 `external_state`

这一段是 validator 真正要 pin 的 BTC 外部状态引用：

- `btc_height`
- `snapshot_id`
- `stable_block_hash`
- `local_state_commit`
- `system_state_id`
- `balance_history_api_version`
- `balance_history_semantics_version`
- `usdb_index_protocol_version`
- `usdb_index_formula_version`

其中：

- `snapshot_id` 锁定 upstream snapshot
- `local_state_commit` 锁定本地 durable core state
- `system_state_id` 锁定给 ETHW 消费的顶层系统状态

### 3.2 `miner_selection`

这一段描述出块方当时看到的 miner pass 选择结果：

- `inscription_id`
- `owner`
- `state`
- `raw_energy`：UIP-0003 raw energy canonical decimal string
- `collab_contribution`：UIP-0004 collab aggregate canonical decimal string
- `effective_energy`：UIP-0004 effective energy canonical decimal string
- `resolved_height`
- `query_block_height`

本文用 `test_selected_pass` 指代 `miner_selection.inscription_id` 对应的 pass。它只表示测试 envelope 选择并要求重放校验的对象，不是 UIP-0007 的额外链上字段。

单 pass envelope 固定的是测试选中的 miner pass 的完整能量三元组；multi-pass envelope 额外携带 `candidate_passes` 与 `selection_rule = uip-0006:effective-energy-desc-pass-id-asc:v1`。当前测试固定选择排序第一项，以验证 UIP-0006 ordering contract；该测试策略不定义 ETHW block-selection policy，也不表示该 pass 已经赢得 PoW 出块竞争。

## 4. 校验流程

validator 风格脚本应始终分两步：

### 4.1 先校验 `external_state`

使用 payload 中记录的 `external_state` 构造 `ConsensusQueryContext`，再调用：

- `get_state_ref_at_height`

若这一步不成立，就不应继续校验 miner pass。

### 4.2 再校验 `miner_selection`

复用同一份 `ConsensusQueryContext`，调用：

- `get_pass_economic_profile`
- `get_candidate_set_view`
- Leader 存在 collab aggregate 时调用 `get_collab_breakdown`

并比对：

- `owner`
- `state`
- `raw_energy`
- `collab_contribution`
- `effective_energy`

`candidate_passes` 必须来自 UIP-0006 canonical `candidate_set_view`，不得从前端 raw leaderboard 或全部 active pass 手工拼装。profile/candidate/breakdown 必须返回同一 `external_state`，并能重算同一 `top_ranked_candidate` 和 collab aggregate。

这样 validator 视角会比“先查当前 state，再零散拼断言”更贴近真实实现。

## 5. 推荐的共享 helper

为了避免脚本继续手拼 JSON，建议在 `regtest_reorg_lib.sh` 中固定以下 helper：

1. `regtest_write_validator_payload_v1`
2. `regtest_validator_payload_expr`
3. `regtest_validator_payload_context_json`
4. `regtest_validate_validator_payload_success`
5. `regtest_validate_validator_payload_consensus_error`

这些 helper 负责把 payload 组装、上下文构造、RPC 调用和断言都收口成稳定 API。

## 6. 建议的专项脚本分层

### 6.1 Happy Path

- `regtest_live_ord_validator_block_body_e2e.sh`

覆盖：

- 原始历史高度验证通过
- BTC head 前进后，旧 payload 仍可验证通过

### 6.2 State Advance

- `regtest_live_ord_validator_block_body_state_advance.sh`

覆盖：

- payload 生成后，后续块对同一张 pass 触发真实变化：
  - `transfer`
  - `remint(prev)`
- 旧 payload 仍按各自历史 `context` 验证通过
- 当前 head 上同一业务对象的 owner / state / energy 已经和旧 payload 不同

### 6.3 Competing Payloads

- `regtest_live_ord_validator_block_body_competing_payloads.sh`

覆盖：

- 同一张 pass 在不同高度生成多份历史 payload
- 每份 payload 只能在各自 `expected_state` 下成立
- payload-A / payload-B 互相串用时返回 `SNAPSHOT_ID_MISMATCH`

### 6.4 Two-Pass Competition

- `regtest_live_ord_validator_block_body_two_pass_competition.sh`

覆盖：

- 同一历史高度 `H` 下存在两张合法候选 pass
- `test_selected_pass` 与 `candidate_passes` 被固定进同一份 block-body test envelope
- validator 在同一历史 `external_state` 下重查两张 pass 的 `snapshot / raw_energy / collab_contribution / effective_energy / state`
- validator 证明 `test_selected_pass` 等于 `top_ranked_candidate(candidate_passes, selection_rule)`，而不是只校验单张 pass
- 后续块让当时的 `top_ranked_candidate` 发生真实状态变化后，旧 envelope 仍按 `H` 通过

### 6.5 Two-Pass Real Energy Advantage

- `regtest_live_ord_validator_block_body_two_pass_energy_advantage.sh`

覆盖：

- 同一历史高度 `H` 下两张候选 pass 存在真实 `effective_energy` 差异，而不是都落到 `0` 后只走 tie-break
- `H` 时 `pass1.effective_energy > pass2.effective_energy`，test envelope 记录 `pass1` 为 `test_selected_pass`
- 后续块通过给 `pass2` owner 追加真实 BTC balance 并等待 energy 增长，使当前 head 的 `top_ranked_candidate` 变为 `pass2`
- validator 仍能按 `H` 的历史 `external_state` 证明旧 envelope 合法
- 新高度的 envelope 会切换到新的 `top_ranked_candidate`，从而证明历史与当前排序首项都能按各自上下文独立成立

### 6.6 Two-Pass Competing Payloads

- `regtest_live_ord_validator_block_body_two_pass_competing_payloads.sh`

覆盖：

- 同一组候选 pass 在 `H` 与 `H+1` 生成两份不同的多 pass payload
- 两份 envelope 的 `snapshot_id / system_state_id / candidate_count / test_selected_pass` 会发生变化
- 每份 payload 只能在各自历史视图下成立
- 跨高度串用 payload 时返回 `SNAPSHOT_ID_MISMATCH`

### 6.7 Two-Pass Reorg

- `regtest_live_ord_validator_block_body_two_pass_reorg.sh`

覆盖：

- 针对多 pass competition payload 执行 same-height reorg
- 旧 envelope 的 state ref、`test_selected_pass`、`candidate_passes` 全部在同一 historical context 下稳定返回 `SNAPSHOT_ID_MISMATCH`

### 6.8 Two-Pass Payload Tamper

- `regtest_live_ord_validator_block_body_two_pass_tamper.sh`

覆盖：

- 在不改 `external_state` 的前提下篡改 multi-pass envelope 的 `test_selected_pass`
- 基础历史 RPC 查询仍能重放真实链上状态
- 但 validator 本地的 `test_selected_pass == top_ranked_candidate(candidate_passes, selection_rule)` 校验必须失败

### 6.9 Three-Pass Candidate-Set

- `regtest_live_ord_validator_block_body_three_pass_candidate_set.sh`

覆盖：

- 同一历史高度下 3 张 pass 组成 `candidate_passes`
- envelope 显式记录 `test_selected_pass + candidate_passes + selection_rule`
- validator 在同一历史 context 下重查 3 张 pass，并重算 `top_ranked_candidate`
- 后续块让当前 `top_ranked_candidate` 真实发生 `transfer` 等状态变化，旧 envelope 仍按历史视图成立

### 6.10 Five-Pass Candidate-Set Tamper

- `regtest_live_ord_validator_block_body_five_pass_candidate_set_tamper.sh`

覆盖：

- 同一历史高度下 5 张 pass 组成更接近真实 validator 审计输入的 `candidate_passes`
- 在不改 `external_state` 的前提下篡改 envelope 中记录的 `test_selected_pass`
- validator 通过本地重算 `test_selected_pass == top_ranked_candidate(candidate_passes, selection_rule)` 识别篡改

### 6.11 Five-Pass Candidate-Set Reorg

- `regtest_live_ord_validator_block_body_five_pass_candidate_set_reorg.sh`

覆盖：

- same-height replacement 覆盖 5-pass candidate-set payload 所在高度
- 旧 envelope 的 `state ref / test_selected_pass / candidate_passes` 在同一 historical context 下稳定返回 `SNAPSHOT_ID_MISMATCH`

### 6.12 Protocol Version Mismatch

- `regtest_live_ord_validator_block_body_protocol_version_mismatch.sh`

覆盖：

- 在不改历史高度和业务对象的前提下，篡改 payload 的 `usdb_index_protocol_version`
- `state ref / economic profile / candidate set / collab breakdown` 都稳定返回 `PROTOCOL_VERSION_MISMATCH`

### 6.13 Formula Version Mismatch

- `regtest_live_ord_validator_block_body_formula_version_mismatch.sh`

覆盖：

- 在不改历史高度和业务对象的前提下，篡改 payload 的 `usdb_index_formula_version`
- `state ref / economic profile / candidate set / collab breakdown` 都稳定返回 `FORMULA_VERSION_MISMATCH`

### 6.14 Semantics Version Mismatch

- `regtest_live_ord_validator_block_body_semantics_version_mismatch.sh`

覆盖：

- 在不改历史高度和业务对象的前提下，篡改 payload 的 `balance_history_semantics_version`
- 历史 context 路径稳定返回 `VERSION_MISMATCH`

### 6.15 Candidate-Set Protocol Version Mismatch

- `regtest_live_ord_validator_block_body_candidate_set_protocol_version_mismatch.sh`

覆盖：

- 在 `test_selected_pass + candidate_passes` envelope 上篡改 `usdb_index_protocol_version`
- `state ref / test_selected_pass / candidate_passes` 的整条 candidate-set 校验路径都稳定返回 `PROTOCOL_VERSION_MISMATCH`

### 6.16 Candidate-Set Semantics Version Mismatch

- `regtest_live_ord_validator_block_body_candidate_set_semantics_version_mismatch.sh`

覆盖：

- 在 `test_selected_pass + candidate_passes` envelope 上篡改 `balance_history_semantics_version`
- `state ref / test_selected_pass / candidate_passes` 的整条 candidate-set 校验路径都稳定返回 `VERSION_MISMATCH`

### 6.17 API Version Mismatch

- `regtest_live_ord_validator_block_body_api_version_mismatch.sh`

覆盖：

- 在单 pass payload 上篡改 `balance_history_api_version`
- `state ref / economic profile / candidate set / collab breakdown` 都稳定返回 `VERSION_MISMATCH`

### 6.18 Version Matrix After Head Advance

- `regtest_live_ord_validator_block_body_version_matrix.sh`

覆盖：

- 在同一历史 payload 上同时构造 `api / semantics / protocol` 三类版本篡改
- BTC head 前进后，原 payload 继续通过
- API / semantics tampered payload 返回 `VERSION_MISMATCH`，protocol tampered payload 返回 `PROTOCOL_VERSION_MISMATCH`

### 6.19 Payload-Version Upgrade

- `regtest_live_ord_validator_block_body_payload_version_upgrade.sh`

覆盖：

- 同一条链上先生成 `payload_version=1.0.0` 的单 pass payload
- 后续高度再生成 `payload_version=1.1.0` 的 candidate-set payload
- validator 在同一升级窗口内同时接受旧 schema 和新 schema 的历史回放
- BTC head 再前进后，两代 payload 仍能按各自历史 context 独立成立

### 6.20 Payload-Version Upgrade Restart

- `regtest_live_ord_validator_block_body_payload_version_upgrade_restart.sh`

覆盖：

- 先生成 `v1.0` 和 `v1.1` 两代 payload
- `balance-history` 与 `usdb-indexer` 重启后，历史 `state ref` 与 mixed payload replay 仍然成立
- 证明 schema 升级窗口不依赖进程内缓存

### 6.21 Payload-Version Upgrade Reorg

- `regtest_live_ord_validator_block_body_payload_version_upgrade_reorg.sh`

覆盖：

- 先生成旧 `v1.0` payload，再生成新 `v1.1` payload
- same-height replacement 只覆盖新 payload 所在高度
- 旧 `v1.0` payload 仍成立，而新 `v1.1` payload 稳定返回 `SNAPSHOT_ID_MISMATCH`

### 6.22 Reorg

- `regtest_live_ord_validator_block_body_reorg.sh`

覆盖：

- same-height reorg 后，旧 payload 返回 `SNAPSHOT_ID_MISMATCH`

### 6.23 Retention / Missing History

- `regtest_live_ord_validator_block_body_retention.sh`

覆盖：

- retention floor 抬高后返回 `STATE_NOT_RETAINED`
- 历史辅助数据缺失时返回 `HISTORY_NOT_AVAILABLE`

### 6.24 Restart Consistency

- `regtest_live_ord_validator_block_body_restart_consistency.sh`

覆盖：

- 单 pass payload 生成后，`balance-history / usdb-indexer` 优雅重启
- 服务离线窗口内 BTC head 前进
- 重启追平后，旧 payload 仍按原历史 context 成立

### 6.25 Not-Ready Window

- `regtest_live_ord_validator_block_body_not_ready_window.sh`

覆盖：

- payload 已生成
- 服务重启并落后于当前 BTC head
- `rpc_alive=true` 但 `consensus_ready=false` 窗口内，validator 回放稳定返回 `SNAPSHOT_NOT_READY`
- 完成 catch-up 后，同一 payload 恢复可验证

### 6.26 Candidate-Set Crash Recovery

- `regtest_live_ord_validator_block_body_candidate_set_crash_recovery.sh`

覆盖：

- candidate-set payload 生成后，`balance-history / usdb-indexer` 被 `kill -9`
- 服务崩溃窗口内 BTC head 前进
- 重启追平后，历史 candidate-set payload 仍可按原 context 回放

## 7. 当前阶段的取舍

当前设计刻意不做这些事情：

- 不模拟完整 ETHW block header / parent hash / tx list
- 不把前端 raw leaderboard 当作 candidate set；`candidate_passes` 必须来自 UIP-0006 canonical `candidate_set_view`
- 不直接引入完整 USDB validator 实现

先把“单个 miner pass + 外部状态引用”的 block-body 校验链做扎实，收益最高，也更容易稳定回归。

## 8. 当前结论

当前仓库已经具备：

- 历史 state ref 查询
- profile/candidate/breakdown 的完整 historical context 校验
- raw/collab/effective energy、level/factor 和 formula-version mismatch 的 validator-style 测试链路

下一步工作的重点是执行完整 live/reorg/restart runner，并在大数据量 candidate/collab 集合上评估 cursor 查询成本和长时间重放稳定性。
