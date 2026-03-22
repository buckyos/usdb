# USDB-Indexer Regtest Live Ord Validator Block-Body Three-Pass Candidate-Set

## 1. 目标

这条场景把当前 multi-pass validator block-body 覆盖从 `two-pass` 推进到 `three-pass candidate-set`。

目标是验证：

- 同一历史高度 `H` 下，payload 显式携带 `winner + candidate_passes`
- validator 在同一历史 `external_state` 下重查 3 张 pass，并重算 winner
- 后续块让当前 winner 真实发生状态变化后，旧 payload 仍按 `H` 的历史视图成立

## 2. 脚本入口

- [regtest_live_ord_validator_block_body_three_pass_candidate_set.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_three_pass_candidate_set.sh)

## 3. 场景步骤

1. 分三次 mint 出 `pass1 / pass2 / pass3`
2. 在最后一张 pass 确认后的高度 `H` 启动 `balance-history` 和 `usdb-indexer`
3. 生成 3-pass candidate-set payload
4. validator 在高度 `H` 重查 3 张 pass 的 `snapshot / energy / state`
5. validator 本地重算 winner，并验证与 payload 一致
6. 后续块对当前 winner 执行真实 `transfer`
7. 旧 payload 仍按历史 `context` 验证通过

## 4. 通过标准

- `candidate_passes` 长度等于 `3`
- 初始历史视图下 `regtest_validate_validator_candidate_set_payload_success` 通过
- winner 在后续高度进入 `dormant`
- 旧 payload 在 `H+1` 和更高 head 下仍通过
