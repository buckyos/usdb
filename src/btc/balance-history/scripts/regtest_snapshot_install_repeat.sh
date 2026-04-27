#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-install-repeat-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-source}"
TARGET_BALANCE_HISTORY_ROOT="${TARGET_BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-target}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30132}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30133}"
BH_RPC_PORT="${BH_RPC_PORT:-30110}"
TARGET_BH_RPC_PORT="${TARGET_BH_RPC_PORT:-30111}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotrepeat}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-source.log}"
TARGET_BALANCE_HISTORY_LOG_FILE="${TARGET_BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-target.log}"
REGTEST_LOG_PREFIX="[snapshot-install-repeat]"

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

regtest_assert_installed_state() {
  local stable_height="$1"
  local stable_block_hash="$2"
  local snapshot_commit="$3"
  local script_hash="$4"
  local balance_sat="$5"
  local balance_height="$6"
  local outpoint_a="$7"
  local outpoint_b="$8"
  local resp

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$stable_height"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$stable_block_hash"
  regtest_assert_json_expr "$resp" "data['result']['latest_block_commit']" "$snapshot_commit"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$balance_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$balance_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${outpoint_a}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${outpoint_b}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "20000000"
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_ensure_workspace_dirs
  mkdir -p "$TARGET_BALANCE_HISTORY_ROOT"

  local source_root source_log source_rpc
  local target_root target_log target_rpc
  local mining_address target_address target_script_hash
  local txid_a txid_b txid_c
  local vout_a vout_b vout_c
  local snapshot_height post_snapshot_height
  local snapshot_file snapshot_block_hash snapshot_commit
  local outpoint_a outpoint_b outpoint_c resp

  source_root="$BALANCE_HISTORY_ROOT"
  source_log="$BALANCE_HISTORY_LOG_FILE"
  source_rpc="$BH_RPC_PORT"
  target_root="$TARGET_BALANCE_HISTORY_ROOT"
  target_log="$TARGET_BALANCE_HISTORY_LOG_FILE"
  target_rpc="$TARGET_BH_RPC_PORT"

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

  txid_a="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$target_address" 0.4)"
  regtest_mine_blocks 1 "$mining_address"
  regtest_wait_until_synced_height "$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  vout_a="$(regtest_get_tx_vout_for_address "$txid_a" "$target_address")"
  regtest_lock_wallet_outpoint "$txid_a" "$vout_a"

  txid_b="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$target_address" 0.2)"
  regtest_mine_blocks 1 "$mining_address"
  snapshot_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$snapshot_height"
  vout_b="$(regtest_get_tx_vout_for_address "$txid_b" "$target_address")"
  regtest_lock_wallet_outpoint "$txid_b" "$vout_b"

  snapshot_block_hash="$(regtest_get_block_hash_by_height "$snapshot_height")"
  resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${snapshot_height}]")"
  snapshot_commit="$(printf '%s' "$resp" | python3 -c "import json,sys; data=json.load(sys.stdin); print(data['result']['block_commit'])")"

  regtest_stop_balance_history
  regtest_run_balance_history_cli "$source_root" create-snapshot --block-height "$snapshot_height"

  snapshot_file="$source_root/snapshots/snapshot_${snapshot_height}.db"
  outpoint_a="${txid_a}:${vout_a}"
  outpoint_b="${txid_b}:${vout_b}"

  BALANCE_HISTORY_ROOT="$target_root"
  BALANCE_HISTORY_LOG_FILE="$target_log"
  BH_RPC_PORT="$target_rpc"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$snapshot_height"
  regtest_stop_balance_history

  regtest_run_balance_history_cli "$target_root" install-snapshot --file "$snapshot_file"
  regtest_assert_artifact_counts "$target_root" "0" "1"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$snapshot_height"
  regtest_assert_installed_state \
    "$snapshot_height" "$snapshot_block_hash" "$snapshot_commit" \
    "$target_script_hash" "60000000" "$snapshot_height" \
    "$outpoint_a" "$outpoint_b"

  regtest_stop_balance_history

  regtest_run_balance_history_cli "$target_root" install-snapshot --file "$snapshot_file"
  regtest_assert_artifact_counts "$target_root" "0" "2"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$snapshot_height"
  regtest_assert_installed_state \
    "$snapshot_height" "$snapshot_block_hash" "$snapshot_commit" \
    "$target_script_hash" "60000000" "$snapshot_height" \
    "$outpoint_a" "$outpoint_b"

  txid_c="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$target_address" 0.1)"
  regtest_mine_blocks 1 "$mining_address"
  post_snapshot_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$post_snapshot_height"
  vout_c="$(regtest_get_tx_vout_for_address "$txid_c" "$target_address")"
  outpoint_c="${txid_c}:${vout_c}"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "70000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$post_snapshot_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${outpoint_c}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "10000000"

  regtest_log "Snapshot install repeat test succeeded."
  regtest_log "Source logs: ${source_log}"
  regtest_log "Target logs: ${target_log}"
}

main "$@"
