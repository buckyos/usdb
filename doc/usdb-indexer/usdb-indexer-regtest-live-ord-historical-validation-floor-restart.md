# USDB-Indexer Regtest: Live Ord Historical Validation Floor Restart

## 目标

验证 ETHW 风格的历史校验请求在“未来历史保留窗口抬高”时，能够稳定返回：

- `STATE_NOT_RETAINED`

当前实现里，历史保留窗口下界由 `usdb-indexer` 的 `genesis_block_height` 统一定义；本场景通过更新该配置并重启服务，模拟未来 prune/floor 上升后的行为。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`

## 步骤

1. 使用 `ord` 真正 mint 一张 pass，并固定铭文确认高度 `H` 的历史 `state ref`。
2. head 前进到 `H+1` 后，再次用旧上下文验证，确认旧高度历史仍可读取。
3. 停止 `usdb-indexer`，把 `genesis_block_height` 提升到 `H+1`。
4. 重启 `usdb-indexer`。
5. 再次用高度 `H` 的旧上下文请求三条接口。
6. 三条请求都必须返回：
   - `STATE_NOT_RETAINED`

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_historical_validation_floor_restart.sh
```
