#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-restart-multi-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BTC_RPC_PORT="${BTC_RPC_PORT:-32332}"
BTC_P2P_PORT="${BTC_P2P_PORT:-32333}"
BH_RPC_PORT="${BH_RPC_PORT:-32310}"
USDB_RPC_PORT="${USDB_RPC_PORT:-32320}"
ORD_RPC_PORT="${ORD_RPC_PORT:-32330}"
WALLET_NAME="${WALLET_NAME:-usdbrestartmultireorg}"
TARGET_HEIGHT="${TARGET_HEIGHT:-40}"
REORG_ROUNDS="${REORG_ROUNDS:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
REGTEST_LOG_PREFIX="[usdb-restart-multi-reorg]"

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

  if (( REORG_ROUNDS <= 0 )); then
    regtest_log "REORG_ROUNDS must be positive"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height round
  local original_hash replacement_hash replacement_address continue_address
  local old_bh_commit_resp old_bh_commit new_bh_commit_resp new_bh_commit
  local old_snapshot_resp old_snapshot_id old_snapshot_commit old_pass_commit_resp old_pass_anchor
  local new_snapshot_resp new_snapshot_id new_snapshot_commit new_pass_commit_resp new_pass_anchor
  local usdb_runtime_log_file

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
  assert_empty_surface_state "$TARGET_HEIGHT"

  current_height="$TARGET_HEIGHT"
  for round in $(seq 1 "$REORG_ROUNDS"); do
    current_height=$((current_height + 1))
    regtest_log "Round ${round}/${REORG_ROUNDS}: mining original tip block at height=${current_height}"
    regtest_mine_blocks 1 "$mining_address"
    regtest_wait_until_balance_history_synced_eq "$current_height"
    regtest_wait_until_usdb_synced_eq "$current_height"

    original_hash="$(regtest_get_bitcoin_block_hash "$current_height")"
    old_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${current_height}]")"
    old_bh_commit="$(regtest_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
    regtest_assert_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('btc_block_hash', ''))" "$original_hash"

    old_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
    old_snapshot_id="$(regtest_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
    old_snapshot_commit="$(regtest_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('latest_block_commit', ''))")"
    regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('local_synced_block_height', ''))" "$current_height"
    regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('balance_history_stable_height', ''))" "$current_height"
    regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('stable_block_hash', ''))" "$original_hash"
    regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('latest_block_commit', ''))" "$old_bh_commit"

    old_pass_commit_resp="$(regtest_rpc_call_usdb_indexer "get_pass_block_commit" "[{\"block_height\":${current_height}}]")"
    old_pass_anchor="$(regtest_json_expr "$old_pass_commit_resp" "((data.get('result') or {}).get('balance_history_block_commit', ''))")"
    regtest_assert_json_expr "$old_pass_commit_resp" "((data.get('result') or {}).get('block_height', ''))" "$current_height"
    regtest_assert_json_expr "$old_pass_commit_resp" "((data.get('result') or {}).get('balance_history_block_commit', ''))" "$old_bh_commit"

    assert_empty_surface_state "$current_height"

    regtest_log "Round ${round}/${REORG_ROUNDS}: stopping services before offline tip replacement at height=${current_height}"
    regtest_stop_usdb_indexer
    regtest_stop_balance_history

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_hash"
    replacement_address="$(regtest_get_new_address)"
    regtest_mine_empty_block "$replacement_address"
    replacement_hash="$(regtest_get_bitcoin_block_hash "$current_height")"
    if [[ "$replacement_hash" == "$original_hash" ]]; then
      regtest_log "Round ${round}/${REORG_ROUNDS}: replacement hash unexpectedly matches original tip hash"
      exit 1
    fi

    regtest_restart_balance_history
    regtest_wait_until_balance_history_synced_eq "$current_height"
    regtest_wait_until_balance_history_block_commit_hash "$current_height" "$replacement_hash"
    new_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${current_height}]")"
    new_bh_commit="$(regtest_json_expr "$new_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"

    regtest_restart_usdb_indexer
    regtest_wait_until_usdb_synced_eq "$current_height"
    regtest_wait_until_rpc_expr_eq \
      "usdb snapshot stable hash after restart multi reorg round ${round}" \
      regtest_rpc_call_usdb_indexer \
      "get_snapshot_info" \
      "[]" \
      "((data.get('result') or {}).get('stable_block_hash', ''))" \
      "$replacement_hash"
    regtest_wait_until_rpc_expr_eq \
      "usdb snapshot latest block commit after restart multi reorg round ${round}" \
      regtest_rpc_call_usdb_indexer \
      "get_snapshot_info" \
      "[]" \
      "((data.get('result') or {}).get('latest_block_commit', ''))" \
      "$new_bh_commit"
    regtest_wait_until_rpc_expr_eq \
      "usdb pass block commit anchor after restart multi reorg round ${round}" \
      regtest_rpc_call_usdb_indexer \
      "get_pass_block_commit" \
      "[{\"block_height\":${current_height}}]" \
      "((data.get('result') or {}).get('balance_history_block_commit', ''))" \
      "$new_bh_commit"

    new_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
    new_snapshot_id="$(regtest_json_expr "$new_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
    new_snapshot_commit="$(regtest_json_expr "$new_snapshot_resp" "((data.get('result') or {}).get('latest_block_commit', ''))")"
    new_pass_commit_resp="$(regtest_rpc_call_usdb_indexer "get_pass_block_commit" "[{\"block_height\":${current_height}}]")"
    new_pass_anchor="$(regtest_json_expr "$new_pass_commit_resp" "((data.get('result') or {}).get('balance_history_block_commit', ''))")"

    if [[ "$new_snapshot_id" == "$old_snapshot_id" ]]; then
      regtest_log "Round ${round}/${REORG_ROUNDS}: snapshot id did not change after restart reorg"
      exit 1
    fi
    if [[ "$new_snapshot_commit" == "$old_snapshot_commit" ]]; then
      regtest_log "Round ${round}/${REORG_ROUNDS}: snapshot latest_block_commit did not change after restart reorg"
      exit 1
    fi
    if [[ "$new_pass_anchor" == "$old_pass_anchor" ]]; then
      regtest_log "Round ${round}/${REORG_ROUNDS}: pass block commit anchor did not change after restart reorg"
      exit 1
    fi

    regtest_assert_usdb_db_scalar \
      "SELECT COUNT(*) FROM pass_block_commits WHERE block_height = ${current_height};" \
      "1" \
      "exactly one pass_block_commit row at replacement height round ${round}"
    regtest_assert_usdb_db_scalar \
      "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height = ${current_height};" \
      "1" \
      "exactly one active_balance_snapshot row at replacement height round ${round}"
    regtest_assert_usdb_db_scalar \
      "SELECT COUNT(*) FROM state WHERE name = 'upstream_reorg_recovery_pending_height';" \
      "0" \
      "pending recovery marker cleared after restart multi reorg round ${round}"

    assert_empty_surface_state "$current_height"
  done

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

  continue_address="$(regtest_get_new_address)"
  regtest_mine_blocks 1 "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((current_height + 1))"
  regtest_wait_until_usdb_synced_eq "$((current_height + 1))"

  regtest_log "USDB indexer restart multi reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}, ${USDB_INDEXER_LOG_FILE}"
}

main "$@"
