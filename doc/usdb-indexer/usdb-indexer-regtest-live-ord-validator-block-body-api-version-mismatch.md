# USDB-Indexer Regtest: Live Ord Validator Block-Body API-Version Mismatch

## 目标

验证 validator block-body payload 的 `balance_history_api_version` 被篡改后，历史 context 校验会稳定落到 `VERSION_MISMATCH`。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- `ConsensusQueryContext.expected_state.balance_history_api_version`
- `VERSION_MISMATCH`

## 步骤

1. 真正 mint 一张 pass，并固定历史高度 `H` 的 validator payload。
2. 先校验原始 payload，必须通过。
3. 仅篡改 `external_state.balance_history_api_version`。
4. 再次按历史 context 校验四个 UIP-0006 查询入口，必须统一返回 `VERSION_MISMATCH`。

## 验收标准

1. 原始 payload 的 profile/candidate/breakdown 可以在同一 external state 下交叉验证。
2. 篡改 API 版本后，四个入口都返回 `VERSION_MISMATCH`。
