#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-restart-recovery-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-source}"
RESTORE_BALANCE_HISTORY_ROOT="${RESTORE_BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history-restore}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29632}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29633}"
BH_RPC_PORT="${BH_RPC_PORT:-29610}"
RESTORE_BH_RPC_PORT="${RESTORE_BH_RPC_PORT:-29611}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotrestart}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-source.log}"
RESTORE_BALANCE_HISTORY_LOG_FILE="${RESTORE_BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history-restore.log}"
REGTEST_LOG_PREFIX="[snapshot-restart-recovery]"

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

regtest_assert_restored_state() {
  local snapshot_height="$1"
  local snapshot_block_hash="$2"
  local snapshot_commit="$3"
  local script_hash_a="$4"
  local script_hash_b="$5"
  local balance_a_sat="$6"
  local balance_b_sat="$7"
  local height_a="$8"
  local height_b="$9"
  local spent_outpoint="${10}"
  local live_outpoint_a="${11}"
  local live_outpoint_b="${12}"
  local resp

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$snapshot_height"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$snapshot_block_hash"
  regtest_assert_json_expr "$resp" "data['result']['latest_block_commit']" "$snapshot_commit"

  resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${snapshot_height}]")"
  regtest_assert_json_expr "$resp" "data['result']['btc_block_hash']" "$snapshot_block_hash"
  regtest_assert_json_expr "$resp" "data['result']['block_commit']" "$snapshot_commit"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$balance_a_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_a"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_b}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$balance_b_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_b"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${spent_outpoint}\"]")"
  regtest_assert_json_expr "$resp" "data['result'] is None" "True"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${live_outpoint_a}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "25000000"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${live_outpoint_b}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "50000000"
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_require_cmd sha256sum

  regtest_ensure_workspace_dirs
  mkdir -p "$RESTORE_BALANCE_HISTORY_ROOT"

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address address_a address_b untracked_address
  local script_hash_a script_hash_b
  local txid_a1 txid_a2 txid_b1 txid_b2 spend_raw spend_signed spend_txid
  local vout_a1 vout_a2 vout_b1 vout_b2
  local height_1 height_2 height_3 height_4 height_5
  local snapshot_height snapshot_file snapshot_hash snapshot_block_hash snapshot_commit
  local expected_a_latest_sat expected_b_latest_sat expected_b_post_restart_sat
  local spent_outpoint live_outpoint_a live_outpoint_b live_outpoint_b2 resp

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  address_a="$(regtest_get_new_address)"
  address_b="$(regtest_get_new_address)"
  untracked_address="$(regtest_get_new_address)"
  script_hash_a="$(regtest_address_to_script_hash "$address_a")"
  script_hash_b="$(regtest_address_to_script_hash "$address_b")"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$COINBASE_MATURITY"

  txid_a1="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_a" 1.0)"
  regtest_mine_blocks 1 "$mining_address"
  height_1="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_1"
  vout_a1="$(regtest_get_tx_vout_for_address "$txid_a1" "$address_a")"
  regtest_lock_wallet_outpoint "$txid_a1" "$vout_a1"

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    lockunspent true "[{\"txid\":\"${txid_a1}\",\"vout\":${vout_a1}}]" >/dev/null
  spend_raw="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    createrawtransaction "[{\"txid\":\"${txid_a1}\",\"vout\":${vout_a1}}]" "{\"${untracked_address}\":0.9999}")"
  spend_signed="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    signrawtransactionwithwallet "$spend_raw" | regtest_json_extract_python 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  spend_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" sendrawtransaction "$spend_signed")"
  regtest_log "Spent tracked output via raw transaction txid=${spend_txid}"
  regtest_mine_blocks 1 "$mining_address"
  height_2="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_2"

  txid_a2="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_a" 0.25)"
  regtest_mine_blocks 1 "$mining_address"
  height_3="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_3"
  vout_a2="$(regtest_get_tx_vout_for_address "$txid_a2" "$address_a")"
  regtest_lock_wallet_outpoint "$txid_a2" "$vout_a2"

  txid_b1="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_b" 0.5)"
  regtest_mine_blocks 1 "$mining_address"
  height_4="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_4"
  vout_b1="$(regtest_get_tx_vout_for_address "$txid_b1" "$address_b")"
  regtest_lock_wallet_outpoint "$txid_b1" "$vout_b1"

  snapshot_height="$height_4"
  snapshot_block_hash="$(regtest_get_block_hash_by_height "$snapshot_height")"
  expected_a_latest_sat="$(regtest_btc_amount_to_sat 0.25)"
  expected_b_latest_sat="$(regtest_btc_amount_to_sat 0.5)"
  expected_b_post_restart_sat="$(regtest_btc_amount_to_sat 0.6)"
  spent_outpoint="${txid_a1}:${vout_a1}"
  live_outpoint_a="${txid_a2}:${vout_a2}"
  live_outpoint_b="${txid_b1}:${vout_b1}"

  resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${snapshot_height}]")"
  snapshot_commit="$(printf '%s' "$resp" | python3 -c "import json,sys; data=json.load(sys.stdin); print(data['result']['block_commit'])")"

  regtest_stop_balance_history
  regtest_run_balance_history_cli "$BALANCE_HISTORY_ROOT" create-snapshot --block-height "$snapshot_height"

  snapshot_file="$BALANCE_HISTORY_ROOT/snapshots/snapshot_${snapshot_height}.db"
  snapshot_hash="$(sha256sum "$snapshot_file" | awk '{print $1}')"

  BALANCE_HISTORY_ROOT="$RESTORE_BALANCE_HISTORY_ROOT"
  BALANCE_HISTORY_LOG_FILE="$RESTORE_BALANCE_HISTORY_LOG_FILE"
  BH_RPC_PORT="$RESTORE_BH_RPC_PORT"

  regtest_create_balance_history_config
  regtest_run_balance_history_cli "$BALANCE_HISTORY_ROOT" install-snapshot --file "$snapshot_file" --hash "$snapshot_hash"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$snapshot_height"
  regtest_assert_restored_state \
    "$snapshot_height" "$snapshot_block_hash" "$snapshot_commit" \
    "$script_hash_a" "$script_hash_b" \
    "$expected_a_latest_sat" "$expected_b_latest_sat" \
    "$height_3" "$height_4" \
    "$spent_outpoint" "$live_outpoint_a" "$live_outpoint_b"

  regtest_restart_balance_history
  regtest_wait_until_synced_height "$snapshot_height"
  regtest_assert_restored_state \
    "$snapshot_height" "$snapshot_block_hash" "$snapshot_commit" \
    "$script_hash_a" "$script_hash_b" \
    "$expected_a_latest_sat" "$expected_b_latest_sat" \
    "$height_3" "$height_4" \
    "$spent_outpoint" "$live_outpoint_a" "$live_outpoint_b"

  txid_b2="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_b" 0.1)"
  regtest_mine_blocks 1 "$mining_address"
  height_5="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_5"
  vout_b2="$(regtest_get_tx_vout_for_address "$txid_b2" "$address_b")"
  live_outpoint_b2="${txid_b2}:${vout_b2}"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_b}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$expected_b_post_restart_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_5"

  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$height_5"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$(regtest_get_block_hash_by_height "$height_5")"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${live_outpoint_b2}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "10000000"

  regtest_restart_balance_history
  regtest_wait_until_synced_height "$height_5"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_b}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$expected_b_post_restart_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_5"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${live_outpoint_b2}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "10000000"

  regtest_log "Snapshot restart recovery test succeeded."
  regtest_log "Source logs: ${WORK_DIR}/balance-history-source.log"
  regtest_log "Restore logs: ${RESTORE_BALANCE_HISTORY_LOG_FILE}"
}

main "$@"