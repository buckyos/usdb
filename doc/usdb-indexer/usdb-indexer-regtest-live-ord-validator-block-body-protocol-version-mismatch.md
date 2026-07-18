# USDB-Indexer Regtest: Live Ord Validator Block-Body Protocol-Version Mismatch

## 目标

验证 validator block-body payload 的 `usdb_index_protocol_version` 被篡改后，历史 context 校验会稳定落到 `PROTOCOL_VERSION_MISMATCH`。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_economic_profile`
- `get_candidate_set_view`
- `get_collab_breakdown`
- `ConsensusQueryContext.expected_state.usdb_index_protocol_version`
- `PROTOCOL_VERSION_MISMATCH`

## 步骤

1. 真正 mint 一张 pass，并固定历史高度 `H` 的 validator payload。
2. 先校验原始 payload，必须通过。
3. 仅篡改 `external_state.usdb_index_protocol_version`。
4. 再次按历史 context 校验四个 UIP-0006 查询入口，必须统一返回 `PROTOCOL_VERSION_MISMATCH`。

## 验收标准

1. 原始 payload 正常通过。
2. 篡改协议版本后，`state ref / economic profile / candidate set / collab breakdown` 四条路径都返回 `PROTOCOL_VERSION_MISMATCH`。
