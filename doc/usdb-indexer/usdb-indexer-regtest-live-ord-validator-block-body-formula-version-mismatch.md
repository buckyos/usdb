# USDB-Indexer Regtest: Live Ord Validator Block-Body Active-Version-Set Mismatch Alias

## 目标

该 runner 是早期 formula-version 场景的入口别名。全局 `usdb_index_formula_version` 已删除；当前场景通过篡改 `active_version_set_id`，验证 UIP-0006 historical context 查询稳定返回 `ACTIVE_VERSION_SET_MISMATCH`。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- `ConsensusQueryContext.expected_state.active_version_set_id`
- `ACTIVE_VERSION_SET_MISMATCH` / `-32056`

## 步骤

1. 通过真实 ord mint 一张 standard pass，并固定历史高度 `H` 的 validator payload。
2. 校验未修改的 payload，要求 state ref/profile/candidate/breakdown 全部通过。
3. 只篡改 `external_state.active_version_set_id`，保留完整 `active_version_set` 不变。
4. 使用篡改后的 context 重放四个 UIP-0006 查询入口。

## 验收标准

1. 原始 payload 正常通过。
2. 四个 historical/economic view 入口都返回错误码 `-32056` 和 `ACTIVE_VERSION_SET_MISMATCH`。
3. 不得把 active version set identity mismatch 降级为普通参数错误或其它 version mismatch。

> 文件名和 runner 名称仅是尚待清理的测试资产命名，不构成兼容接口。
