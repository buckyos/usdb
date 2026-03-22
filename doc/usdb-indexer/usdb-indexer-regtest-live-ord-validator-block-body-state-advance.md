# USDB-Indexer Regtest: Live Ord Validator Block-Body State Advance

## 目标

验证 ETHW 风格 validator block-body payload 在后续 BTC 高度出现真实业务变化时，仍能按历史 `context` 稳定校验，而不会被当前 head 污染。

这条场景和单纯的 happy-path 区别在于：

1. `H` 生成的 payload 固定的是一张当时 `active` 的 pass。
2. 后续高度会对同一张 pass 触发真实变化：
   - `transfer`
   - `remint(prev)`
3. validator 仍必须能用 `H` 的历史 `context` 校验旧 payload。

## 覆盖点

- `validator payload v1`
- `get_state_ref_at_height`
- `get_pass_snapshot(context=...)`
- `get_pass_energy(context=...)`
- 历史 payload 在真实业务演进下的稳定性
- pass 的 owner / state / energy 在后续高度上的真实变化

## 步骤

1. `wallet_a` mint `pass1`，在 mint 高度 `H` 写出 `payload_mint`。
2. 立即按 validator 流程校验 `payload_mint`，必须通过。
3. `wallet_a -> wallet_b` 转移 `pass1`，在 `H+1`：
   - `pass1` 当前态变成 `dormant`
   - `owner` 切换为 `wallet_b`
   - 旧的 `payload_mint` 仍必须通过历史校验
4. 在 transfer 高度写出 `payload_transfer`。
5. `wallet_b` 执行 `remint(prev=pass1)`，在 `H+2`：
   - `pass1` 进入 `consumed`
   - 新的 `pass2` 变成 `active`
   - `payload_mint` 和 `payload_transfer` 都仍必须通过
6. 再挖一个空块到 `H+3`，再次验证历史 payload 不会被新 head 污染。

## 验收标准

1. `payload_mint` 在 `H`、`H+1`、`H+2`、`H+3` 都能稳定通过。
2. `payload_transfer` 在 `H+1` 生成后，在 `H+2`、`H+3` 也都能稳定通过。
3. `pass1` 在当前 head 上已经发生真实变化：
   - state 从 `active` 变成 `dormant`
   - owner 从 `wallet_a` 的历史 owner 标识切换为 `wallet_b` 的历史 owner 标识
   - 当前 head 下的查询结果不再等于旧 payload 中记录的 owner/state
4. `remint(prev)` 后，`pass1` 会进一步变成 `consumed`，而新 `pass2` 变成 `active`。
5. `pass2` 在 remint 高度上成为新的 `active` pass。
6. validator 依赖的是 payload 固定下来的历史 `context`，不是当前 head 的查询结果。

## 运行方式

```bash
BITCOIN_BIN_DIR=/home/bucky/btc/bitcoin-28.1/bin \
ORD_BIN=/home/bucky/ord/target/release/ord \
bash src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_state_advance.sh
```
