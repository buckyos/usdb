# usdb-indexer regtest: validator block-body not-ready window

## Goal

验证 `rpc_alive=true` 但 `consensus_ready=false` 的窗口里，历史 payload 回放会 fail-closed。

## Coverage

- payload 生成后服务停机并落后于 BTC head
- 重启后先进入 `rpc_alive=true, consensus_ready=false`
- 此窗口内回放稳定返回 `SNAPSHOT_NOT_READY`
- 完成 catch-up 后同一 payload 恢复可验证

## Script

- [regtest_live_ord_validator_block_body_not_ready_window.sh](/home/bucky/work/usdb/src/btc/usdb-indexer/scripts/regtest_live_ord_validator_block_body_not_ready_window.sh)
