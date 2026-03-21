# USDB-Indexer Regtest: Live Ord Historical Validation History Not Available

## 目标

验证 ETHW 风格的历史校验请求在“高度仍在保留窗口内，但历史辅助数据缺失”时，能够稳定返回：

- `HISTORY_NOT_AVAILABLE`

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`

## 步骤

1. 使用 `ord` 真正 mint 一张 pass，并固定铭文确认高度 `H` 的历史 `state ref`。
2. head 前进到 `H+1` 后，再次用旧上下文验证，确认旧高度历史仍可读取。
3. 停止 `usdb-indexer`，手工删除本地 SQLite 中 `active_balance_snapshots` 在高度 `H` 的记录，用来模拟 retained window 内的历史辅助数据缺口。
4. 重启 `usdb-indexer`。
5. 再次用高度 `H` 的旧上下文请求三条接口。
6. 三条请求都必须返回：
   - `HISTORY_NOT_AVAILABLE`

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_historical_validation_history_not_available.sh
```
