#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-install-downgrade-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30032}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30033}"
BH_RPC_PORT="${BH_RPC_PORT:-30010}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotdowngrade}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[snapshot-install-downgrade]"

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

regtest_assert_backup_count() {
  local root_dir="$1"
  local expected_backup_count="$2"
  local staging_count backup_count

  staging_count="$(find "$root_dir" -maxdepth 1 -type d -name 'snapshot_install_staging_*' | wc -l | tr -d ' ')"
  backup_count="$(find "$root_dir" -maxdepth 1 -type d -name 'db_backup_snapshot_install_*' | wc -l | tr -d ' ')"
  regtest_log "Install artifact assertion: root=${root_dir}, staging_count=${staging_count}, backup_count=${backup_count}"
  if [[ "$staging_count" != "0" || "$backup_count" != "$expected_backup_count" ]]; then
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
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address target_address target_script_hash
  local txid_old txid_new vout_old vout_new
  local old_height new_height
  local old_block_hash new_block_hash old_snapshot_file old_snapshot_hash
  local old_commit new_commit resp

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
  old_block_hash="$(regtest_get_block_hash_by_height "$old_height")"

  resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${old_height}]")"
  old_commit="$(printf '%s' "$resp" | python3 -c "import json,sys; data=json.load(sys.stdin); print(data['result']['block_commit'])")"

  regtest_stop_balance_history
  regtest_run_balance_history_cli "$BALANCE_HISTORY_ROOT" create-snapshot --block-height "$old_height"
  old_snapshot_file="$BALANCE_HISTORY_ROOT/snapshots/snapshot_${old_height}.db"
  old_snapshot_hash="$(sha256sum "$old_snapshot_file" | awk '{print $1}')"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$old_height"

  txid_new="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$target_address" 0.2)"
  regtest_mine_blocks 1 "$mining_address"
  new_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$new_height"
  vout_new="$(regtest_get_tx_vout_for_address "$txid_new" "$target_address")"
  regtest_lock_wallet_outpoint "$txid_new" "$vout_new"
  new_block_hash="$(regtest_get_block_hash_by_height "$new_height")"

  resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${new_height}]")"
  new_commit="$(printf '%s' "$resp" | python3 -c "import json,sys; data=json.load(sys.stdin); print(data['result']['block_commit'])")"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$new_height"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$new_block_hash"
  regtest_assert_json_expr "$resp" "data['result']['latest_block_commit']" "$new_commit"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "60000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$new_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_old}:${vout_old}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_new}:${vout_new}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "20000000"

  regtest_stop_balance_history
  regtest_config_set_max_sync_block_height "$BALANCE_HISTORY_ROOT/config.toml" "$old_height"
  regtest_run_balance_history_cli "$BALANCE_HISTORY_ROOT" install-snapshot --file "$old_snapshot_file" --hash "$old_snapshot_hash"
  regtest_assert_backup_count "$BALANCE_HISTORY_ROOT" "1"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$old_height"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$old_height"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$old_block_hash"
  regtest_assert_json_expr "$resp" "data['result']['latest_block_commit']" "$old_commit"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "40000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$old_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_old}:${vout_old}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_new}:${vout_new}\"]")"
  regtest_assert_json_expr "$resp" "data['result'] is None" "True"

  regtest_stop_balance_history
  regtest_config_set_max_sync_block_height "$BALANCE_HISTORY_ROOT/config.toml" "4294967295"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$new_height"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$new_height"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$new_block_hash"
  regtest_assert_json_expr "$resp" "data['result']['latest_block_commit']" "$new_commit"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "60000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$new_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_old}:${vout_old}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_new}:${vout_new}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "20000000"

  regtest_log "Snapshot install downgrade test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"