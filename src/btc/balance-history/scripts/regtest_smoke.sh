#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-regtest-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-19443}"
BH_RPC_PORT="${BH_RPC_PORT:-18080}"
WALLET_NAME="${WALLET_NAME:-bhitest}"
TARGET_HEIGHT="${TARGET_HEIGHT:-120}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
ENABLE_TRANSFER_CHECK="${ENABLE_TRANSFER_CHECK:-1}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"

BITCOIND_PID=""
BALANCE_HISTORY_PID=""

log() {
  echo "[regtest-smoke] $*"
}

cleanup() {
  set +e

  if [[ -n "$BALANCE_HISTORY_PID" ]] && kill -0 "$BALANCE_HISTORY_PID" 2>/dev/null; then
    log "Stopping balance-history process pid=$BALANCE_HISTORY_PID"
    curl -s -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
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

  if command -v bitcoin-cli >/dev/null 2>&1; then
    bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" stop >/dev/null 2>&1 || true
  fi

  if [[ -n "$BITCOIND_PID" ]] && kill -0 "$BITCOIND_PID" 2>/dev/null; then
    for _ in $(seq 1 20); do
      if ! kill -0 "$BITCOIND_PID" 2>/dev/null; then
        break
      fi
      sleep 0.5
    done
    kill -9 "$BITCOIND_PID" 2>/dev/null || true
  fi
}

require_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "Missing required command: $1" >&2
    exit 1
  fi
}

rpc_call() {
  local method="$1"
  local params="${2:-[]}"
  curl -s -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

parse_json_number_result() {
  sed -n 's/.*"result"[[:space:]]*:[[:space:]]*\([0-9]\+\).*/\1/p' | head -n 1
}

parse_json_string_result() {
  sed -n 's/.*"result"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1
}

json_extract_python() {
  local script="$1"
  python3 -c "$script"
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
  script_pubkey="$(bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    getaddressinfo "$address" | json_extract_python 'import json,sys; print(json.load(sys.stdin)["scriptPubKey"])')"

  python3 - "$script_pubkey" <<'PY'
import hashlib
import sys

script_hex = sys.argv[1]
script = bytes.fromhex(script_hex)
digest = hashlib.sha256(script).digest()[::-1].hex()
print(digest)
PY
}

wait_until_synced_height() {
  local target_height="$1"
  log "Waiting until synced block height >= ${target_height}"

  local start_ts now synced height_resp
  start_ts="$(date +%s)"
  while true; do
    height_resp="$(rpc_call "get_block_height" "[]")"
    synced="$(echo "$height_resp" | parse_json_number_result)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      log "Sync reached height=${synced}"
      break
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "Sync timeout, last response: ${height_resp}"
      log "See log file: ${WORK_DIR}/balance-history.log"
      exit 1
    fi
    sleep 1
  done
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

main() {
  trap cleanup EXIT

  require_cmd bitcoind
  require_cmd bitcoin-cli
  require_cmd cargo
  require_cmd curl
  require_cmd python3

  mkdir -p "$WORK_DIR" "$BITCOIN_DIR" "$BALANCE_HISTORY_ROOT"
  log "Workspace directory: $WORK_DIR"

  log "Starting bitcoind regtest on rpcport=${BTC_RPC_PORT}"
  bitcoind \
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
  log "bitcoind pid=${BITCOIND_PID}"

  log "Creating/Loading wallet ${WALLET_NAME}"
  bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -named createwallet wallet_name="$WALLET_NAME" descriptors=false load_on_startup=true >/dev/null 2>&1 || true

  local mining_address
  mining_address="$(bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" getnewaddress)"
  log "Mining ${TARGET_HEIGHT} blocks to address=${mining_address}"
  bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    generatetoaddress "$TARGET_HEIGHT" "$mining_address" >/dev/null

  create_balance_history_config
  log "Generated balance-history config at ${BALANCE_HISTORY_ROOT}/config.toml"

  log "Starting balance-history service (root=${BALANCE_HISTORY_ROOT}, rpc=${BH_RPC_PORT})"
  (
    cd "$REPO_ROOT"
    cargo run --manifest-path src/btc/Cargo.toml -p balance-history -- \
      --root-dir "$BALANCE_HISTORY_ROOT" \
      --skip-process-lock
  ) >"${WORK_DIR}/balance-history.log" 2>&1 &
  BALANCE_HISTORY_PID=$!

  log "Waiting for balance-history RPC readiness"
  local ready=0
  for _ in $(seq 1 120); do
    if rpc_call "get_network_type" "[]" >/dev/null 2>&1; then
      ready=1
      break
    fi
    sleep 0.5
  done
  if [[ "$ready" -ne 1 ]]; then
    log "balance-history RPC not ready, see log: ${WORK_DIR}/balance-history.log"
    exit 1
  fi

  local network_resp network
  network_resp="$(rpc_call "get_network_type" "[]")"
  network="$(echo "$network_resp" | parse_json_string_result)"
  log "RPC get_network_type => ${network}"
  if [[ "$network" != "regtest" ]]; then
    log "Unexpected network type: ${network_resp}"
    exit 1
  fi

  wait_until_synced_height "$TARGET_HEIGHT"

  if [[ "$ENABLE_TRANSFER_CHECK" == "1" ]]; then
    local receiver_address txid expected_height script_hash balance_resp got_balance expected_sat

    receiver_address="$(bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" getnewaddress)"
    log "Sending ${SEND_AMOUNT_BTC} BTC to receiver address=${receiver_address}"
    txid="$(bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
    log "Created txid=${txid}"

    log "Mining 1 block to confirm transfer"
    bitcoin-cli -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
      generatetoaddress 1 "$mining_address" >/dev/null

    expected_height=$((TARGET_HEIGHT + 1))
    wait_until_synced_height "$expected_height"

    script_hash="$(address_to_script_hash "$receiver_address")"
    balance_resp="$(rpc_call "get_address_balance" "[{\"script_hash\":\"${script_hash}\",\"block_height\":${expected_height},\"block_range\":null}]")"
    got_balance="$(echo "$balance_resp" | json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result",[]); print(r[0]["balance"] if r else 0)')"
    expected_sat="$(btc_amount_to_sat "$SEND_AMOUNT_BTC")"

    log "Transfer balance check: height=${expected_height}, script_hash=${script_hash}, expected=${expected_sat}, got=${got_balance}"
    if [[ "$got_balance" != "$expected_sat" ]]; then
      log "Transfer balance mismatch, response: ${balance_resp}"
      exit 1
    fi
  fi

  log "Smoke test succeeded."
  log "Logs: ${WORK_DIR}/balance-history.log"
}

main "$@"
