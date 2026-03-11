#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-regtest-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28132}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28133}"
BH_RPC_PORT="${BH_RPC_PORT:-28110}"
WALLET_NAME="${WALLET_NAME:-bhitest}"
TARGET_HEIGHT="${TARGET_HEIGHT:-120}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
ENABLE_TRANSFER_CHECK="${ENABLE_TRANSFER_CHECK:-1}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[regtest-smoke]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  regtest_ensure_workspace_dirs

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address
  mining_address="$(regtest_get_new_address)"
  regtest_log "Mining ${TARGET_HEIGHT} blocks to address=${mining_address}"
  regtest_mine_blocks "$TARGET_HEIGHT" "$mining_address"

  regtest_create_balance_history_config
  regtest_log "Generated balance-history config at ${BALANCE_HISTORY_ROOT}/config.toml"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready

  local network_resp network
  network_resp="$(regtest_rpc_call_balance_history "get_network_type" "[]")"
  network="$(echo "$network_resp" | regtest_parse_json_string_result)"
  regtest_log "RPC get_network_type => ${network}"
  if [[ "$network" != "regtest" ]]; then
    regtest_log "Unexpected network type: ${network_resp}"
    exit 1
  fi

  regtest_wait_until_synced_height "$TARGET_HEIGHT"

  if [[ "$ENABLE_TRANSFER_CHECK" == "1" ]]; then
    local receiver_address txid expected_height script_hash balance_resp got_balance expected_sat

    receiver_address="$(regtest_get_new_address)"
    regtest_log "Sending ${SEND_AMOUNT_BTC} BTC to receiver address=${receiver_address}"
    txid="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
    regtest_log "Created txid=${txid}"

    regtest_log "Mining 1 block to confirm transfer"
    regtest_mine_blocks 1 "$mining_address"

    expected_height=$((TARGET_HEIGHT + 1))
    regtest_wait_until_synced_height "$expected_height"

    script_hash="$(regtest_address_to_script_hash "$receiver_address")"
    balance_resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash}\",\"block_height\":${expected_height},\"block_range\":null}]")"
    got_balance="$(echo "$balance_resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result",[]); print(r[0]["balance"] if r else 0)')"
    expected_sat="$(regtest_btc_amount_to_sat "$SEND_AMOUNT_BTC")"

    regtest_log "Transfer balance check: height=${expected_height}, script_hash=${script_hash}, expected=${expected_sat}, got=${got_balance}"
    if [[ "$got_balance" != "$expected_sat" ]]; then
      regtest_log "Transfer balance mismatch, response: ${balance_resp}"
      exit 1
    fi
  fi

  regtest_log "Smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"
