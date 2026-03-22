# USDB-Indexer Regtest Live Ord Validator Block-Body Five-Pass Candidate-Set Reorg

## 1. 目标

这条场景验证：当 `5-pass candidate-set payload` 所在高度发生 same-height replacement 时，旧 payload 的 `state ref / winner / candidate_passes` 都会在同一历史 context 下稳定落到 `SNAPSHOT_ID_MISMATCH`。

## 2. 脚本入口

- [regtest_live_ord_validator_block_body_five_pass_candidate_set_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_five_pass_candidate_set_reorg.sh)

## 3. 场景步骤

1. mint 出 `5` 张候选 pass，并在高度 `H` 生成 candidate-set payload
2. validator 先在原始历史视图下校验 payload 通过
3. 对高度 `H` 触发 same-height replacement
4. 等待 `balance-history` 和 `usdb-indexer` 收敛到 replacement chain
5. 用旧 payload 再次做历史校验

## 4. 通过标准

- replacement 后 `snapshot_id` 发生变化
- 旧 payload 的 `get_state_ref_at_height`
- 旧 payload 的所有 candidate `get_pass_snapshot / get_pass_energy`
- 都稳定返回 `SNAPSHOT_ID_MISMATCH`
