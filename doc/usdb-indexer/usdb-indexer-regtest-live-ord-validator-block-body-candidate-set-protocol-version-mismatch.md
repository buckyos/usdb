# USDB-Indexer Regtest: Live Ord Validator Block-Body Candidate-Set Active-Version-Set Mismatch

## 目标

验证多 `pass` candidate-set test envelope 的 `active_version_set_id` 被篡改后，整条 candidate-set 历史校验路径会稳定落到 `ACTIVE_VERSION_SET_MISMATCH`。

## 覆盖点

- `winner + candidate_passes` payload
- `get_state_ref_at_height`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- canonical candidate-set 及每张 candidate profile 的批量历史 context 校验
- `ACTIVE_VERSION_SET_MISMATCH` / `-32056`

## 步骤

1. 在同一历史高度构造 3 张候选 pass，并生成 `winner + candidate_passes` payload。
2. 先校验原始 payload，必须通过。
3. 仅篡改 `external_state.active_version_set_id`，不改变完整 `active_version_set`。
4. 再次校验，要求四个 UIP-0006 view 及每张 candidate profile 都稳定返回 `ACTIVE_VERSION_SET_MISMATCH`。

## 验收标准

1. 原始 candidate-set payload 正常通过。
2. 篡改 active version set identity 后，candidate-set 历史校验统一返回 `ACTIVE_VERSION_SET_MISMATCH`，而不是出现不一致的混合错误。

> runner 文件名保留了早期 `protocol-version-mismatch` 描述，当前测试语义以本文和 UIP-0008 为准；后续脚本命名清理不影响协议行为。
