#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-rpc-semantics-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29032}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29033}"
BH_RPC_PORT="${BH_RPC_PORT:-29010}"
WALLET_NAME="${WALLET_NAME:-bhrpcsemantics}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[rpc-semantics]"

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

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address address_a address_b address_c untracked_address
  local script_hash_a script_hash_b script_hash_c
  local txid_a1 txid_b1 txid_a2 txid_c1 spend_raw spend_signed spend_txid
  local vout_a1 vout_b1 vout_a2 vout_c1
  local height_1 height_2 height_3 height_4 height_5 latest_height
  local resp expected_a_h3_sat expected_b_h3_sat expected_c_h5_sat future_height

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  address_a="$(regtest_get_new_address)"
  address_b="$(regtest_get_new_address)"
  address_c="$(regtest_get_new_address)"
  untracked_address="$(regtest_get_new_address)"
  script_hash_a="$(regtest_address_to_script_hash "$address_a")"
  script_hash_b="$(regtest_address_to_script_hash "$address_b")"
  script_hash_c="$(regtest_address_to_script_hash "$address_c")"

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

  txid_b1="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_b" 0.5)"
  regtest_mine_blocks 1 "$mining_address"
  height_2="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_2"
  vout_b1="$(regtest_get_tx_vout_for_address "$txid_b1" "$address_b")"
  regtest_lock_wallet_outpoint "$txid_b1" "$vout_b1"

  txid_a2="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_a" 0.25)"
  regtest_mine_blocks 1 "$mining_address"
  height_3="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_3"
  vout_a2="$(regtest_get_tx_vout_for_address "$txid_a2" "$address_a")"
  regtest_lock_wallet_outpoint "$txid_a2" "$vout_a2"

  regtest_mine_empty_block "$mining_address"
  height_4="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_4"

  txid_c1="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_c" 0.1)"
  regtest_mine_blocks 1 "$mining_address"
  height_5="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_5"
  vout_c1="$(regtest_get_tx_vout_for_address "$txid_c1" "$address_c")"
  regtest_lock_wallet_outpoint "$txid_c1" "$vout_c1"

  expected_a_h3_sat="$(regtest_btc_amount_to_sat 1.25)"
  expected_b_h3_sat="$(regtest_btc_amount_to_sat 0.5)"
  expected_c_h5_sat="$(regtest_btc_amount_to_sat 0.1)"
  future_height=$((height_5 + 10))
  latest_height="$height_5"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$expected_a_h3_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_3"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":${height_4},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$expected_a_h3_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_3"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":${future_height},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$expected_a_h3_sat"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_3"

  resp="$(regtest_rpc_call_balance_history "get_address_balance_delta" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":${height_3},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['delta']" "25000000"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$expected_a_h3_sat"

  resp="$(regtest_rpc_call_balance_history "get_address_balance_delta" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":${height_4},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0] is None" "True"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":{\"start\":${height_1},\"end\":${height_4}}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "2"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_1"
  regtest_assert_json_expr "$resp" "data['result'][1]['block_height']" "$height_3"

  resp="$(regtest_rpc_call_balance_history "get_address_balance_delta" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":{\"start\":${height_3},\"end\":${height_5}}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "1"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_3"
  regtest_assert_json_expr "$resp" "data['result'][0]['delta']" "25000000"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":{\"start\":${height_4},\"end\":${height_4}}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "0"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances" "[{\"script_hashes\":[\"${script_hash_b}\",\"${script_hash_a}\",\"${script_hash_b}\"],\"block_height\":${height_3},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "3"
  regtest_assert_json_expr "$resp" "data['result'][0][0]['balance']" "$expected_b_h3_sat"
  regtest_assert_json_expr "$resp" "data['result'][1][0]['balance']" "$expected_a_h3_sat"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['balance']" "$expected_b_h3_sat"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_a1}:${vout_a1}\"]")"
  regtest_assert_json_expr "$resp" "data['result']['value']" "100000000"

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    lockunspent true "[{\"txid\":\"${txid_a1}\",\"vout\":${vout_a1}}]" >/dev/null
  spend_raw="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    createrawtransaction "[{\"txid\":\"${txid_a1}\",\"vout\":${vout_a1}}]" "{\"${untracked_address}\":0.9999}")"
  spend_signed="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    signrawtransactionwithwallet "$spend_raw" | regtest_json_extract_python 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  spend_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" sendrawtransaction "$spend_signed")"
  regtest_log "Spent tracked outpoint via raw transaction txid=${spend_txid}"
  regtest_mine_blocks 1 "$mining_address"
  latest_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$latest_height"

  resp="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid_a1}:${vout_a1}\"]")"
  regtest_assert_json_expr "$resp" "data['result'] is None" "True"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_c}\",\"block_height\":${height_5},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "$expected_c_h5_sat"

  regtest_log "RPC semantics test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"