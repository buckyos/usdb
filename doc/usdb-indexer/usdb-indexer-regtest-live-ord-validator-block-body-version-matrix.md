# USDB-Indexer Regtest: Live Ord Validator Block-Body Version Matrix

## 目标

验证同一历史 test envelope 在 `balance-history api / semantics / active version set identity` 三类期望下的回放分流，并证明 BTC head 前进不会掩盖对应 mismatch。

## 覆盖点

- 原始 payload 的历史校验
- `balance_history_api_version`
- `balance_history_semantics_version`
- `active_version_set_id`
- BTC head 前进后的历史回放

## 步骤

1. 真正 mint 一张 pass，并生成一份原始 validator payload。
2. 基于同一历史 payload 派生 3 份版本篡改变体：
   - API version
   - semantics version
   - active version set identity
3. 在原始高度验证：
   - 原 payload 通过
   - API / semantics 篡改返回 `VERSION_MISMATCH`
   - active version set identity 篡改返回 `ACTIVE_VERSION_SET_MISMATCH`
4. BTC head 再前进 1 块后重复同样断言。

## 验收标准

1. 原 payload 不受 head 前进影响，仍可按历史 context 回放。
2. 3 种 identity/version 篡改在 head 前进前后都稳定返回各自错误，不被 current head 覆盖。
