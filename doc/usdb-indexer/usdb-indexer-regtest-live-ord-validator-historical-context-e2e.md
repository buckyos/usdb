# USDB-Indexer Regtest: Live Ord Validator Historical Context E2E

## 目标

验证更接近 ETHW validator 的完整链路：

1. 出块侧先固定一份外部状态引用：
   - `height`
   - `snapshot_id`
   - `system_state_id`
   - `local_state_commit`
   - `pass info`
2. 节点收到这份“区块载荷”后，必须按历史 `context` 验证，而不是按当前 head 查询。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`
- validator block payload 的构造与回放校验

## 步骤

1. 使用 `ord` 真正 mint 一张 pass。
2. 固定铭文业务所在高度 `H` 的历史 `state ref`、`pass snapshot`、`pass energy`。
3. 生成一份模拟的 validator block payload，记录：
   - `height`
   - `snapshot_id`
   - `stable_block_hash`
   - `local_state_commit`
   - `system_state_id`
   - `pass_id`
   - `owner/state/energy`
4. 用这份 payload 立即做一次“验块”：
   - `get_state_ref_at_height(H, context)`
   - `get_pass_snapshot(H, context)`
   - `get_pass_energy(H, context)`
5. BTC head 前进到 `H+1` 后，再次用同一份 payload 验证，仍必须通过。
6. 对高度 `H` 触发 same-height replacement reorg。
7. 再次用旧 payload 验证，三条查询都必须返回：
   - `SNAPSHOT_ID_MISMATCH`

## 验收标准

1. head 前进本身不能破坏旧 payload 的历史校验。
2. 历史高度被 replacement chain 改写后，旧 payload 必须稳定失效。
3. validator 依赖的不是“当前状态”，而是 payload 固定下来的历史 `context`。

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_historical_context_e2e.sh
```
