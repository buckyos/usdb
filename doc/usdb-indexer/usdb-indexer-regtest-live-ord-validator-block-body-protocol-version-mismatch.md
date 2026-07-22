# USDB-Indexer Regtest: Live Ord Validator Block-Body Active-Version-Set Mismatch

## 目标

验证 validator block-body test envelope 的 `active_version_set_id` 被篡改后，历史 context 校验会稳定落到 `ACTIVE_VERSION_SET_MISMATCH`。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- `ConsensusQueryContext.expected_state.active_version_set_id`
- `ACTIVE_VERSION_SET_MISMATCH` / `-32056`

## 步骤

1. 真正 mint 一张 pass，并固定历史高度 `H` 的 validator payload。
2. 先校验原始 payload，必须通过。
3. 仅篡改 `external_state.active_version_set_id`，不改变完整 `active_version_set`。
4. 再次按历史 context 校验四个 UIP-0006 查询入口，必须统一返回 `ACTIVE_VERSION_SET_MISMATCH`。

## 验收标准

1. 原始 payload 正常通过。
2. 篡改 active version set identity 后，`state ref / economic profile / candidate set / collab breakdown` 四条路径都返回 `ACTIVE_VERSION_SET_MISMATCH`。

> runner 文件名保留了早期 `protocol-version-mismatch` 描述，当前测试语义以本文和 UIP-0008 为准。
