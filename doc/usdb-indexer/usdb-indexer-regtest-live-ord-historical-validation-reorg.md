# USDB-Indexer Regtest: Live Ord Historical Validation Reorg

## 目标

验证 ETHW 风格的历史校验请求在以下情况下仍然行为稳定：

- 先固定高度 `H` 的历史 `state ref`
- BTC head 前进到 `H+1`
- 使用旧的 `(height, snapshot_id, system_state_id, local_state_commit)` 继续校验
- 同高度 replacement reorg 发生后，旧上下文被明确识别为 `SNAPSHOT_ID_MISMATCH`

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`

三者都携带相同 `ConsensusQueryContext`，模拟 ETHW 验块时的固定外部状态引用。

## 步骤

1. 使用 `ord` 真正 mint 一张 pass。
2. 在铭文确认高度 `H` 上读取历史 `state ref`，构造 `ConsensusQueryContext`。
3. 在高度 `H` 上验证：
   - `get_state_ref_at_height`
   - `get_pass_snapshot`
   - `get_pass_energy`
4. 再挖一个空块到 `H+1`，再次用旧上下文验证，结果仍应成功。
5. invalidate 原来的 `H`，同高度挖 replacement block，再补一个继续块。
6. 再次用旧上下文验证，三条请求都必须返回：
   - `SNAPSHOT_ID_MISMATCH`

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_historical_validation_reorg.sh
```
