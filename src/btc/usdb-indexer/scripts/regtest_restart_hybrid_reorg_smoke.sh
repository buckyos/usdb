#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-restart-hybrid-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BTC_RPC_PORT="${BTC_RPC_PORT:-32432}"
BTC_P2P_PORT="${BTC_P2P_PORT:-32433}"
BH_RPC_PORT="${BH_RPC_PORT:-32410}"
USDB_RPC_PORT="${USDB_RPC_PORT:-32420}"
ORD_RPC_PORT="${ORD_RPC_PORT:-32430}"
WALLET_NAME="${WALLET_NAME:-usdbrestarthybridreorg}"
SCENARIO_START_HEIGHT="${SCENARIO_START_HEIGHT:-45}"
DEEP_REORG_DEPTH="${DEEP_REORG_DEPTH:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
REGTEST_LOG_PREFIX="[usdb-restart-hybrid-reorg]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

assert_empty_surface_state() {
  local block_height="$1"
  regtest_assert_usdb_active_balance_snapshot_zero "$block_height"
  regtest_assert_usdb_pass_stats_zero "$block_height"
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  if (( DEEP_REORG_DEPTH <= 0 )); then
    regtest_log "DEEP_REORG_DEPTH must be positive"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height stable_prefix_height affected_height target_height
  local original_affected_hash original_tip_hash replacement_affected_hash replacement_tip_hash
  local replacement_address round usdb_runtime_log_file
  local old_snapshot_resp old_snapshot_id old_tip_commit_resp old_tip_commit old_affected_commit_resp old_affected_commit
  local new_snapshot_resp new_snapshot_id new_tip_commit_resp new_tip_commit new_affected_commit_resp new_affected_commit

  mining_address="$(regtest_get_new_address)"
  current_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  stable_prefix_height=$((current_height + SCENARIO_START_HEIGHT))
  affected_height=$((stable_prefix_height + 1))
  target_height=$((stable_prefix_height + DEEP_REORG_DEPTH))

  regtest_log "Mining stable prefix to height=${stable_prefix_height}"
  regtest_mine_blocks "$((stable_prefix_height - current_height))" "$mining_address"
  if (( DEEP_REORG_DEPTH > 0 )); then
    regtest_log "Mining original tail to height=${target_height}"
    regtest_mine_blocks "$DEEP_REORG_DEPTH" "$mining_address"
  fi

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  usdb_runtime_log_file="${USDB_INDEXER_ROOT}/logs/usdb-indexer_rCURRENT.log"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$target_height"

  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$target_height"

  original_affected_hash="$(regtest_get_bitcoin_block_hash "$affected_height")"
  original_tip_hash="$(regtest_get_bitcoin_block_hash "$target_height")"
  old_tip_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${target_height}]")"
  old_tip_commit="$(regtest_json_expr "$old_tip_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  old_affected_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${affected_height}]")"
  old_affected_commit="$(regtest_json_expr "$old_affected_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  old_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  old_snapshot_id="$(regtest_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
  regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('local_synced_block_height', ''))" "$target_height"
  regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('balance_history_stable_height', ''))" "$target_height"
  regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('stable_block_hash', ''))" "$original_tip_hash"
  regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('latest_block_commit', ''))" "$old_tip_commit"

  assert_empty_surface_state "$affected_height"
  assert_empty_surface_state "$target_height"

  regtest_log "Stopping services before hybrid offline reorg: affected_height=${affected_height}, target_height=${target_height}"
  regtest_stop_usdb_indexer
  regtest_stop_balance_history

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_affected_hash"
  regtest_mine_empty_block "$(regtest_get_new_address)"

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$affected_height")"
  for round in $(seq 1 "$DEEP_REORG_DEPTH"); do
    replacement_address="$(regtest_get_new_address)"
    regtest_log "Mining hybrid replacement block ${round}/${DEEP_REORG_DEPTH} to address=${replacement_address}"
    regtest_mine_empty_block "$replacement_address"
  done

  replacement_affected_hash="$(regtest_get_bitcoin_block_hash "$affected_height")"
  replacement_tip_hash="$(regtest_get_bitcoin_block_hash "$target_height")"
  if [[ "$replacement_tip_hash" == "$original_tip_hash" ]]; then
    regtest_log "Hybrid replacement tip hash unexpectedly matches original tip hash"
    exit 1
  fi
  if [[ "$replacement_affected_hash" == "$original_affected_hash" ]]; then
    regtest_log "Hybrid replacement affected hash unexpectedly matches original affected hash"
    exit 1
  fi

  regtest_restart_balance_history
  regtest_wait_until_balance_history_synced_eq "$target_height"
  regtest_wait_until_balance_history_block_commit_hash "$affected_height" "$replacement_affected_hash"
  regtest_wait_until_balance_history_block_commit_hash "$target_height" "$replacement_tip_hash"
  new_affected_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${affected_height}]")"
  new_affected_commit="$(regtest_json_expr "$new_affected_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  new_tip_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${target_height}]")"
  new_tip_commit="$(regtest_json_expr "$new_tip_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"

  regtest_restart_usdb_indexer
  regtest_wait_until_usdb_synced_eq "$target_height"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash after restart hybrid reorg" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$replacement_tip_hash"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot latest block commit after restart hybrid reorg" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('latest_block_commit', ''))" \
    "$new_tip_commit"
  regtest_wait_until_rpc_expr_eq \
    "usdb pass block commit anchor at affected height after restart hybrid reorg" \
    regtest_rpc_call_usdb_indexer \
    "get_pass_block_commit" \
    "[{\"block_height\":${affected_height}}]" \
    "((data.get('result') or {}).get('balance_history_block_commit', ''))" \
    "$new_affected_commit"
  regtest_wait_until_rpc_expr_eq \
    "usdb pass block commit anchor at target height after restart hybrid reorg" \
    regtest_rpc_call_usdb_indexer \
    "get_pass_block_commit" \
    "[{\"block_height\":${target_height}}]" \
    "((data.get('result') or {}).get('balance_history_block_commit', ''))" \
    "$new_tip_commit"

  regtest_wait_until_file_contains \
    "usdb-indexer runtime log" \
    "$usdb_runtime_log_file" \
    "Detected upstream anchor drift, rolling back local indexer state"
  regtest_wait_until_file_contains \
    "usdb-indexer runtime log" \
    "$usdb_runtime_log_file" \
    "Resuming pending upstream reorg recovery"
  regtest_wait_until_file_contains \
    "usdb-indexer runtime log" \
    "$usdb_runtime_log_file" \
    "Pending upstream reorg recovery completed"

  new_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  new_snapshot_id="$(regtest_json_expr "$new_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
  if [[ "$new_snapshot_id" == "$old_snapshot_id" ]]; then
    regtest_log "Snapshot id did not change after restart hybrid reorg"
    exit 1
  fi
  if [[ "$new_tip_commit" == "$old_tip_commit" ]]; then
    regtest_log "Target-height block commit did not change after restart hybrid reorg"
    exit 1
  fi
  if [[ "$new_affected_commit" == "$old_affected_commit" ]]; then
    regtest_log "Affected-height block commit did not change after restart hybrid reorg"
    exit 1
  fi

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM pass_block_commits WHERE block_height >= ${affected_height} AND block_height <= ${target_height};" \
    "$DEEP_REORG_DEPTH" \
    "exactly one pass_block_commit row per replayed height in hybrid range"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height >= ${affected_height} AND block_height <= ${target_height};" \
    "$DEEP_REORG_DEPTH" \
    "exactly one active_balance_snapshot row per replayed height in hybrid range"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM state WHERE name = 'upstream_reorg_recovery_pending_height';" \
    "0" \
    "pending recovery marker cleared after restart hybrid reorg"

  assert_empty_surface_state "$affected_height"
  assert_empty_surface_state "$target_height"

  regtest_log "USDB indexer restart hybrid reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}, ${USDB_INDEXER_LOG_FILE}"
}

main "$@"
