# USDB-Indexer Regtest: Live Ord Validator Block-Body Semantics-Version Mismatch

## 目标

验证 validator block-body payload 的 `balance_history_semantics_version` 被篡改后，历史 context 校验会稳定落到 `VERSION_MISMATCH`。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`
- `ConsensusQueryContext.expected_state.balance_history_semantics_version`
- `VERSION_MISMATCH`

## 步骤

1. 真正 mint 一张 pass，并固定历史高度 `H` 的 validator payload。
2. 先校验原始 payload，必须通过。
3. 仅篡改 `external_state.balance_history_semantics_version`。
4. 再次按历史 context 校验，必须统一返回 `VERSION_MISMATCH`。

## 验收标准

1. 原始 payload 正常通过。
2. 篡改语义版本后，`state ref / pass snapshot / pass energy` 三条路径都返回 `VERSION_MISMATCH`。

