#!/usr/bin/env bash
# Distinguish a temporarily restored Core prefix from changed committed BTC history.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-core-recovery-XXXXXX)}"
BTC_RPC_PORT="${BTC_RPC_PORT:-31932}"
BTC_P2P_PORT="${BTC_P2P_PORT:-31933}"
BH_RPC_PORT="${BH_RPC_PORT:-31910}"
USDB_INDEXER_RPC_PORT="${USDB_INDEXER_RPC_PORT:-31920}"
REGTEST_LOG_PREFIX="[core-recovery]"

# shellcheck source=regtest_reorg_lib.sh
source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

assert_preserved_wait() {
  regtest_wait_until_rpc_expr_eq "BH waiting for Core recovery" \
    regtest_rpc_call_balance_history get_readiness '[]' \
    "'UpstreamRecoveryPending' in data.get('result', {}).get('blockers', [])" True
  # Observe multiple downstream polls: this must never become a durable reorg epoch.
  for _ in $(seq 1 8); do
    local bh indexer
    bh="$(regtest_rpc_call_balance_history get_readiness '[]')"
    indexer="$(regtest_rpc_call_usdb_indexer get_readiness '[]')"
    regtest_assert_json_expr "$bh" "data['result']['stable_height']" 40
    regtest_assert_json_expr "$bh" "data['result']['latest_block_commit']" "$original_commit"
    regtest_assert_json_expr "$bh" "data['result']['consensus_ready']" False
    regtest_assert_json_expr "$bh" "data['result']['query_ready']" False
    regtest_assert_json_expr "$indexer" "data['result']['synced_block_height']" 40
    regtest_assert_json_expr "$indexer" "data['result']['upstream_reorg_epoch']" 0
    sleep 1
  done
}

assert_recovered() {
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready
  local snapshot indexer state_ref
  snapshot="$(regtest_rpc_call_balance_history get_snapshot_info '[]')"
  indexer="$(regtest_rpc_call_usdb_indexer get_readiness '[]')"
  regtest_assert_json_expr "$snapshot" "data['result']['stable_height']" 40
  state_ref="$(regtest_rpc_call_balance_history get_state_ref_at_height '[{"block_height":40}]')"
  regtest_assert_json_expr "$state_ref" "data['result']['snapshot_id']" "$original_snapshot"
  regtest_assert_json_expr "$snapshot" "data['result']['latest_block_commit']" "$original_commit"
  regtest_assert_json_expr "$indexer" "data['result']['upstream_reorg_epoch']" 0
}

main() {
  trap regtest_cleanup EXIT
  regtest_resolve_bitcoin_binaries
  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet
  local address original_tip original_stable snapshot original_commit original_snapshot replacement
  address="$(regtest_get_new_address)"
  regtest_mine_blocks "$((40 + BTC_STABLE_LAG_BLOCKS))" "$address"
  original_tip="$(regtest_get_bitcoin_block_hash "$((40 + BTC_STABLE_LAG_BLOCKS))")"
  original_stable="$(regtest_get_bitcoin_block_hash 40)"
  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq 40
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq 40
  regtest_wait_usdb_consensus_ready
  snapshot="$(regtest_rpc_call_balance_history get_snapshot_info '[]')"
  original_commit="$(regtest_json_expr "$snapshot" "data['result']['latest_block_commit']")"
  original_snapshot="$(regtest_json_expr "$(regtest_rpc_call_balance_history get_state_ref_at_height '[{"block_height":40}]')" "data['result']['snapshot_id']")"

  # Reboot with one confirmation block temporarily absent, then restore that same block.
  regtest_log "Testing restart with a lower confirmation target and unchanged history"
  regtest_stop_usdb_indexer
  regtest_stop_balance_history
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_tip"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  assert_preserved_wait
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" reconsiderblock "$original_tip"
  assert_recovered

  # An even shorter but matching prefix is inconclusive, not evidence of a fork.
  regtest_log "Testing restart with Core behind the committed height"
  regtest_stop_usdb_indexer
  regtest_stop_balance_history
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_stable"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  assert_preserved_wait
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" reconsiderblock "$original_stable"
  assert_recovered

  # A replacement hash at committed height 40 must still trigger the durable epoch.
  regtest_log "Testing a real replacement of committed history while the confirmation target is lower"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_stable"
  address="$(regtest_get_new_address)"
  regtest_mine_blocks "$BTC_STABLE_LAG_BLOCKS" "$address"
  replacement="$(regtest_get_bitcoin_block_hash 40)"
  [[ "$replacement" != "$original_stable" ]]
  regtest_wait_until_balance_history_synced_eq 39
  regtest_wait_until_usdb_synced_eq 39
  regtest_assert_json_expr "$(regtest_rpc_call_usdb_indexer get_readiness '[]')" \
    "data['result']['upstream_reorg_epoch']" 1
  regtest_mine_blocks 1 "$address"
  regtest_wait_until_balance_history_synced_eq 40
  regtest_wait_until_usdb_synced_eq 40
  regtest_wait_until_balance_history_block_commit_hash 40 "$replacement"
  regtest_log "Core recovery regression passed: matching history preserved; real fork advanced epoch. Logs: $WORK_DIR"
}

main "$@"
