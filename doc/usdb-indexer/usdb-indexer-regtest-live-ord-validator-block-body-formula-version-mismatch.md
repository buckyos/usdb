# USDB-Indexer Regtest: Live Ord Validator Block-Body Formula-Version Mismatch

## 目标

验证 validator payload 的 `usdb_index_formula_version` 被篡改后，UIP-0006 historical context 查询稳定返回 `FORMULA_VERSION_MISMATCH`。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- `ConsensusQueryContext.expected_state.usdb_index_formula_version`
- `FORMULA_VERSION_MISMATCH` / `-32052`

## 步骤

1. 通过真实 ord mint 一张 standard pass，并固定历史高度 `H` 的 validator payload。
2. 校验未修改的 payload，要求 state ref/profile/candidate/breakdown 全部通过。
3. 只把 `external_state.usdb_index_formula_version` 改为不受支持的值。
4. 使用篡改后的 context 重放四个 UIP-0006 查询入口。

## 验收标准

1. 原始 payload 正常通过。
2. 四个 historical/economic view 入口都返回错误码 `-32052` 和 `FORMULA_VERSION_MISMATCH`。
3. 不得把 formula mismatch 降级为普通参数错误或其他 version mismatch。
