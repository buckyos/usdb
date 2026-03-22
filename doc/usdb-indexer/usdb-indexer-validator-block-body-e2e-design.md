# USDB Indexer Validator Block-Body E2E 设计

## 1. 目标

这份设计把现有的 historical-context 校验脚本进一步收敛成更贴近 ETHW validator 真实消费方式的测试模型。

目标不是立刻模拟整条 ETHW 链，而是先固定一份更像真实 block body 的外部状态 payload，并验证：

1. 出块方可以在 BTC 高度 `H` 生成一份稳定的 validator payload。
2. 验证方只依赖 payload 和 BTC RPC，就能按历史上下文重放校验。
3. BTC head 前进、same-height reorg、历史保留窗口变化、历史辅助数据缺失时，错误分流仍然稳定。

## 2. 当前脚手架可复用部分

当前 `usdb-indexer` regtest 栈已经具备这条链路的多数基础能力：

- `balance-history.get_state_ref_at_height`
- `usdb-indexer.get_state_ref_at_height`
- `usdb-indexer.get_pass_snapshot(context=...)`
- `usdb-indexer.get_pass_energy(context=...)`
- `ConsensusQueryContext`
- `STATE_NOT_RETAINED / HISTORY_NOT_AVAILABLE / *_MISMATCH`

共享脚手架 [regtest_reorg_lib.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh) 已经能复用：

- 服务生命周期与 readiness 等待
- `regtest_wait_usdb_state_ref_available`
- `regtest_get_usdb_state_ref_response`
- `regtest_build_consensus_context_json`
- `regtest_assert_usdb_consensus_error`
- live ord mint / send / reorg / restart helper

## 3. Validator Payload v1

建议统一一份更贴近 ETHW block body 的 payload 结构：

```json
{
  "payload_version": "1.0.0",
  "external_state": {
    "btc_height": 900123,
    "snapshot_id": "snapshot-...",
    "stable_block_hash": "000000...",
    "local_state_commit": "local-...",
    "system_state_id": "system-...",
    "usdb_index_protocol_version": "1.0.0"
  },
  "miner_selection": {
    "inscription_id": "txidi0",
    "owner": "76a914...",
    "state": "active",
    "energy": 123456789,
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
- `usdb_index_protocol_version`

其中：

- `snapshot_id` 锁定 upstream snapshot
- `local_state_commit` 锁定本地 durable core state
- `system_state_id` 锁定给 ETHW 消费的顶层系统状态

### 3.2 `miner_selection`

这一段描述出块方当时看到的 miner pass 选择结果：

- `inscription_id`
- `owner`
- `state`
- `energy`
- `resolved_height`
- `query_block_height`

当前阶段只固定单个 miner pass，不把排行榜、多 pass 选择逻辑、或 ETHW 链内其它字段一起引入。

## 4. 校验流程

validator 风格脚本应始终分两步：

### 4.1 先校验 `external_state`

使用 payload 中记录的 `external_state` 构造 `ConsensusQueryContext`，再调用：

- `get_state_ref_at_height`

若这一步不成立，就不应继续校验 miner pass。

### 4.2 再校验 `miner_selection`

复用同一份 `ConsensusQueryContext`，调用：

- `get_pass_snapshot`
- `get_pass_energy`

并比对：

- `owner`
- `state`
- `energy`

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
- `winner` 与 `candidates` 被固定进同一份 block-body payload
- validator 在同一历史 `external_state` 下重查两张 pass 的 `snapshot / energy / state`
- validator 证明 `winner` 满足 `max_energy + inscription_id` tie-break 选择规则，而不是只校验单张 pass
- 后续块让 winner 本身发生真实状态变化后，旧 payload 仍按 `H` 通过

### 6.5 Two-Pass Real Energy Advantage

- `regtest_live_ord_validator_block_body_two_pass_energy_advantage.sh`

覆盖：

- 同一历史高度 `H` 下两张候选 pass 存在真实 `energy` 差异，而不是都落到 `0` 后只走 tie-break
- `H` 时 `pass1.energy > pass2.energy`，payload 记录 `pass1` 为 winner
- 后续块通过给 `pass2` owner 追加真实 BTC balance 并等待 energy 增长，使当前 head 上的赢家翻转为 `pass2`
- validator 仍能按 `H` 的历史 `external_state` 证明旧 payload 合法
- 新高度的 payload 会切换到新的 winner，从而证明“历史赢家”和“当前赢家”都能按各自上下文独立成立

### 6.6 Two-Pass Competing Payloads

- `regtest_live_ord_validator_block_body_two_pass_competing_payloads.sh`

覆盖：

- 同一组候选 pass 在 `H` 与 `H+1` 生成两份不同的多 pass payload
- 两份 payload 的 `snapshot_id / system_state_id / candidate_count / winner` 会发生变化
- 每份 payload 只能在各自历史视图下成立
- 跨高度串用 payload 时返回 `SNAPSHOT_ID_MISMATCH`

### 6.7 Two-Pass Reorg

- `regtest_live_ord_validator_block_body_two_pass_reorg.sh`

覆盖：

- 针对多 pass competition payload 执行 same-height reorg
- 旧 payload 的 state ref、winner pass、candidate passes 全部在同一历史 context 下稳定返回 `SNAPSHOT_ID_MISMATCH`

### 6.8 Two-Pass Payload Tamper

- `regtest_live_ord_validator_block_body_two_pass_tamper.sh`

覆盖：

- 在不改 `external_state` 的前提下篡改 multi-pass payload 的 `winner`
- 基础历史 RPC 查询仍能重放真实链上状态
- 但 validator 本地的 `winner == recomputed(candidate_passes, selection_rule)` 校验必须失败

### 6.9 Reorg

- `regtest_live_ord_validator_block_body_reorg.sh`

覆盖：

- same-height reorg 后，旧 payload 返回 `SNAPSHOT_ID_MISMATCH`

### 6.10 Retention / Missing History

- `regtest_live_ord_validator_block_body_retention.sh`

覆盖：

- retention floor 抬高后返回 `STATE_NOT_RETAINED`
- 历史辅助数据缺失时返回 `HISTORY_NOT_AVAILABLE`

## 7. 当前阶段的取舍

当前设计刻意不做这些事情：

- 不模拟完整 ETHW block header / parent hash / tx list
- 不把 leaderboard 或多 pass 选择逻辑塞进第一版 validator payload
- 不直接引入完整 ETHW validator 实现

先把“单个 miner pass + 外部状态引用”的 block-body 校验链做扎实，收益最高，也更容易稳定回归。

## 8. 当前结论

当前仓库已经具备：

- 历史 state ref 查询
- `pass snapshot / energy` 的历史 context 校验
- validator-style payload e2e 的基本链路

下一步工作的重点是：

- 把 payload 结构标准化
- 把现有脚本中的 payload 组装和校验逻辑下沉到共享 helper
- 再在这个基础上扩出更细分的 block-body happy-path / reorg / retention 专项场景
