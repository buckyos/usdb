#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-multi-input-same-block-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30632}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30633}"
BH_RPC_PORT="${BH_RPC_PORT:-30610}"
WALLET_NAME="${WALLET_NAME:-bhmultiinput}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[multi-input-same-block]"

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

regtest_build_outputs_json() {
  python3 - "$@" <<'PY'
import json
import sys

args = sys.argv[1:]
if len(args) % 2 != 0:
    raise SystemExit("expected alternating address/amount pairs")

outputs = {}
for idx in range(0, len(args), 2):
    outputs[args[idx]] = args[idx + 1]

print(json.dumps(outputs))
PY
}

regtest_spend_multi_input() {
  local inputs_json="$1"
  local outputs_json="$2"
  local raw_tx signed_tx spend_txid

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    lockunspent true "$inputs_json" >/dev/null
  raw_tx="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    createrawtransaction "$inputs_json" "$outputs_json")"
  signed_tx="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    signrawtransactionwithwallet "$raw_tx" | regtest_json_extract_python 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  spend_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" sendrawtransaction "$signed_tx")"
  regtest_log "Created multi-input spend transaction txid=${spend_txid}, inputs=${inputs_json}, outputs=${outputs_json}"
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  local mining_address address_a address_b address_c
  local script_hash_a script_hash_b script_hash_c
  local fund_txid vout_a vout_b inputs_json outputs_json
  local bonus_txid height_1 height_2 future_height resp

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  address_a="$(regtest_get_new_address)"
  address_b="$(regtest_get_new_address)"
  address_c="$(regtest_get_new_address)"
  script_hash_a="$(regtest_address_to_script_hash "$address_a")"
  script_hash_b="$(regtest_address_to_script_hash "$address_b")"
  script_hash_c="$(regtest_address_to_script_hash "$address_c")"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$COINBASE_MATURITY"

  fund_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    sendmany "" "$(regtest_build_outputs_json "$address_a" "1.0" "$address_b" "0.6")")"
  regtest_log "Created initial dual-funding transaction txid=${fund_txid}"
  regtest_mine_blocks 1 "$mining_address"
  height_1="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_1"
  vout_a="$(regtest_get_tx_vout_for_address "$fund_txid" "$address_a")"
  vout_b="$(regtest_get_tx_vout_for_address "$fund_txid" "$address_b")"
  regtest_lock_wallet_outpoint "$fund_txid" "$vout_a"
  regtest_lock_wallet_outpoint "$fund_txid" "$vout_b"

  inputs_json="[{\"txid\":\"${fund_txid}\",\"vout\":${vout_a}},{\"txid\":\"${fund_txid}\",\"vout\":${vout_b}}]"
  outputs_json="$(regtest_build_outputs_json "$address_c" "1.10000000" "$address_a" "0.49998000")"
  regtest_spend_multi_input "$inputs_json" "$outputs_json"
  bonus_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_a" 0.05)"
  regtest_log "Created same-block bonus payment to address_a txid=${bonus_txid}"
  regtest_mine_blocks 1 "$mining_address"
  height_2="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_2"
  future_height=$((height_2 + 10))

  regtest_assert_address_balance_sat "$address_a" "$height_1" "100000000"
  regtest_assert_address_balance_sat "$address_b" "$height_1" "60000000"
  regtest_assert_address_balance_sat "$address_c" "$height_1" "0"

  regtest_assert_address_balance_sat "$address_a" "$height_2" "54998000"
  regtest_assert_address_balance_sat "$address_b" "$height_2" "0"
  regtest_assert_address_balance_sat "$address_c" "$height_2" "110000000"

  resp="$(regtest_rpc_call_balance_history "get_address_balance_delta" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":${height_2},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "1"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$height_2"
  regtest_assert_json_expr "$resp" "data['result'][0]['delta']" "-45002000"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "54998000"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":{\"start\":${height_1},\"end\":$((height_2 + 1))}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "2"
  regtest_assert_json_expr "$resp" "data['result'][0]['delta']" "100000000"
  regtest_assert_json_expr "$resp" "data['result'][1]['delta']" "-45002000"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances_delta" "[{\"script_hashes\":[\"${script_hash_a}\",\"${script_hash_b}\",\"${script_hash_c}\",\"${script_hash_a}\"],\"block_height\":${height_2},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "4"
  regtest_assert_json_expr "$resp" "data['result'][0][0]['delta']" "-45002000"
  regtest_assert_json_expr "$resp" "data['result'][1][0]['delta']" "-60000000"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['delta']" "110000000"
  regtest_assert_json_expr "$resp" "data['result'][3][0]['delta']" "-45002000"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances" "[{\"script_hashes\":[\"${script_hash_c}\",\"${script_hash_b}\",\"${script_hash_a}\"],\"block_height\":${future_height},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0][0]['balance']" "110000000"
  regtest_assert_json_expr "$resp" "data['result'][1][0]['balance']" "0"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['balance']" "54998000"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['block_height']" "$height_2"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances_delta" "[{\"script_hashes\":[\"${script_hash_a}\",\"${script_hash_b}\",\"${script_hash_c}\"],\"block_height\":null,\"block_range\":{\"start\":${height_1},\"end\":$((height_2 + 1))}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'][0])" "2"
  regtest_assert_json_expr "$resp" "len(data['result'][1])" "2"
  regtest_assert_json_expr "$resp" "len(data['result'][2])" "1"
  regtest_assert_json_expr "$resp" "data['result'][0][1]['balance']" "54998000"
  regtest_assert_json_expr "$resp" "data['result'][1][1]['balance']" "0"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['block_height']" "$height_2"

  regtest_log "Multi-input same-block query test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"