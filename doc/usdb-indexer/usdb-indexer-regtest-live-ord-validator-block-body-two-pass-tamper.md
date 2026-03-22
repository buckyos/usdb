# USDB Indexer Regtest: Live Ord Validator Block-Body Two-Pass Tamper

## 1. 目标

这条场景验证 multi-pass payload 即使引用了真实存在的历史 state ref，也不能随意篡改 winner。

## 2. 场景

1. 在高度 `H` 生成一份有效的 two-pass competition payload。
2. 复制 payload，但把 `miner_selection` 篡改成 loser。
3. 验证：
   - 篡改后的 payload 仍能通过基础历史 RPC 查询
   - 但 validator 本地按 `candidate_passes + selection_rule` 重算 winner 时，必须检测到不一致并拒绝

## 3. 对应脚本

- [regtest_live_ord_validator_block_body_two_pass_tamper.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_two_pass_tamper.sh)
