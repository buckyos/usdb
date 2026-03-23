# usdb-indexer regtest: validator block-body restart consistency

## Goal

验证历史 `validator block-body payload` 在服务优雅重启后仍可回放。

## Coverage

- payload 生成后停止 `balance-history / usdb-indexer`
- 离线窗口内 BTC head 前进
- 重启追平后，旧 payload 仍按原历史 context 通过

## Script

- [regtest_live_ord_validator_block_body_restart_consistency.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_restart_consistency.sh)
