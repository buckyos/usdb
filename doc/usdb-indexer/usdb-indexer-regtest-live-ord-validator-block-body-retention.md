# USDB-Indexer Regtest: Live Ord Validator Block-Body Retention

## 目标

验证 validator block-body payload 在历史保留窗口变化和历史辅助数据缺失下的错误分流：

1. 先生成一份结构化 `validator payload v1`
2. payload 在原始历史高度和 head 前进后都必须校验通过
3. retention floor 上升后，旧 payload 必须返回 `STATE_NOT_RETAINED`
4. 恢复保留窗口并制造 retained 历史辅助数据缺失后，旧 payload 必须返回 `HISTORY_NOT_AVAILABLE`

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`
- validator payload v1
- `STATE_NOT_RETAINED`
- `HISTORY_NOT_AVAILABLE`

## 步骤

1. 使用 `ord` 真正 mint 一张 pass。
2. 固定历史高度 `H` 的：
   - `state ref`
   - `pass snapshot`
   - `pass energy`
3. 写出一份 `validator block-body payload v1`。
4. 先做 happy-path 校验，必须通过。
5. BTC head 前进到 `H+1` 后，再做一次 happy-path 校验，仍必须通过。
6. 抬高 `genesis_block_height`，模拟历史保留窗口上升。
7. 再次校验旧 payload，三条查询都必须返回：
   - `STATE_NOT_RETAINED`
8. 恢复历史保留窗口，删除 retained 高度的 `active_balance_snapshots`。
9. 再次校验旧 payload，三条查询都必须返回：
   - `HISTORY_NOT_AVAILABLE`

## 验收标准

1. payload 在历史仍被保留且辅助数据完整时可稳定通过。
2. 落到保留窗口之外时稳定返回 `STATE_NOT_RETAINED`。
3. 仍在保留窗口内但辅助数据缺失时稳定返回 `HISTORY_NOT_AVAILABLE`。

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_retention.sh
```
