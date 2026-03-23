# USDB-Indexer Regtest: Live Ord Validator Block-Body Version Matrix

## 目标

验证同一历史 payload 在 `api / semantics / protocol` 三类版本期望下的回放分流，并证明 BTC head 前进不会掩盖这些 `VERSION_MISMATCH`。

## 覆盖点

- 原始 payload 的历史校验
- `balance_history_api_version`
- `balance_history_semantics_version`
- `usdb_index_protocol_version`
- BTC head 前进后的历史回放

## 步骤

1. 真正 mint 一张 pass，并生成一份原始 validator payload。
2. 基于同一历史 payload 派生 3 份版本篡改变体：
   - API version
   - semantics version
   - protocol version
3. 在原始高度验证：
   - 原 payload 通过
   - 3 份篡改 payload 都返回 `VERSION_MISMATCH`
4. BTC head 再前进 1 块后重复同样断言。

## 验收标准

1. 原 payload 不受 head 前进影响，仍可按历史 context 回放。
2. 3 种版本篡改在 head 前进前后都稳定返回 `VERSION_MISMATCH`。
