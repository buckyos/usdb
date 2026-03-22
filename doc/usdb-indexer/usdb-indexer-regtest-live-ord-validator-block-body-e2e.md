# USDB-Indexer Regtest: Live Ord Validator Block-Body E2E

## 目标

验证第一条拆开的 validator block-body happy-path：

1. 出块侧生成一份结构化 `validator payload v1`
2. 节点按历史 `context` 验证这份 payload
3. BTC head 前进后，同一份 payload 仍然可以通过历史校验

这条场景只覆盖 happy-path，不夹带 reorg 或 retention 语义。

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`
- `validator payload v1`
- BTC head 前进后的历史上下文稳定性

## 步骤

1. 使用 `ord` 真正 mint 一张 pass。
2. 固定铭文业务所在高度 `H` 的历史：
   - `state ref`
   - `pass snapshot`
   - `pass energy`
3. 写出一份 `validator block-body payload v1`：
   - `external_state`
   - `miner_selection`
4. 用这份 payload 立即做一次 validator 风格校验，必须通过。
5. BTC head 前进到 `H+1`。
6. 再次用同一份 payload 做校验，仍必须通过。

## 验收标准

1. payload 在原始历史高度上可验证通过。
2. BTC head 前进本身不能破坏旧 payload 的历史校验。
3. validator 依赖的是 payload 固定下来的历史 `context`，不是当前 head 查询结果。

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_e2e.sh
```
