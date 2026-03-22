# USDB Indexer Regtest: Live Ord Validator Block-Body Two-Pass Competing Payloads

## 1. 目标

这条场景验证同一组候选 pass 在不同历史高度生成的两份 multi-pass payload 只能在各自历史视图下成立，不能串用。

## 2. 场景

1. 在高度 `H` 形成两张候选 pass 的 competition payload。
2. 到 `H+1` 让 `H` 时的 winner 发生真实状态变化。
3. 在 `H+1` 重新生成一份 competition payload。
4. 分别验证：
   - 两份 payload 各自通过
   - `snapshot_id / system_state_id / candidate_count / winner` 发生变化
   - 用 `H` 的 payload 去校验 `H+1` 返回 `SNAPSHOT_ID_MISMATCH`
   - 用 `H+1` 的 payload 去校验 `H` 返回 `SNAPSHOT_ID_MISMATCH`

## 3. 对应脚本

- [regtest_live_ord_validator_block_body_two_pass_competing_payloads.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_two_pass_competing_payloads.sh)
