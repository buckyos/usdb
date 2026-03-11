#!/usr/bin/env bash

if [[ -n "${USDB_BH_REGTEST_LIB_SH:-}" ]]; then
  return 0
fi
USDB_BH_REGTEST_LIB_SH=1

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)}"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-regtest-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28132}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28133}"
BH_RPC_PORT="${BH_RPC_PORT:-28110}"
WALLET_NAME="${WALLET_NAME:-bhitest}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
CURL_CONNECT_TIMEOUT_SEC="${CURL_CONNECT_TIMEOUT_SEC:-2}"
CURL_MAX_TIME_SEC="${CURL_MAX_TIME_SEC:-5}"
REGTEST_DIAG_TAIL_LINES="${REGTEST_DIAG_TAIL_LINES:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
COINBASE_MATURITY="${COINBASE_MATURITY:-100}"

BITCOIND_PID="${BITCOIND_PID:-}"
BALANCE_HISTORY_PID="${BALANCE_HISTORY_PID:-}"
BITCOIND_BIN="${BITCOIND_BIN:-}"
BITCOIN_CLI_BIN="${BITCOIN_CLI_BIN:-}"
REGTEST_DIAGNOSTICS_PRINTED="${REGTEST_DIAGNOSTICS_PRINTED:-0}"

regtest_log() {
  echo "${REGTEST_LOG_PREFIX:-[balance-history-regtest]} $*"
}

regtest_require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

regtest_resolve_bitcoin_binaries() {
  local candidate_bitcoind=""
  local candidate_bitcoin_cli=""

  if [[ -n "$BITCOIN_BIN_DIR" ]]; then
    candidate_bitcoind="${BITCOIN_BIN_DIR}/bitcoind"
    candidate_bitcoin_cli="${BITCOIN_BIN_DIR}/bitcoin-cli"
    if [[ -x "$candidate_bitcoind" ]] && [[ -x "$candidate_bitcoin_cli" ]]; then
      BITCOIND_BIN="$candidate_bitcoind"
      BITCOIN_CLI_BIN="$candidate_bitcoin_cli"
      regtest_log "Using Bitcoin Core binaries from BITCOIN_BIN_DIR=${BITCOIN_BIN_DIR}"
      return
    fi
  fi

  BITCOIND_BIN="$(command -v bitcoind || true)"
  BITCOIN_CLI_BIN="$(command -v bitcoin-cli || true)"
  if [[ -z "$BITCOIND_BIN" || -z "$BITCOIN_CLI_BIN" ]]; then
    echo "Missing required commands bitcoind/bitcoin-cli. Tried BITCOIN_BIN_DIR=${BITCOIN_BIN_DIR} and PATH." >&2
    exit 1
  fi

  regtest_log "Using Bitcoin Core binaries from PATH"
}

regtest_ensure_workspace_dirs() {
  mkdir -p "$WORK_DIR" "$BITCOIN_DIR" "$BALANCE_HISTORY_ROOT"
  regtest_log "Workspace directory: $WORK_DIR"
}

regtest_json_extract_python() {
  local script="$1"
  python3 -c "$script"
}

regtest_parse_json_number_result() {
  sed -n 's/.*"result"[[:space:]]*:[[:space:]]*\([0-9]\+\).*/\1/p' | head -n 1
}

regtest_parse_json_string_result() {
  sed -n 's/.*"result"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1
}

regtest_rpc_call_balance_history() {
  local method="$1"
  local params="${2:-[]}"
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

regtest_get_balance_history_height() {
  regtest_rpc_call_balance_history "get_block_height" "[]" | regtest_parse_json_number_result
}

regtest_get_block_commit_hash() {
  local block_height="$1"
  regtest_rpc_call_balance_history "get_block_commit" "[${block_height}]" \
    | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result"); print((r or {}).get("btc_block_hash", ""))'
}

regtest_get_snapshot_stable_hash() {
  regtest_rpc_call_balance_history "get_snapshot_info" "[]" \
    | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result") or {}; print(r.get("stable_block_hash", ""))'
}

regtest_get_utxo_value_sat() {
  local txid="$1"
  local vout="$2"
  local response
  response="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${txid}:${vout}\"]")"
  if [[ -z "$response" ]]; then
    echo ""
    return 0
  fi

  local error_message
  error_message="$(printf '%s' "$response" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); err=d.get("error") or {}; print(err.get("message", ""))')"
  if [[ -n "$error_message" ]]; then
    regtest_log "get_live_utxo RPC returned error for ${txid}:${vout}: ${response}"
    echo ""
    return 0
  fi

  printf '%s' "$response" \
    | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result"); print("" if r is None else r.get("value", ""))'
}

regtest_assert_utxo_value_sat() {
  local txid="$1"
  local vout="$2"
  local expected_sat="$3"
  local actual_sat

  actual_sat="$(regtest_get_utxo_value_sat "$txid" "$vout")"
  regtest_log "UTXO assertion: txid=${txid}, vout=${vout}, expected_sat=${expected_sat}, actual_sat=${actual_sat}"
  if [[ "$actual_sat" != "$expected_sat" ]]; then
    regtest_log "UTXO assertion failed for ${txid}:${vout}"
    exit 1
  fi
}

regtest_assert_utxo_missing() {
  local txid="$1"
  local vout="$2"
  local actual_sat

  actual_sat="$(regtest_get_utxo_value_sat "$txid" "$vout")"
  regtest_log "UTXO missing assertion: txid=${txid}, vout=${vout}, actual_sat=${actual_sat}"
  if [[ -n "$actual_sat" ]]; then
    regtest_log "Expected missing UTXO but found ${txid}:${vout} value=${actual_sat}"
    exit 1
  fi
}

regtest_create_balance_history_config() {
  mkdir -p "$BALANCE_HISTORY_ROOT"

  cat >"${BALANCE_HISTORY_ROOT}/config.toml" <<EOF
root_dir = "${BALANCE_HISTORY_ROOT}"

[btc]
network = "regtest"
data_dir = "${BITCOIN_DIR}/regtest"
rpc_url = "http://127.0.0.1:${BTC_RPC_PORT}"

[ordinals]
rpc_url = "http://127.0.0.1:"

[electrs]
rpc_url = "tcp://127.0.0.1:50001"

[sync]
local_loader_threshold = 100000000
batch_size = 32
max_sync_block_height = 4294967295

[rpc_server]
port = ${BH_RPC_PORT}
EOF
}

regtest_wait_balance_history_rpc_ready() {
  regtest_log "Waiting for balance-history RPC readiness"

  for _ in $(seq 1 120); do
    if regtest_rpc_call_balance_history "get_network_type" "[]" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done

  regtest_log "balance-history RPC not ready, see log: ${BALANCE_HISTORY_LOG_FILE}"
  exit 1
}

regtest_start_bitcoind() {
  regtest_log "Starting bitcoind regtest on rpcport=${BTC_RPC_PORT}, port=${BTC_P2P_PORT}, bin=${BITCOIND_BIN}"
  "$BITCOIND_BIN" \
    -regtest \
    -server=1 \
    -txindex=1 \
    -persistmempool=0 \
    -fallbackfee=0.0001 \
    -datadir="$BITCOIN_DIR" \
    -rpcport="$BTC_RPC_PORT" \
    -port="$BTC_P2P_PORT" \
    -daemonwait

  BITCOIND_PID="$(pgrep -f "bitcoind.*-datadir=${BITCOIN_DIR}" | head -n 1 || true)"
  if [[ -z "$BITCOIND_PID" ]]; then
    regtest_log "Failed to detect bitcoind PID"
    exit 1
  fi
  regtest_log "bitcoind pid=${BITCOIND_PID}"
}

regtest_ensure_wallet() {
  regtest_log "Creating/Loading wallet ${WALLET_NAME}"

  if ! "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    getwalletinfo >/dev/null 2>&1; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -named createwallet wallet_name="$WALLET_NAME" load_on_startup=true >/dev/null 2>&1 || true
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      loadwallet "$WALLET_NAME" >/dev/null 2>&1 || true
  fi

  if ! "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    getwalletinfo >/dev/null 2>&1; then
    regtest_log "Failed to create/load wallet: ${WALLET_NAME}"
    exit 1
  fi
}

regtest_get_new_address() {
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" getnewaddress
}

regtest_lock_wallet_outpoint() {
  local txid="$1"
  local vout="$2"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    lockunspent false "[{\"txid\":\"${txid}\",\"vout\":${vout}}]" >/dev/null
}

regtest_get_tx_vout_for_address() {
  local txid="$1"
  local address="$2"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getrawtransaction "$txid" true \
  | python3 -c 'import json, sys
address = sys.argv[1]
data = json.load(sys.stdin)
for output in data.get("vout", []):
  script = output.get("scriptPubKey", {})
  output_address = script.get("address")
  addresses = script.get("addresses") or []
  if output_address == address or address in addresses:
    print(output.get("n", ""))
    break' "$address"
}

regtest_mine_blocks() {
  local block_count="$1"
  local address="$2"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    generatetoaddress "$block_count" "$address" >/dev/null
}

regtest_mine_empty_block() {
  local address="$1"
  regtest_log "Mining empty replacement block to address=${address}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -named generateblock output="$address" transactions='[]' >/dev/null
}

regtest_ensure_mature_funds() {
  local address="$1"
  local required_height="$((COINBASE_MATURITY + 1))"
  local current_height blocks_to_mine

  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  if (( current_height >= required_height )); then
    regtest_log "Coinbase funds already mature at height=${current_height}"
    return 0
  fi

  blocks_to_mine=$((required_height - current_height))
  regtest_log "Mining ${blocks_to_mine} maturity blocks to reach spendable funds at height=${required_height}"
  regtest_mine_blocks "$blocks_to_mine" "$address"
}

regtest_start_balance_history() {
  regtest_log "Starting balance-history service (root=${BALANCE_HISTORY_ROOT}, rpc=${BH_RPC_PORT})"
  (
    cd "$REPO_ROOT"
    cargo run --manifest-path src/btc/Cargo.toml -p balance-history -- \
      --root-dir "$BALANCE_HISTORY_ROOT" \
      --skip-process-lock
  ) >"${BALANCE_HISTORY_LOG_FILE}" 2>&1 &
  BALANCE_HISTORY_PID=$!
}

# Restart the service and wait until its RPC endpoint becomes reachable again.
regtest_restart_balance_history() {
  regtest_stop_balance_history
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
}

regtest_wait_until_synced_height() {
  local target_height="$1"
  regtest_log "Waiting until synced block height >= ${target_height}"

  local start_ts now synced height_resp
  start_ts="$(date +%s)"
  while true; do
    height_resp="$(regtest_rpc_call_balance_history "get_block_height" "[]")"
    synced="$(echo "$height_resp" | regtest_parse_json_number_result)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      regtest_log "Sync reached height=${synced}"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Sync timeout, last response: ${height_resp}"
      regtest_log "See log file: ${BALANCE_HISTORY_LOG_FILE}"
      exit 1
    fi
    sleep 1
  done
}

regtest_wait_until_block_commit_hash() {
  local block_height="$1"
  local expected_hash="$2"
  regtest_log "Waiting until get_block_commit(${block_height}) reports hash=${expected_hash}"

  local start_ts now resp got_hash current_height
  start_ts="$(date +%s)"
  while true; do
    resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${block_height}]")"
    got_hash="$(echo "$resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result"); print((r or {}).get("btc_block_hash", ""))')"
    current_height="$(echo "$(regtest_rpc_call_balance_history "get_block_height" "[]")" | regtest_parse_json_number_result)"
    current_height="${current_height:-0}"
    if [[ "$got_hash" == "$expected_hash" ]] && [[ "$current_height" -ge "$block_height" ]]; then
      regtest_log "Service observed new canonical hash at height=${block_height}"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for new block commit hash. last_commit_resp=${resp}, current_height=${current_height}"
      regtest_log "See log file: ${BALANCE_HISTORY_LOG_FILE}"
      exit 1
    fi
    sleep 1
  done
}

regtest_btc_amount_to_sat() {
  local amount_btc="$1"
  python3 - "$amount_btc" <<'PY'
from decimal import Decimal, ROUND_DOWN
import sys

amount = Decimal(sys.argv[1])
sat = int((amount * Decimal("100000000")).to_integral_value(rounding=ROUND_DOWN))
print(sat)
PY
}

regtest_address_to_script_hash() {
  local address="$1"
  local script_pubkey
  script_pubkey="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    getaddressinfo "$address" | regtest_json_extract_python 'import json,sys; print(json.load(sys.stdin)["scriptPubKey"])')"

  python3 - "$script_pubkey" <<'PY'
import hashlib
import sys

script_hex = sys.argv[1]
script = bytes.fromhex(script_hex)
digest = hashlib.sha256(script).digest()[::-1].hex()
print(digest)
PY
}

regtest_get_address_balance_sat() {
  local address="$1"
  local block_height="$2"
  local script_hash
  script_hash="$(regtest_address_to_script_hash "$address")"
  regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash}\",\"block_height\":${block_height},\"block_range\":null}]" \
    | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result", []); print(r[0]["balance"] if r else 0)'
}

regtest_assert_address_balance_btc() {
  local address="$1"
  local block_height="$2"
  local amount_btc="$3"
  local expected_sat actual_sat

  expected_sat="$(regtest_btc_amount_to_sat "$amount_btc")"
  actual_sat="$(regtest_get_address_balance_sat "$address" "$block_height")"
  regtest_log "Balance assertion: address=${address}, height=${block_height}, expected_sat=${expected_sat}, actual_sat=${actual_sat}"
  if [[ "$actual_sat" != "$expected_sat" ]]; then
    regtest_log "Balance assertion failed for address=${address} at height=${block_height}"
    exit 1
  fi
}

regtest_print_tail_if_exists() {
  local label="$1"
  local file_path="$2"
  if [[ -f "$file_path" ]]; then
    regtest_log "---- ${label} (tail -n ${REGTEST_DIAG_TAIL_LINES}) ----"
    tail -n "$REGTEST_DIAG_TAIL_LINES" "$file_path" || true
    regtest_log "---- end ${label} ----"
  fi
}

regtest_print_failure_diagnostics() {
  local exit_code="$1"
  if [[ "$REGTEST_DIAGNOSTICS_PRINTED" == "1" ]]; then
    return 0
  fi
  REGTEST_DIAGNOSTICS_PRINTED=1

  regtest_log "Failure diagnostics: exit_code=${exit_code}, work_dir=${WORK_DIR}, btc_rpc_port=${BTC_RPC_PORT}, btc_p2p_port=${BTC_P2P_PORT}, bh_rpc_port=${BH_RPC_PORT}"
  regtest_print_tail_if_exists "balance-history stdout log" "$BALANCE_HISTORY_LOG_FILE"
  regtest_print_tail_if_exists "balance-history service log" "${BALANCE_HISTORY_ROOT}/logs/balance-history_rCURRENT.log"
  regtest_print_tail_if_exists "bitcoind debug log" "${BITCOIN_DIR}/regtest/debug.log"
}

regtest_stop_balance_history() {
  if [[ -n "$BALANCE_HISTORY_PID" ]] && kill -0 "$BALANCE_HISTORY_PID" 2>/dev/null; then
    regtest_log "Stopping balance-history process pid=$BALANCE_HISTORY_PID"
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1 || true

    for _ in $(seq 1 20); do
      if [[ "$(ps -o stat= -p "$BALANCE_HISTORY_PID" 2>/dev/null | tr -d ' ')" == Z* ]]; then
        wait "$BALANCE_HISTORY_PID" 2>/dev/null || true
        BALANCE_HISTORY_PID=""
        return 0
      fi

      if ! kill -0 "$BALANCE_HISTORY_PID" 2>/dev/null; then
        wait "$BALANCE_HISTORY_PID" 2>/dev/null || true
        BALANCE_HISTORY_PID=""
        return 0
      fi
      sleep 0.5
    done

    kill -9 "$BALANCE_HISTORY_PID" 2>/dev/null || true
    wait "$BALANCE_HISTORY_PID" 2>/dev/null || true
  fi

  BALANCE_HISTORY_PID=""
}

regtest_stop_bitcoind() {
  if [[ -n "$BITCOIN_CLI_BIN" ]] && [[ -x "$BITCOIN_CLI_BIN" ]]; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" stop >/dev/null 2>&1 || true
  fi

  if [[ -n "$BITCOIND_PID" ]] && kill -0 "$BITCOIND_PID" 2>/dev/null; then
    for _ in $(seq 1 20); do
      if ! kill -0 "$BITCOIND_PID" 2>/dev/null; then
        return 0
      fi
      sleep 0.5
    done

    kill -9 "$BITCOIND_PID" 2>/dev/null || true
  fi
}

regtest_cleanup() {
  local exit_code=$?
  set +e
  if [[ "$exit_code" -ne 0 ]]; then
    regtest_print_failure_diagnostics "$exit_code"
  fi
  regtest_stop_balance_history
  regtest_stop_bitcoind
}