#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-reorg-smoke-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29332}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29333}"
BH_RPC_PORT="${BH_RPC_PORT:-29310}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29320}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29330}"
WALLET_NAME="${WALLET_NAME:-usdbreorg}"
TARGET_HEIGHT="${TARGET_HEIGHT:-40}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
REGTEST_LOG_PREFIX="[usdb-reorg-smoke]"

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

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address original_hash ancestor_hash replacement_hash
  local old_bh_commit_resp old_bh_commit new_bh_commit_resp new_bh_commit
  local usdb_sync_resp usdb_snapshot_resp usdb_pass_commit_resp replacement_address continue_address

  mining_address="$(regtest_get_new_address)"
  regtest_log "Mining ${TARGET_HEIGHT} blocks to address=${mining_address}"
  regtest_mine_blocks "$TARGET_HEIGHT" "$mining_address"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$TARGET_HEIGHT"
  regtest_wait_balance_history_consensus_ready

  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$TARGET_HEIGHT"
  regtest_wait_usdb_consensus_ready

  original_hash="$(regtest_get_bitcoin_block_hash "$TARGET_HEIGHT")"
  ancestor_hash="$(regtest_get_bitcoin_block_hash "$((TARGET_HEIGHT - 1))")"
  old_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${TARGET_HEIGHT}]")"
  old_bh_commit="$(regtest_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"

  regtest_assert_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('btc_block_hash', ''))" "$original_hash"

  usdb_sync_resp="$(regtest_rpc_call_usdb_indexer "get_sync_status" "[]")"
  regtest_assert_json_expr "$usdb_sync_resp" "(data.get('result') or {}).get('synced_block_height')" "$TARGET_HEIGHT"
  regtest_assert_json_expr "$usdb_sync_resp" "(data.get('result') or {}).get('balance_history_stable_height')" "$TARGET_HEIGHT"

  usdb_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$usdb_snapshot_resp" "(data.get('result') or {}).get('local_synced_block_height')" "$TARGET_HEIGHT"
  regtest_assert_json_expr "$usdb_snapshot_resp" "(data.get('result') or {}).get('balance_history_stable_height')" "$TARGET_HEIGHT"
  regtest_assert_json_expr "$usdb_snapshot_resp" "(data.get('result') or {}).get('stable_block_hash')" "$original_hash"
  regtest_assert_json_expr "$usdb_snapshot_resp" "(data.get('result') or {}).get('latest_block_commit')" "$old_bh_commit"

  usdb_pass_commit_resp="$(regtest_rpc_call_usdb_indexer "get_pass_block_commit" "[{\"block_height\":${TARGET_HEIGHT}}]")"
  regtest_assert_json_expr "$usdb_pass_commit_resp" "(data.get('result') or {}).get('block_height')" "$TARGET_HEIGHT"
  regtest_assert_json_expr "$usdb_pass_commit_resp" "(data.get('result') or {}).get('balance_history_block_height')" "$TARGET_HEIGHT"
  regtest_assert_json_expr "$usdb_pass_commit_resp" "(data.get('result') or {}).get('balance_history_block_commit')" "$old_bh_commit"

  assert_empty_surface_state "$TARGET_HEIGHT"

  regtest_log "Triggering height-regression reorg by invalidating tip height=${TARGET_HEIGHT}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_hash"

  regtest_wait_until_balance_history_synced_eq "$((TARGET_HEIGHT - 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_until_rpc_expr_eq \
    "balance-history snapshot stable hash" \
    regtest_rpc_call_balance_history \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$ancestor_hash"

  regtest_wait_until_usdb_synced_eq "$((TARGET_HEIGHT - 1))"
  regtest_wait_usdb_consensus_ready
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot local synced height" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('local_synced_block_height', ''))" \
    "$((TARGET_HEIGHT - 1))"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot upstream stable height" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('balance_history_stable_height', ''))" \
    "$((TARGET_HEIGHT - 1))"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$ancestor_hash"

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM pass_block_commits WHERE block_height > $((TARGET_HEIGHT - 1));" \
    "0" \
    "future pass_block_commits cleared after rollback"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height > $((TARGET_HEIGHT - 1));" \
    "0" \
    "future active_balance_snapshots cleared after rollback"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM state WHERE name = 'upstream_reorg_recovery_pending_height';" \
    "0" \
    "pending recovery marker cleared after rollback"

  replacement_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$replacement_address"
  replacement_hash="$(regtest_get_bitcoin_block_hash "$TARGET_HEIGHT")"
  if [[ "$replacement_hash" == "$original_hash" ]]; then
    regtest_log "Replacement hash unexpectedly matches original tip hash"
    exit 1
  fi

  regtest_wait_until_balance_history_synced_eq "$TARGET_HEIGHT"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_until_balance_history_block_commit_hash "$TARGET_HEIGHT" "$replacement_hash"
  new_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${TARGET_HEIGHT}]")"
  new_bh_commit="$(regtest_json_expr "$new_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"

  regtest_wait_until_usdb_synced_eq "$TARGET_HEIGHT"
  regtest_wait_usdb_consensus_ready
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash after replay" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$replacement_hash"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot latest block commit after replay" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('latest_block_commit', ''))" \
    "$new_bh_commit"
  regtest_wait_until_rpc_expr_eq \
    "usdb pass block commit anchor after replay" \
    regtest_rpc_call_usdb_indexer \
    "get_pass_block_commit" \
    "[{\"block_height\":${TARGET_HEIGHT}}]" \
    "((data.get('result') or {}).get('balance_history_block_commit', ''))" \
    "$new_bh_commit"

  assert_empty_surface_state "$TARGET_HEIGHT"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_blocks 1 "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((TARGET_HEIGHT + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_until_usdb_synced_eq "$((TARGET_HEIGHT + 1))"
  regtest_wait_usdb_consensus_ready

  regtest_log "USDB indexer height-regression reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}, ${USDB_INDEXER_LOG_FILE}"
}

main "$@"
