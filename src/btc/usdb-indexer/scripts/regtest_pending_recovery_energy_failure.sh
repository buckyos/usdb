#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-pending-recovery-energy-failure-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29832}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29833}"
BH_RPC_PORT="${BH_RPC_PORT:-29810}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29820}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29830}"
WALLET_NAME="${WALLET_NAME:-usdbpendingenergy}"
TARGET_HEIGHT="${TARGET_HEIGHT:-40}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
REGTEST_LOG_PREFIX="[usdb-pending-recovery-energy-failure]"
USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES="${USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES:-1}"

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
  local new_bh_commit_resp new_bh_commit replacement_address continue_address
  local usdb_runtime_log_file rollback_height

  rollback_height="$((TARGET_HEIGHT - 1))"
  mining_address="$(regtest_get_new_address)"
  regtest_log "Mining ${TARGET_HEIGHT} blocks to address=${mining_address}"
  regtest_mine_blocks "$TARGET_HEIGHT" "$mining_address"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  usdb_runtime_log_file="${USDB_INDEXER_ROOT}/logs/usdb-indexer_rCURRENT.log"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$TARGET_HEIGHT"

  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$TARGET_HEIGHT"

  original_hash="$(regtest_get_bitcoin_block_hash "$TARGET_HEIGHT")"
  ancestor_hash="$(regtest_get_bitcoin_block_hash "$rollback_height")"
  assert_empty_surface_state "$TARGET_HEIGHT"

  regtest_log "Triggering height-regression reorg with injected energy recovery failure at tip height=${TARGET_HEIGHT}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_hash"

  regtest_wait_until_balance_history_synced_eq "$rollback_height"
  regtest_wait_until_rpc_expr_eq \
    "balance-history snapshot stable hash after rollback" \
    regtest_rpc_call_balance_history \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$ancestor_hash"

  regtest_wait_until_file_contains \
    "usdb-indexer runtime log" \
    "$usdb_runtime_log_file" \
    "Injected reorg recovery energy failure"
  regtest_wait_until_usdb_db_scalar_eq \
    "SELECT value FROM state WHERE name = 'upstream_reorg_recovery_pending_height';" \
    "$rollback_height" \
    "pending recovery marker present after injected energy failure"
  regtest_wait_until_usdb_synced_eq "$rollback_height"

  regtest_wait_until_file_contains \
    "usdb-indexer runtime log" \
    "$usdb_runtime_log_file" \
    "Pending upstream reorg recovery completed"
  regtest_wait_until_usdb_db_scalar_eq \
    "SELECT value FROM state WHERE name = 'upstream_reorg_recovery_pending_height';" \
    "" \
    "pending recovery marker cleared after retry"

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM pass_block_commits WHERE block_height > ${rollback_height};" \
    "0" \
    "future pass_block_commits cleared after pending energy recovery retry"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height > ${rollback_height};" \
    "0" \
    "future active_balance_snapshots cleared after pending energy recovery retry"
  assert_empty_surface_state "$rollback_height"

  replacement_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$replacement_address"
  replacement_hash="$(regtest_get_bitcoin_block_hash "$TARGET_HEIGHT")"
  if [[ "$replacement_hash" == "$original_hash" ]]; then
    regtest_log "Replacement hash unexpectedly matches original tip hash"
    exit 1
  fi

  regtest_wait_until_balance_history_synced_eq "$TARGET_HEIGHT"
  regtest_wait_until_balance_history_block_commit_hash "$TARGET_HEIGHT" "$replacement_hash"
  new_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${TARGET_HEIGHT}]")"
  new_bh_commit="$(regtest_json_expr "$new_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"

  regtest_wait_until_usdb_synced_eq "$TARGET_HEIGHT"
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

  assert_empty_surface_state "$TARGET_HEIGHT"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_blocks 1 "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((TARGET_HEIGHT + 1))"
  regtest_wait_until_usdb_synced_eq "$((TARGET_HEIGHT + 1))"

  regtest_log "USDB indexer pending recovery energy failure test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}, ${USDB_INDEXER_LOG_FILE}"
}

main "$@"
