#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-install-retry-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-source}"
TARGET_BALANCE_HISTORY_ROOT="${TARGET_BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-target}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29732}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29733}"
BH_RPC_PORT="${BH_RPC_PORT:-29710}"
TARGET_BH_RPC_PORT="${TARGET_BH_RPC_PORT:-29711}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotretry}"
TARGET_MAX_SYNC_HEIGHT="${TARGET_MAX_SYNC_HEIGHT:-102}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-source.log}"
TARGET_BALANCE_HISTORY_LOG_FILE="${TARGET_BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-target.log}"
REGTEST_LOG_PREFIX="[snapshot-install-retry]"

source "${SCRIPT_DIR}/regtest_lib.sh"

regtest_assert_json_expr() {
  local response="$1"
  local expression="$2"
  local expected="$3"
  local actual

  actual="$(printf '%s' "$response" | python3 -c "import json,sys; data=json.load(sys.stdin); print(${expression})")"
  regtest_log "RPC assertion: expr=${expression}, expected=${expected}, actual=${actual}"
  if [[ "$actual" != "$expected" ]]; then
    regtest_log "RPC assertion failed. response=${response}"
    exit 1
  fi
}

regtest_expect_cli_failure() {
  local root_dir="$1"
  local output_file="$2"
  shift 2

  if regtest_run_balance_history_cli "$root_dir" "$@" >"$output_file" 2>&1; then
    regtest_log "Expected CLI failure but command succeeded: $*"
    cat "$output_file" >&2 || true
    exit 1
  fi
}

regtest_assert_artifact_counts() {
  local root_dir="$1"
  local expected_staging="$2"
  local expected_backup="$3"
  local staging_count backup_count

  staging_count="$(find "$root_dir" -maxdepth 1 -type d -name 'snapshot_install_staging_*' | wc -l | tr -d ' ')"
  backup_count="$(find "$root_dir" -maxdepth 1 -type d -name 'db_backup_snapshot_install_*' | wc -l | tr -d ' ')"
  regtest_log "Install artifact assertion: root=${root_dir}, staging_count=${staging_count}, backup_count=${backup_count}"
  if [[ "$staging_count" != "$expected_staging" || "$backup_count" != "$expected_backup" ]]; then
    find "$root_dir" -maxdepth 1 \( -type d -name 'snapshot_install_staging_*' -o -type d -name 'db_backup_snapshot_install_*' \) -print >&2 || true
    exit 1
  fi
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_require_cmd sha256sum

  regtest_ensure_workspace_dirs
  mkdir -p "$TARGET_BALANCE_HISTORY_ROOT"

  local mining_address target_address
  local target_script_hash txid_old txid_new txid_after_restore
  local vout_old vout_new vout_after_restore
  local old_height snapshot_height post_restore_height
  local old_block_hash snapshot_block_hash snapshot_commit snapshot_file snapshot_hash wrong_hash
  local wrong_hash_output resp

  wrong_hash_output="$WORK_DIR/install_wrong_hash_retry.out"

  regtest_start_bitcoind
  regtest_ensure_wallet

  mining_address="$(regtest_get_new_address)"
  target_address="$(regtest_get_new_address)"
  target_script_hash="$(regtest_address_to_script_hash "$target_address")"
  regtest_ensure_mature_funds "$mining_address"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$COINBASE_MATURITY"

  txid_old="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$target_address" 0.4)"
  regtest_mine_blocks 1 "$mining_address"
  old_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$old_height"
  vout_old="$(regtest_get_tx_vout_for_address "$txid_old" "$target_address")"
  regtest_lock_wallet_outpoint "$txid_old" "$vout_old"

  txid_new="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$target_address" 0.2)"
  regtest_mine_blocks 1 "$mining_address"
  snapshot_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$snapshot_height"
  vout_new="$(regtest_get_tx_vout_for_address "$txid_new" "$target_address")"
  regtest_lock_wallet_outpoint "$txid_new" "$vout_new"

  snapshot_block_hash="$(regtest_get_block_hash_by_height "$snapshot_height")"
  resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${snapshot_height}]")"
  snapshot_commit="$(printf '%s' "$resp" | python3 -c "import json,sys; data=json.load(sys.stdin); print(data['result']['block_commit'])")"

  regtest_stop_balance_history
  regtest_run_balance_history_cli "$BALANCE_HISTORY_ROOT" create-snapshot --block-height "$snapshot_height"

  snapshot_file="$BALANCE_HISTORY_ROOT/snapshots/snapshot_${snapshot_height}.db"
  snapshot_hash="$(sha256sum "$snapshot_file" | awk '{print $1}')"
  wrong_hash="0${snapshot_hash:1}"
  old_block_hash="$(regtest_get_block_hash_by_height "$old_height")"

  BALANCE_HISTORY_ROOT="$TARGET_BALANCE_HISTORY_ROOT"
  BALANCE_HISTORY_LOG_FILE="$TARGET_BALANCE_HISTORY_LOG_FILE"
  BH_RPC_PORT="$TARGET_BH_RPC_PORT"
  regtest_create_balance_history_config
  regtest_config_set_max_sync_block_height "$BALANCE_HISTORY_ROOT/config.toml" "$TARGET_MAX_SYNC_HEIGHT"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$TARGET_MAX_SYNC_HEIGHT"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$TARGET_MAX_SYNC_HEIGHT"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$old_block_hash"

  regtest_stop_balance_history
  regtest_expect_cli_failure "$BALANCE_HISTORY_ROOT" "$wrong_hash_output" install-snapshot --file "$snapshot_file" --hash "$wrong_hash"
  regtest_assert_artifact_counts "$BALANCE_HISTORY_ROOT" "0" "0"

  regtest_run_balance_history_cli "$BALANCE_HISTORY_ROOT" install-snapshot --file "$snapshot_file" --hash "$snapshot_hash"
  regtest_assert_artifact_counts "$BALANCE_HISTORY_ROOT" "0" "1"
  regtest_config_set_max_sync_block_height "$BALANCE_HISTORY_ROOT/config.toml" "4294967295"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$snapshot_height"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$snapshot_height"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$snapshot_block_hash"
  regtest_assert_json_expr "$resp" "data['result']['latest_block_commit']" "$snapshot_commit"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "60000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$snapshot_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_old}:${vout_old}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_new}:${vout_new}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "20000000"

  txid_after_restore="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$target_address" 0.1)"
  regtest_mine_blocks 1 "$mining_address"
  post_restore_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$post_restore_height"
  vout_after_restore="$(regtest_get_tx_vout_for_address "$txid_after_restore" "$target_address")"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "70000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$post_restore_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_after_restore}:${vout_after_restore}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "10000000"

  regtest_log "Snapshot install retry test succeeded."
  regtest_log "Source logs: ${WORK_DIR}/balance-history-source.log"
  regtest_log "Target logs: ${TARGET_BALANCE_HISTORY_LOG_FILE}"
}

main "$@"