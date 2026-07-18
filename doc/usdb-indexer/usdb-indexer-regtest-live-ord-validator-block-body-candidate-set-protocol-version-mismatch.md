# USDB-Indexer Regtest: Live Ord Validator Block-Body Candidate-Set Protocol-Version Mismatch

## 目标

验证多 `pass` candidate-set payload 的 `usdb_index_protocol_version` 被篡改后，整条 candidate-set 历史校验路径会稳定落到 `PROTOCOL_VERSION_MISMATCH`。

## 覆盖点

- `winner + candidate_passes` payload
- `get_state_ref_at_height`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- canonical candidate-set 及每张 candidate profile 的批量历史 context 校验
- `PROTOCOL_VERSION_MISMATCH`

## 步骤

1. 在同一历史高度构造 3 张候选 pass，并生成 `winner + candidate_passes` payload。
2. 先校验原始 payload，必须通过。
3. 仅篡改 `external_state.usdb_index_protocol_version`。
4. 再次校验，要求四个 UIP-0006 view 及每张 candidate profile 都稳定返回 `PROTOCOL_VERSION_MISMATCH`。

## 验收标准

1. 原始 candidate-set payload 正常通过。
2. 篡改协议版本后，candidate-set 历史校验统一返回 `PROTOCOL_VERSION_MISMATCH`，而不是出现不一致的混合错误。
