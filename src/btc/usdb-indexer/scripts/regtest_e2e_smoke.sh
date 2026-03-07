#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-regtest-XXXXXX)}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"

BTC_RPC_PORT="${BTC_RPC_PORT:-19453}"
BH_RPC_PORT="${BH_RPC_PORT:-18090}"
USDB_RPC_PORT="${USDB_RPC_PORT:-18110}"

WALLET_NAME="${WALLET_NAME:-usdbitest}"
TARGET_HEIGHT="${TARGET_HEIGHT:-120}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
ENABLE_TRANSFER_CHECK="${ENABLE_TRANSFER_CHECK:-1}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.0}"
MIN_SPENDABLE_BLOCK_HEIGHT="${MIN_SPENDABLE_BLOCK_HEIGHT:-101}"
CURL_CONNECT_TIMEOUT_SEC="${CURL_CONNECT_TIMEOUT_SEC:-2}"
CURL_MAX_TIME_SEC="${CURL_MAX_TIME_SEC:-5}"

BITCOIND_PID=""
BALANCE_HISTORY_PID=""
USDB_INDEXER_PID=""
BITCOIND_BIN=""
BITCOIN_CLI_BIN=""

log() {
  echo "[usdb-regtest-e2e] $*"
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

resolve_bitcoin_binaries() {
  local candidate_bitcoind=""
  local candidate_bitcoin_cli=""

  if [[ -n "$BITCOIN_BIN_DIR" ]]; then
    candidate_bitcoind="${BITCOIN_BIN_DIR}/bitcoind"
    candidate_bitcoin_cli="${BITCOIN_BIN_DIR}/bitcoin-cli"
    if [[ -x "$candidate_bitcoind" ]] && [[ -x "$candidate_bitcoin_cli" ]]; then
      BITCOIND_BIN="$candidate_bitcoind"
      BITCOIN_CLI_BIN="$candidate_bitcoin_cli"
      log "Using Bitcoin Core binaries from BITCOIN_BIN_DIR=${BITCOIN_BIN_DIR}"
      return
    fi
  fi

  BITCOIND_BIN="$(command -v bitcoind || true)"
  BITCOIN_CLI_BIN="$(command -v bitcoin-cli || true)"
  if [[ -z "$BITCOIND_BIN" || -z "$BITCOIN_CLI_BIN" ]]; then
    echo "Missing required commands bitcoind/bitcoin-cli. Tried BITCOIN_BIN_DIR=${BITCOIN_BIN_DIR} and PATH." >&2
    exit 1
  fi
  log "Using Bitcoin Core binaries from PATH"
}

cleanup() {
  set +e

  if [[ -n "$USDB_INDEXER_PID" ]] && kill -0 "$USDB_INDEXER_PID" 2>/dev/null; then
    log "Stopping usdb-indexer process pid=$USDB_INDEXER_PID"
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${USDB_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1
    for _ in $(seq 1 20); do
      if ! kill -0 "$USDB_INDEXER_PID" 2>/dev/null; then
        break
      fi
      sleep 0.5
    done
    kill -9 "$USDB_INDEXER_PID" 2>/dev/null || true
  fi

  if [[ -n "$BALANCE_HISTORY_PID" ]] && kill -0 "$BALANCE_HISTORY_PID" 2>/dev/null; then
    log "Stopping balance-history process pid=$BALANCE_HISTORY_PID"
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1
    for _ in $(seq 1 20); do
      if ! kill -0 "$BALANCE_HISTORY_PID" 2>/dev/null; then
        break
      fi
      sleep 0.5
    done
    kill -9 "$BALANCE_HISTORY_PID" 2>/dev/null || true
  fi

  if [[ -n "$BITCOIN_CLI_BIN" ]] && [[ -x "$BITCOIN_CLI_BIN" ]]; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" stop >/dev/null 2>&1 || true
  fi
}

json_extract_python() {
  local script="$1"
  python3 -c "$script"
}

parse_json_string_result() {
  sed -n 's/.*"result"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1
}

rpc_call_balance_history() {
  local method="$1"
  local params="${2:-[]}"
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

rpc_call_usdb_indexer() {
  local method="$1"
  local params="${2:-[]}"
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    -X POST "http://127.0.0.1:${USDB_RPC_PORT}" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

wait_rpc_ready() {
  local service_name="$1"
  local url="$2"
  local method="$3"
  local params="$4"

  log "Waiting for ${service_name} RPC readiness"
  for _ in $(seq 1 120); do
    if curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "$url" -H 'content-type: application/json' \
      --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}" >/dev/null 2>&1; then
      return
    fi
    sleep 0.5
  done

  log "${service_name} RPC is not ready at ${url}"
  exit 1
}

wait_until_balance_history_synced() {
  local target_height="$1"
  local start_ts now resp synced
  log "Waiting until balance-history synced height >= ${target_height}"

  start_ts="$(date +%s)"
  while true; do
    resp="$(rpc_call_balance_history "get_block_height" "[]" || true)"
    synced="$(echo "$resp" | json_extract_python 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit(0)
print(d.get("result", 0))' 2>/dev/null || true)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      log "balance-history synced height=${synced}"
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "balance-history sync timeout, last response: ${resp}"
      exit 1
    fi
    sleep 1
  done
}

wait_until_usdb_synced() {
  local target_height="$1"
  local start_ts now resp synced
  log "Waiting until usdb-indexer synced height >= ${target_height}"

  start_ts="$(date +%s)"
  while true; do
    resp="$(rpc_call_usdb_indexer "get_synced_block_height" "[]" || true)"
    synced="$(echo "$resp" | json_extract_python 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit(0)
r = d.get("result")
print(0 if r is None else r)' 2>/dev/null || true)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      log "usdb-indexer synced height=${synced}"
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "usdb-indexer sync timeout, last response: ${resp}"
      exit 1
    fi
    sleep 1
  done
}

btc_amount_to_sat() {
  local amount_btc="$1"
  python3 - "$amount_btc" <<'PY'
from decimal import Decimal, ROUND_DOWN
import sys
amount = Decimal(sys.argv[1])
sat = int((amount * Decimal("100000000")).to_integral_value(rounding=ROUND_DOWN))
print(sat)
PY
}

address_to_script_hash() {
  local address="$1"
  local script_pubkey
  script_pubkey="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    getaddressinfo "$address" | json_extract_python 'import json,sys; print(json.load(sys.stdin)["scriptPubKey"])')"

  python3 - "$script_pubkey" <<'PY'
import hashlib
import sys
script_hex = sys.argv[1]
script = bytes.fromhex(script_hex)
print(hashlib.sha256(script).digest()[::-1].hex())
PY
}

create_balance_history_config() {
  mkdir -p "$BALANCE_HISTORY_ROOT"
  cat >"${BALANCE_HISTORY_ROOT}/config.toml" <<EOF
root_dir = "${BALANCE_HISTORY_ROOT}"

[btc]
network = "regtest"
data_dir = "${BITCOIN_DIR}/regtest"
rpc_url = "http://127.0.0.1:${BTC_RPC_PORT}"

[ordinals]
rpc_url = "http://127.0.0.1:8070"

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

create_usdb_indexer_config() {
  mkdir -p "$USDB_INDEXER_ROOT"
  cat >"${USDB_INDEXER_ROOT}/config.json" <<EOF
{
  "isolate": null,
  "bitcoin": {
    "network": "regtest",
    "data_dir": "${BITCOIN_DIR}/regtest",
    "rpc_url": "http://127.0.0.1:${BTC_RPC_PORT}"
  },
  "ordinals": {
    "rpc_url": "http://127.0.0.1:8070"
  },
  "balance_history": {
    "rpc_url": "http://127.0.0.1:${BH_RPC_PORT}"
  },
  "usdb": {
    "genesis_block_height": 1,
    "active_address_page_size": 1024,
    "balance_query_batch_size": 256,
    "balance_query_concurrency": 4,
    "balance_query_timeout_ms": 10000,
    "balance_query_max_retries": 2,
    "inscription_source": "bitcoind",
    "inscription_source_shadow_compare": false,
    "inscription_source_shadow_fail_fast": false,
    "rpc_server_port": ${USDB_RPC_PORT},
    "rpc_server_enabled": true,
    "monitor_ord_enabled": false
  }
}
EOF
}

main() {
  trap cleanup EXIT

  resolve_bitcoin_binaries
  require_cmd cargo
  require_cmd curl
  require_cmd python3

  mkdir -p "$WORK_DIR" "$BITCOIN_DIR" "$BALANCE_HISTORY_ROOT" "$USDB_INDEXER_ROOT"
  log "Workspace directory: $WORK_DIR"

  local effective_target_height
  effective_target_height="$TARGET_HEIGHT"
  if [[ "$ENABLE_TRANSFER_CHECK" == "1" ]] && (( TARGET_HEIGHT < MIN_SPENDABLE_BLOCK_HEIGHT )); then
    effective_target_height="$MIN_SPENDABLE_BLOCK_HEIGHT"
    log "TARGET_HEIGHT=${TARGET_HEIGHT} is lower than spendable requirement ${MIN_SPENDABLE_BLOCK_HEIGHT}; using effective target ${effective_target_height} for transfer check."
  fi

  log "Starting bitcoind regtest on rpcport=${BTC_RPC_PORT}, bin=${BITCOIND_BIN}"
  "$BITCOIND_BIN" \
    -regtest \
    -server=1 \
    -txindex=1 \
    -fallbackfee=0.0001 \
    -datadir="$BITCOIN_DIR" \
    -rpcport="$BTC_RPC_PORT" \
    -daemonwait

  BITCOIND_PID="$(pgrep -f "bitcoind.*-datadir=${BITCOIN_DIR}" | head -n 1 || true)"
  if [[ -z "$BITCOIND_PID" ]]; then
    log "Failed to detect bitcoind PID"
    exit 1
  fi

  log "Creating/Loading wallet ${WALLET_NAME}"
  if ! "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    getwalletinfo >/dev/null 2>&1; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -named createwallet wallet_name="$WALLET_NAME" load_on_startup=true >/dev/null 2>&1 || true
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      loadwallet "$WALLET_NAME" >/dev/null 2>&1 || true
  fi
  if ! "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    getwalletinfo >/dev/null 2>&1; then
    log "Failed to create/load wallet: ${WALLET_NAME}"
    exit 1
  fi

  local mining_address
  mining_address="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" getnewaddress)"
  log "Mining ${effective_target_height} blocks to address=${mining_address}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    generatetoaddress "$effective_target_height" "$mining_address" >/dev/null

  create_balance_history_config
  create_usdb_indexer_config
  log "Generated balance-history config at ${BALANCE_HISTORY_ROOT}/config.toml"
  log "Generated usdb-indexer config at ${USDB_INDEXER_ROOT}/config.json"

  (
    cd "$REPO_ROOT"
    cargo run --manifest-path src/btc/Cargo.toml -p balance-history -- \
      --root-dir "$BALANCE_HISTORY_ROOT" \
      --skip-process-lock
  ) >"${WORK_DIR}/balance-history.log" 2>&1 &
  BALANCE_HISTORY_PID=$!

  wait_rpc_ready "balance-history" "http://127.0.0.1:${BH_RPC_PORT}" "get_network_type" "[]"

  local bh_network_resp bh_network
  bh_network_resp="$(rpc_call_balance_history "get_network_type" "[]")"
  bh_network="$(echo "$bh_network_resp" | parse_json_string_result)"
  log "balance-history network=${bh_network}"
  if [[ "$bh_network" != "regtest" ]]; then
    log "Unexpected balance-history network response: ${bh_network_resp}"
    exit 1
  fi

  wait_until_balance_history_synced "$effective_target_height"

  (
    cd "$REPO_ROOT"
    cargo run --manifest-path src/btc/Cargo.toml -p usdb-indexer -- \
      --root-dir "$USDB_INDEXER_ROOT" \
      --skip-process-lock
  ) >"${WORK_DIR}/usdb-indexer.log" 2>&1 &
  USDB_INDEXER_PID=$!

  wait_rpc_ready "usdb-indexer" "http://127.0.0.1:${USDB_RPC_PORT}" "get_network_type" "[]"

  local usdb_network_resp usdb_network
  usdb_network_resp="$(rpc_call_usdb_indexer "get_network_type" "[]")"
  usdb_network="$(echo "$usdb_network_resp" | parse_json_string_result)"
  log "usdb-indexer network=${usdb_network}"
  if [[ "$usdb_network" != "regtest" ]]; then
    log "Unexpected usdb-indexer network response: ${usdb_network_resp}"
    exit 1
  fi

  wait_until_usdb_synced "$effective_target_height"

  if [[ "$ENABLE_TRANSFER_CHECK" == "1" ]]; then
    local receiver_address txid expected_height expected_sat script_hash bh_balance_resp got_balance
    receiver_address="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" getnewaddress)"
    log "Sending ${SEND_AMOUNT_BTC} BTC to receiver address=${receiver_address}"
    txid="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
    log "Created txid=${txid}"

    log "Mining 1 block to confirm transfer"
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
      generatetoaddress 1 "$mining_address" >/dev/null

    expected_height=$((effective_target_height + 1))
    wait_until_balance_history_synced "$expected_height"
    wait_until_usdb_synced "$expected_height"

    script_hash="$(address_to_script_hash "$receiver_address")"
    expected_sat="$(btc_amount_to_sat "$SEND_AMOUNT_BTC")"
    bh_balance_resp="$(rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash}\",\"block_height\":${expected_height},\"block_range\":null}]")"
    got_balance="$(echo "$bh_balance_resp" | json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result",[]); print(r[0]["balance"] if r else 0)')"

    log "Balance assertion: height=${expected_height}, script_hash=${script_hash}, expected=${expected_sat}, got=${got_balance}"
    if [[ "$got_balance" != "$expected_sat" ]]; then
      log "Balance assertion failed, response: ${bh_balance_resp}"
      exit 1
    fi
  fi

  log "E2E smoke test succeeded."
  log "Logs: ${WORK_DIR}/balance-history.log, ${WORK_DIR}/usdb-indexer.log"
}

main "$@"
