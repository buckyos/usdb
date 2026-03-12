#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-install-corrupt-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-source}"
TARGET_BALANCE_HISTORY_ROOT="${TARGET_BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-target}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30232}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30233}"
BH_RPC_PORT="${BH_RPC_PORT:-30210}"
TARGET_BH_RPC_PORT="${TARGET_BH_RPC_PORT:-30211}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotcorrupt}"
TARGET_MAX_SYNC_HEIGHT="${TARGET_MAX_SYNC_HEIGHT:-102}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-source.log}"
TARGET_BALANCE_HISTORY_LOG_FILE="${TARGET_BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-target.log}"
REGTEST_LOG_PREFIX="[snapshot-install-corrupt]"

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

regtest_assert_no_install_artifacts() {
  local root_dir="$1"
  local staging_count backup_count

  staging_count="$(find "$root_dir" -maxdepth 1 -type d -name 'snapshot_install_staging_*' | wc -l | tr -d ' ')"
  backup_count="$(find "$root_dir" -maxdepth 1 -type d -name 'db_backup_snapshot_install_*' | wc -l | tr -d ' ')"
  regtest_log "Install artifact assertion: root=${root_dir}, staging_count=${staging_count}, backup_count=${backup_count}"
  if [[ "$staging_count" != "0" || "$backup_count" != "0" ]]; then
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

  local source_root source_log source_rpc
  local target_root target_log target_rpc
  local mining_address target_address target_script_hash
  local txid_old txid_new vout_old vout_new
  local old_height snapshot_height old_block_hash snapshot_file corrupt_snapshot_file corrupt_snapshot_hash
  local corrupt_output resp

  source_root="$BALANCE_HISTORY_ROOT"
  source_log="$BALANCE_HISTORY_LOG_FILE"
  source_rpc="$BH_RPC_PORT"
  target_root="$TARGET_BALANCE_HISTORY_ROOT"
  target_log="$TARGET_BALANCE_HISTORY_LOG_FILE"
  target_rpc="$TARGET_BH_RPC_PORT"
  corrupt_output="$WORK_DIR/install_corrupt_snapshot.out"

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

  regtest_stop_balance_history
  regtest_run_balance_history_cli "$source_root" create-snapshot --block-height "$snapshot_height"

  snapshot_file="$source_root/snapshots/snapshot_${snapshot_height}.db"
  corrupt_snapshot_file="$source_root/snapshots/snapshot_${snapshot_height}_corrupt.db"
  cp "$snapshot_file" "$corrupt_snapshot_file"
  python3 - "$corrupt_snapshot_file" <<'PY'
from pathlib import Path
import sys

path = Path(sys.argv[1])
content = path.read_bytes()
path.write_bytes(b'not-a-sqlite-snapshot\n' + content[:64])
PY
  corrupt_snapshot_hash="$(sha256sum "$corrupt_snapshot_file" | awk '{print $1}')"
  old_block_hash="$(regtest_get_block_hash_by_height "$old_height")"

  BALANCE_HISTORY_ROOT="$target_root"
  BALANCE_HISTORY_LOG_FILE="$target_log"
  BH_RPC_PORT="$target_rpc"
  regtest_create_balance_history_config
  regtest_config_set_max_sync_block_height "$target_root/config.toml" "$TARGET_MAX_SYNC_HEIGHT"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$TARGET_MAX_SYNC_HEIGHT"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$TARGET_MAX_SYNC_HEIGHT"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$old_block_hash"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "40000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$old_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_old}:${vout_old}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_new}:${vout_new}\"]")"
  regtest_assert_json_expr "$resp" "data['result'] is None" "True"

  regtest_stop_balance_history

  regtest_expect_cli_failure "$target_root" "$corrupt_output" install-snapshot --file "$corrupt_snapshot_file" --hash "$corrupt_snapshot_hash"
  regtest_assert_no_install_artifacts "$target_root"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$TARGET_MAX_SYNC_HEIGHT"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$TARGET_MAX_SYNC_HEIGHT"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$old_block_hash"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${target_script_hash}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "40000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$old_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_old}:${vout_old}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_new}:${vout_new}\"]")"
  regtest_assert_json_expr "$resp" "data['result'] is None" "True"

  regtest_log "Snapshot install corrupt test succeeded."
  regtest_log "Source logs: ${source_log}"
  regtest_log "Target logs: ${target_log}"
}

main "$@"