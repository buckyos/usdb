# USDB-Indexer Regtest: Live Ord Validator Block-Body Competing Payloads

## 目标

验证同一张 pass 在不同 BTC 高度形成的两份 validator block-body payload，必须只在各自历史 `context` 下成立，不能串用。

这条场景重点覆盖：

1. `H` 的 payload-A 对应 mint 后的历史状态。
2. `H+1` 的 payload-B 对应 transfer 后的历史状态。
3. A 和 B 都各自可验证。
4. A 的 `expected_state` 不能拿去验证 `H+1`。
5. B 的 `expected_state` 不能拿去验证 `H`。

## 覆盖点

- `validator payload v1`
- 同一张 pass 的 competing historical payloads
- `get_state_ref_at_height`
- `get_pass_snapshot(context=...)`
- `get_pass_energy(context=...)`
- `SNAPSHOT_ID_MISMATCH`

## 步骤

1. `wallet_a` mint `pass1`，在 `H` 生成 `payload_mint`。
2. 立即按 validator 流程校验 `payload_mint`，必须通过。
3. `wallet_a -> wallet_b` transfer `pass1`，在 `H+1` 生成 `payload_transfer`。
4. 立即按 validator 流程校验 `payload_transfer`，必须通过。
5. 验证两份 payload 的关键历史引用不同：
   - `snapshot_id`
   - `system_state_id`
   - `owner`
   - `state`
6. 用 `payload_mint` 的 `expected_state` 去校验 `H+1`，必须稳定返回 `SNAPSHOT_ID_MISMATCH`。
7. 用 `payload_transfer` 的 `expected_state` 去校验 `H`，必须稳定返回 `SNAPSHOT_ID_MISMATCH`。
8. BTC head 再前进 1 块，两份 payload 仍各自独立有效。

## 验收标准

1. `payload_mint` 只能在 mint-height 历史状态下成立。
2. `payload_transfer` 只能在 transfer-height 历史状态下成立。
3. 同一张 pass 在不同高度形成的历史 payload 不能串用。
4. BTC head 前进本身不能破坏各自历史 payload 的可验证性。

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_competing_payloads.sh
```
