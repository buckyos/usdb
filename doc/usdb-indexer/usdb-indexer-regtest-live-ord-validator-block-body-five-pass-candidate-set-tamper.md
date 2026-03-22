# USDB-Indexer Regtest Live Ord Validator Block-Body Five-Pass Candidate-Set Tamper

## 1. 目标

这条场景把 validator candidate-set 扩到 `5` 张 pass，并验证“外部状态不变，但 winner 被篡改”时，validator 本地重算能稳定识别。

## 2. 脚本入口

- [regtest_live_ord_validator_block_body_five_pass_candidate_set_tamper.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_five_pass_candidate_set_tamper.sh)

## 3. 场景步骤

1. 在同一条历史链上 mint 出 `5` 张候选 pass
2. 生成合法的 5-pass candidate-set payload
3. 先验证合法 payload 在历史 `context` 下通过
4. 在不改 `external_state` 的前提下，把 payload 里的 winner 改成一个 loser
5. validator 重新读取所有 candidate，并本地重算 winner

## 4. 通过标准

- 合法 payload 的 `candidate_passes` 长度等于 `5`
- 合法 payload 校验通过
- tampered payload 在历史 RPC 查询仍能重放真实链上状态
- 但 `winner != recomputed(candidate_passes, selection_rule)`，tamper 校验失败
