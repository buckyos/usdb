#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-spend-graph-queries-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30532}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30533}"
BH_RPC_PORT="${BH_RPC_PORT:-30510}"
WALLET_NAME="${WALLET_NAME:-bhspendgraph}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[spend-graph-queries]"

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

regtest_spend_outpoint() {
  local txid="$1"
  local vout="$2"
  local outputs_json="$3"
  local raw_tx signed_tx spend_txid

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    lockunspent true "[{\"txid\":\"${txid}\",\"vout\":${vout}}]" >/dev/null
  raw_tx="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    createrawtransaction "[{\"txid\":\"${txid}\",\"vout\":${vout}}]" "$outputs_json")"
  signed_tx="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    signrawtransactionwithwallet "$raw_tx" | regtest_json_extract_python 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  spend_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" sendrawtransaction "$signed_tx")"
  regtest_log "Created spend transaction txid=${spend_txid}, input=${txid}:${vout}, outputs=${outputs_json}"
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  local mining_address address_a address_b address_c address_untracked
  local script_hash_a script_hash_b script_hash_c
  local fund_txid vout_a1 vout_b1 outputs_json
  local height_1 height_2 height_3 resp

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  address_a="$(regtest_get_new_address)"
  address_b="$(regtest_get_new_address)"
  address_c="$(regtest_get_new_address)"
  address_untracked="$(regtest_get_new_address)"
  script_hash_a="$(regtest_address_to_script_hash "$address_a")"
  script_hash_b="$(regtest_address_to_script_hash "$address_b")"
  script_hash_c="$(regtest_address_to_script_hash "$address_c")"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$COINBASE_MATURITY"

  fund_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    sendmany "" "$(regtest_build_outputs_json "$address_a" "1.0" "$address_b" "0.6")")"
  regtest_log "Created initial fanout transaction txid=${fund_txid}"
  regtest_mine_blocks 1 "$mining_address"
  height_1="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_1"
  vout_a1="$(regtest_get_tx_vout_for_address "$fund_txid" "$address_a")"
  vout_b1="$(regtest_get_tx_vout_for_address "$fund_txid" "$address_b")"
  regtest_lock_wallet_outpoint "$fund_txid" "$vout_a1"
  regtest_lock_wallet_outpoint "$fund_txid" "$vout_b1"

  outputs_json="$(regtest_build_outputs_json "$address_c" "0.40000000" "$address_untracked" "0.59999000")"
  regtest_spend_outpoint "$fund_txid" "$vout_a1" "$outputs_json"
  regtest_mine_blocks 1 "$mining_address"
  height_2="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_2"

  outputs_json="$(regtest_build_outputs_json "$address_a" "0.20000000" "$address_c" "0.39998000")"
  regtest_spend_outpoint "$fund_txid" "$vout_b1" "$outputs_json"
  regtest_mine_blocks 1 "$mining_address"
  height_3="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$height_3"

  regtest_assert_address_balance_sat "$address_a" "$height_1" "100000000"
  regtest_assert_address_balance_sat "$address_b" "$height_1" "60000000"
  regtest_assert_address_balance_sat "$address_c" "$height_1" "0"

  regtest_assert_address_balance_sat "$address_a" "$height_2" "0"
  regtest_assert_address_balance_sat "$address_b" "$height_2" "60000000"
  regtest_assert_address_balance_sat "$address_c" "$height_2" "40000000"

  regtest_assert_address_balance_sat "$address_a" "$height_3" "20000000"
  regtest_assert_address_balance_sat "$address_b" "$height_3" "0"
  regtest_assert_address_balance_sat "$address_c" "$height_3" "79998000"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances" "[{\"script_hashes\":[\"${script_hash_c}\",\"${script_hash_a}\",\"${script_hash_b}\",\"${script_hash_a}\"],\"block_height\":${height_3},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "4"
  regtest_assert_json_expr "$resp" "data['result'][0][0]['balance']" "79998000"
  regtest_assert_json_expr "$resp" "data['result'][1][0]['balance']" "20000000"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['balance']" "0"
  regtest_assert_json_expr "$resp" "data['result'][3][0]['balance']" "20000000"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances_delta" "[{\"script_hashes\":[\"${script_hash_a}\",\"${script_hash_b}\",\"${script_hash_c}\"],\"block_height\":${height_2},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0][0]['delta']" "-100000000"
  regtest_assert_json_expr "$resp" "data['result'][0][0]['balance']" "0"
  regtest_assert_json_expr "$resp" "data['result'][1][0] is None" "True"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['delta']" "40000000"
  regtest_assert_json_expr "$resp" "data['result'][2][0]['balance']" "40000000"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":{\"start\":${height_1},\"end\":$((height_3 + 1))}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "3"
  regtest_assert_json_expr "$resp" "data['result'][0]['delta']" "100000000"
  regtest_assert_json_expr "$resp" "data['result'][1]['delta']" "-100000000"
  regtest_assert_json_expr "$resp" "data['result'][2]['delta']" "20000000"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances_delta" "[{\"script_hashes\":[\"${script_hash_a}\",\"${script_hash_b}\",\"${script_hash_c}\"],\"block_height\":null,\"block_range\":{\"start\":${height_1},\"end\":$((height_3 + 1))}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'][0])" "3"
  regtest_assert_json_expr "$resp" "len(data['result'][1])" "2"
  regtest_assert_json_expr "$resp" "len(data['result'][2])" "2"
  regtest_assert_json_expr "$resp" "data['result'][1][1]['delta']" "-60000000"
  regtest_assert_json_expr "$resp" "data['result'][2][1]['balance']" "79998000"

  regtest_log "Spend graph query test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"