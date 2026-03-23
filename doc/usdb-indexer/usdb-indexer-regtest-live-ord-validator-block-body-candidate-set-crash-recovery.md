# usdb-indexer regtest: validator block-body candidate-set crash recovery

## Goal

验证 candidate-set payload 在服务崩溃后仍能恢复并回放。

## Coverage

- 生成 `payload_version=1.1.0` candidate-set payload
- `balance-history / usdb-indexer` 被 `kill -9`
- 服务离线期间 BTC head 继续前进
- 重启并追平后，历史 payload 仍按原 context 成立

## Script

- [regtest_live_ord_validator_block_body_candidate_set_crash_recovery.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_candidate_set_crash_recovery.sh)
