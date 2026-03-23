# USDB-Indexer Regtest: Live Ord Validator Block-Body Candidate-Set Semantics-Version Mismatch

## 目标

验证多 `pass` candidate-set payload 的 `balance_history_semantics_version` 被篡改后，整条 candidate-set 历史校验路径会稳定落到 `VERSION_MISMATCH`。

## 覆盖点

- `winner + candidate_passes` payload
- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`
- candidate-set 批量历史 context 校验
- `VERSION_MISMATCH`

## 步骤

1. 在同一历史高度构造 3 张候选 pass，并生成 `winner + candidate_passes` payload。
2. 先校验原始 payload，必须通过。
3. 仅篡改 `external_state.balance_history_semantics_version`。
4. 再次校验，要求 `state ref / winner / candidate_passes` 都稳定返回 `VERSION_MISMATCH`。

