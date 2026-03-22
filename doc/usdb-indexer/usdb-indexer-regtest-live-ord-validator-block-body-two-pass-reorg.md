# USDB Indexer Regtest: Live Ord Validator Block-Body Two-Pass Reorg

## 1. 目标

这条场景验证针对两张候选 pass 生成的 competition payload，在 same-height reorg 之后会被整体判定为历史状态不匹配。

## 2. 场景

1. 在高度 `H` 生成 multi-pass competition payload。
2. 先确认 payload 在原链上可通过。
3. 对高度 `H` 触发 same-height replacement reorg。
4. 验证旧 payload 的：
   - `get_state_ref_at_height`
   - winner `get_pass_snapshot / get_pass_energy`
   - candidate `get_pass_snapshot / get_pass_energy`
   都稳定返回 `SNAPSHOT_ID_MISMATCH`。

## 3. 对应脚本

- [regtest_live_ord_validator_block_body_two_pass_reorg.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_two_pass_reorg.sh)
