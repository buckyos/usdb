# USDB-Indexer Regtest: Live Ord Validator Block-Body Reorg

## 目标

验证 validator block-body payload 在 same-height replacement reorg 下的失效语义：

1. 先生成一份结构化 `validator payload v1`
2. 在 reorg 前，这份 payload 可以按历史 context 校验通过
3. 对 payload 对应的 BTC 高度触发 same-height replacement reorg 后
4. 旧 payload 必须稳定返回 `SNAPSHOT_ID_MISMATCH`

## 覆盖点

- `get_state_ref_at_height`
- `get_pass_snapshot`
- `get_pass_energy`
- validator payload v1
- same-height reorg 后的 `SNAPSHOT_ID_MISMATCH`

## 步骤

1. 使用 `ord` 真正 mint 一张 pass。
2. 固定历史高度 `H` 的：
   - `state ref`
   - `pass snapshot`
   - `pass energy`
3. 写出一份 `validator block-body payload v1`。
4. 先用这份 payload 做一次历史校验，必须通过。
5. 对高度 `H` 所在分支触发 same-height replacement reorg。
6. 再次用旧 payload 校验，三条查询都必须返回：
   - `SNAPSHOT_ID_MISMATCH`

## 验收标准

1. 旧 payload 在 reorg 前可验证通过。
2. replacement chain 改写历史状态后，旧 payload 稳定失效。
3. validator 看到的是“历史状态已变”，而不是“当前 head 只是前进了”。

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_reorg.sh
```
