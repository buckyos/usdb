#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-world-sim-XXXXXX)}"

BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-ord}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
WORLD_SIMULATOR="${WORLD_SIMULATOR:-$REPO_ROOT/src/btc/usdb-indexer/scripts/regtest_world_simulator.py}"

BTC_RPC_PORT="${BTC_RPC_PORT:-19483}"
BTC_P2P_PORT="${BTC_P2P_PORT:-19484}"
BH_RPC_PORT="${BH_RPC_PORT:-18123}"
USDB_RPC_PORT="${USDB_RPC_PORT:-18143}"
ORD_SERVER_PORT="${ORD_SERVER_PORT:-18124}"

MINER_WALLET_NAME="${MINER_WALLET_NAME:-usdb-world-miner}"
ORD_WALLET_PREFIX="${ORD_WALLET_PREFIX:-usdb-world-agent}"
AGENT_COUNT="${AGENT_COUNT:-5}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-140}"
FUND_AGENT_AMOUNT_BTC="${FUND_AGENT_AMOUNT_BTC:-4.0}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"

SIM_BLOCKS="${SIM_BLOCKS:-300}"
SIM_SEED="${SIM_SEED:-42}"
SIM_FEE_RATE="${SIM_FEE_RATE:-1}"
SIM_MAX_ACTIONS_PER_BLOCK="${SIM_MAX_ACTIONS_PER_BLOCK:-2}"
SIM_MINT_PROBABILITY="${SIM_MINT_PROBABILITY:-0.20}"
SIM_INVALID_MINT_PROBABILITY="${SIM_INVALID_MINT_PROBABILITY:-0.02}"
SIM_TRANSFER_PROBABILITY="${SIM_TRANSFER_PROBABILITY:-0.20}"
SIM_REMINT_PROBABILITY="${SIM_REMINT_PROBABILITY:-0.10}"
SIM_SEND_PROBABILITY="${SIM_SEND_PROBABILITY:-0.30}"
SIM_SPEND_PROBABILITY="${SIM_SPEND_PROBABILITY:-0.15}"
SIM_SLEEP_MS_BETWEEN_BLOCKS="${SIM_SLEEP_MS_BETWEEN_BLOCKS:-0}"
SIM_FAIL_FAST="${SIM_FAIL_FAST:-0}"
SIM_INITIAL_ACTIVE_AGENTS="${SIM_INITIAL_ACTIVE_AGENTS:-3}"
SIM_AGENT_GROWTH_INTERVAL_BLOCKS="${SIM_AGENT_GROWTH_INTERVAL_BLOCKS:-30}"
SIM_AGENT_GROWTH_STEP="${SIM_AGENT_GROWTH_STEP:-1}"
SIM_POLICY_MODE="${SIM_POLICY_MODE:-adaptive}"
SIM_SCRIPTED_CYCLE="${SIM_SCRIPTED_CYCLE:-mint,send_balance,transfer,remint,spend_balance,noop}"
SIM_REPORT_ENABLED="${SIM_REPORT_ENABLED:-1}"
SIM_REPORT_FILE="${SIM_REPORT_FILE:-$WORK_DIR/world-sim-report.jsonl}"
SIM_REPORT_FLUSH_EVERY="${SIM_REPORT_FLUSH_EVERY:-1}"

CURL_CONNECT_TIMEOUT_SEC="${CURL_CONNECT_TIMEOUT_SEC:-2}"
CURL_MAX_TIME_SEC="${CURL_MAX_TIME_SEC:-8}"
DIAG_TAIL_LINES="${DIAG_TAIL_LINES:-120}"

BITCOIND_BIN=""
BITCOIN_CLI_BIN=""
BITCOIND_PID=""
BALANCE_HISTORY_PID=""
USDB_INDEXER_PID=""
ORD_SERVER_PID=""
DIAGNOSTIC_PRINTED=0
LAST_ERROR_LINE="unknown"
LAST_ERROR_COMMAND="script_exit"

AGENT_WALLETS=()
AGENT_ADDRESSES=()

log() {
  echo "[usdb-world-sim] $*"
}

print_tail_if_exists() {
  local label="$1"
  local file_path="$2"
  if [[ -f "$file_path" ]]; then
    log "---- ${label} (tail -n ${DIAG_TAIL_LINES}) ----"
    tail -n "$DIAG_TAIL_LINES" "$file_path" || true
    log "---- end ${label} ----"
  else
    log "Diagnostic file not found: ${label} path=${file_path}"
  fi
}

print_failure_diagnostics() {
  local exit_code="$1"
  local line_no="$2"
  local command_text="$3"
  if [[ "$DIAGNOSTIC_PRINTED" == "1" ]]; then
    return
  fi
  DIAGNOSTIC_PRINTED=1

  log "Failure diagnostics: exit_code=${exit_code}, line=${line_no}, command=${command_text}"
  log "Runtime context: work_dir=${WORK_DIR}, btc_rpc_port=${BTC_RPC_PORT}, btc_p2p_port=${BTC_P2P_PORT}, ord_server_port=${ORD_SERVER_PORT}, bh_rpc_port=${BH_RPC_PORT}, usdb_rpc_port=${USDB_RPC_PORT}"

  print_tail_if_exists "ord-server.log" "${WORK_DIR}/ord-server.log"
  print_tail_if_exists "balance-history.log" "${WORK_DIR}/balance-history.log"
  print_tail_if_exists "usdb-indexer.log" "${WORK_DIR}/usdb-indexer.log"
  print_tail_if_exists "bitcoind-debug.log" "${BITCOIN_DIR}/regtest/debug.log"
  if [[ "${SIM_REPORT_ENABLED}" == "1" ]]; then
    print_tail_if_exists "world-sim-report" "${SIM_REPORT_FILE}"
  fi
}

on_error() {
  local exit_code="$1"
  local line_no="$2"
  local command_text="$3"
  LAST_ERROR_LINE="$line_no"
  LAST_ERROR_COMMAND="$command_text"
  print_failure_diagnostics "$exit_code" "$line_no" "$command_text"
}

on_exit() {
  local exit_code=$?
  if [[ "$exit_code" -ne 0 ]]; then
    print_failure_diagnostics "$exit_code" "$LAST_ERROR_LINE" "$LAST_ERROR_COMMAND"
  fi
  cleanup
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
      return
    fi
  fi

  BITCOIND_BIN="$(command -v bitcoind || true)"
  BITCOIN_CLI_BIN="$(command -v bitcoin-cli || true)"
  if [[ -z "$BITCOIND_BIN" || -z "$BITCOIN_CLI_BIN" ]]; then
    echo "Missing required commands bitcoind/bitcoin-cli. Tried BITCOIN_BIN_DIR=${BITCOIN_BIN_DIR} and PATH." >&2
    exit 1
  fi
}

run_ord() {
  "$ORD_BIN" \
    --regtest \
    --bitcoin-rpc-url "http://127.0.0.1:${BTC_RPC_PORT}" \
    --cookie-file "${BITCOIN_DIR}/regtest/.cookie" \
    --bitcoin-data-dir "$BITCOIN_DIR" \
    --data-dir "$ORD_DATA_DIR" \
    "$@"
}

run_ord_wallet_named() {
  local wallet_name="$1"
  shift
  run_ord wallet \
    --no-sync \
    --server-url "http://127.0.0.1:${ORD_SERVER_PORT}" \
    --name "$wallet_name" \
    "$@"
}

extract_bech32_address() {
  local raw="$1"
  python3 - "$raw" <<'PY'
import json
import re
import sys

raw = sys.argv[1]

def match_text(text: str) -> str:
    m = re.search(r"(bc1|tb1|bcrt1)[ac-hj-np-z02-9]{20,}", text)
    return m.group(0) if m else ""

for candidate in [raw]:
    try:
        payload = json.loads(candidate)
    except Exception:
        payload = None
    if isinstance(payload, dict):
        values = []
        address = payload.get("address")
        if isinstance(address, str):
            values.append(address)
        addresses = payload.get("addresses")
        if isinstance(addresses, list):
            values.extend([v for v in addresses if isinstance(v, str)])
        for value in values:
            matched = match_text(value)
            if matched:
                print(matched)
                raise SystemExit(0)

matched = match_text(raw)
print(matched)
PY
}

detect_ord_chain_on_port() {
  local status_html
  status_html="$(curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    "http://127.0.0.1:${ORD_SERVER_PORT}/status" 2>/dev/null || true)"
  if [[ -z "$status_html" ]]; then
    echo ""
    return
  fi

  python3 - "$status_html" <<'PY'
import re
import sys

html = sys.argv[1]
match = re.search(r"<dt>\s*chain\s*</dt>\s*<dd>\s*([^<\s]+)\s*</dd>", html, re.IGNORECASE)
print(match.group(1).strip().lower() if match else "")
PY
}

assert_ord_server_port_available() {
  local chain
  chain="$(detect_ord_chain_on_port)"
  if [[ -z "$chain" ]]; then
    return
  fi
  if [[ "$chain" != "regtest" ]]; then
    log "Detected existing ord server on port ${ORD_SERVER_PORT} with chain=${chain}. This script requires an isolated regtest ord server."
    exit 1
  fi
  log "Detected existing regtest ord server on port ${ORD_SERVER_PORT}. Please use a different ORD_SERVER_PORT for isolation."
  exit 1
}

stop_process() {
  local pid="$1"
  if [[ -z "$pid" ]]; then
    return
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    wait "$pid" >/dev/null 2>&1 || true
    return
  fi
  kill "$pid" >/dev/null 2>&1 || true
  for _ in $(seq 1 30); do
    if ! kill -0 "$pid" 2>/dev/null; then
      break
    fi
    sleep 0.1
  done
  if kill -0 "$pid" 2>/dev/null; then
    kill -9 "$pid" >/dev/null 2>&1 || true
  fi
  wait "$pid" >/dev/null 2>&1 || true
}

cleanup() {
  set +e

  if [[ -n "$USDB_INDEXER_PID" ]] && kill -0 "$USDB_INDEXER_PID" 2>/dev/null; then
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${USDB_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1
    stop_process "$USDB_INDEXER_PID"
  fi

  if [[ -n "$BALANCE_HISTORY_PID" ]] && kill -0 "$BALANCE_HISTORY_PID" 2>/dev/null; then
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1
    stop_process "$BALANCE_HISTORY_PID"
  fi

  if [[ -n "$ORD_SERVER_PID" ]] && kill -0 "$ORD_SERVER_PID" 2>/dev/null; then
    stop_process "$ORD_SERVER_PID"
  fi

  if [[ -n "$BITCOIN_CLI_BIN" ]] && [[ -x "$BITCOIN_CLI_BIN" ]]; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" stop >/dev/null 2>&1 || true
  fi
  if [[ -n "$BITCOIND_PID" ]]; then
    stop_process "$BITCOIND_PID"
  fi
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

json_result_u32() {
  python3 -c 'import json,sys; print(int(json.load(sys.stdin).get("result", 0)))'
}

wait_until_balance_history_synced() {
  local target_height="$1"
  local start_ts now resp synced
  start_ts="$(date +%s)"
  while true; do
    resp="$(curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${BH_RPC_PORT}" -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"get_block_height","params":[]}' || true)"
    synced="$(echo "$resp" | json_result_u32 2>/dev/null || true)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      return
    fi
    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "balance-history sync timeout, target=${target_height}, last=${resp}"
      exit 1
    fi
    sleep 1
  done
}

wait_until_usdb_synced() {
  local target_height="$1"
  local start_ts now resp synced
  start_ts="$(date +%s)"
  while true; do
    resp="$(curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${USDB_RPC_PORT}" -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"get_synced_block_height","params":[]}' || true)"
    synced="$(echo "$resp" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit(0)
res = d.get("result")
print(int(res) if res is not None else 0)
' 2>/dev/null || true)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      return
    fi
    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "usdb-indexer sync timeout, target=${target_height}, last=${resp}"
      exit 1
    fi
    sleep 1
  done
}

get_ord_server_block_height() {
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    "http://127.0.0.1:${ORD_SERVER_PORT}/blockcount" | tr -d '\n\r '
}

wait_until_ord_server_synced_to_bitcoind() {
  local start_ts now ord_height btc_height
  start_ts="$(date +%s)"
  while true; do
    btc_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount 2>/dev/null || echo 0)"
    ord_height="$(get_ord_server_block_height 2>/dev/null || echo 0)"
    if [[ "$ord_height" =~ ^[0-9]+$ ]] && [[ "$btc_height" =~ ^[0-9]+$ ]] && [[ "$ord_height" -ge "$btc_height" ]]; then
      return
    fi
    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "ord server sync timeout: ord_height=${ord_height:-unknown}, btc_height=${btc_height:-unknown}"
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
    "inscription_fixture_file": null,
    "inscription_source_shadow_compare": false,
    "inscription_source_shadow_fail_fast": false,
    "rpc_server_port": ${USDB_RPC_PORT},
    "rpc_server_enabled": true,
    "monitor_ord_enabled": false
  }
}
EOF
}

join_by_comma() {
  local IFS=","
  echo "$*"
}

main() {
  trap 'on_error $? $LINENO "$BASH_COMMAND"' ERR
  trap on_exit EXIT

  resolve_bitcoin_binaries
  require_cmd "$ORD_BIN"
  require_cmd cargo
  require_cmd curl
  require_cmd python3
  assert_ord_server_port_available

  if [[ ! -f "$WORLD_SIMULATOR" ]]; then
    log "World simulator script not found: ${WORLD_SIMULATOR}"
    exit 1
  fi

  mkdir -p "$WORK_DIR" "$BITCOIN_DIR" "$ORD_DATA_DIR" "$BALANCE_HISTORY_ROOT" "$USDB_INDEXER_ROOT"
  log "Workspace directory: $WORK_DIR"

  log "Starting bitcoind: rpc_port=${BTC_RPC_PORT}, p2p_port=${BTC_P2P_PORT}"
  "$BITCOIND_BIN" \
    -regtest \
    -server=1 \
    -port="$BTC_P2P_PORT" \
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

  log "Creating/loading miner wallet ${MINER_WALLET_NAME}"
  if ! "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" getwalletinfo >/dev/null 2>&1; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -named createwallet wallet_name="$MINER_WALLET_NAME" load_on_startup=true >/dev/null 2>&1 || true
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      loadwallet "$MINER_WALLET_NAME" >/dev/null 2>&1 || true
  fi

  local mining_address
  mining_address="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" getnewaddress)"
  log "Premining ${PREMINE_BLOCKS} blocks: address=${mining_address}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    generatetoaddress "$PREMINE_BLOCKS" "$mining_address" >/dev/null

  log "Starting ord server: port=${ORD_SERVER_PORT}"
  run_ord --index-addresses --index-transactions server --address 127.0.0.1 --http --http-port "$ORD_SERVER_PORT" \
    >"${WORK_DIR}/ord-server.log" 2>&1 &
  ORD_SERVER_PID=$!
  wait_until_ord_server_synced_to_bitcoind

  log "Creating ${AGENT_COUNT} agent wallets"
  for i in $(seq 1 "$AGENT_COUNT"); do
    local wallet_name="${ORD_WALLET_PREFIX}-${i}"
    AGENT_WALLETS+=("$wallet_name")
    run_ord_wallet_named "$wallet_name" create >/dev/null 2>&1 || true

    local receive_output receive_address
    receive_output="$(run_ord_wallet_named "$wallet_name" receive 2>&1 || true)"
    receive_address="$(extract_bech32_address "$receive_output")"
    if [[ -z "$receive_address" ]]; then
      log "Failed to parse receive address: wallet=${wallet_name}, output=${receive_output}"
      exit 1
    fi
    AGENT_ADDRESSES+=("$receive_address")

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      sendtoaddress "$receive_address" "$FUND_AGENT_AMOUNT_BTC" >/dev/null
  done

  log "Funding agent wallets confirmed by ${FUND_CONFIRM_BLOCKS} blocks"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    generatetoaddress "$FUND_CONFIRM_BLOCKS" "$mining_address" >/dev/null
  wait_until_ord_server_synced_to_bitcoind

  create_balance_history_config
  create_usdb_indexer_config

  (
    cd "$REPO_ROOT"
    cargo run --manifest-path src/btc/Cargo.toml -p balance-history -- \
      --root-dir "$BALANCE_HISTORY_ROOT" \
      --skip-process-lock
  ) >"${WORK_DIR}/balance-history.log" 2>&1 &
  BALANCE_HISTORY_PID=$!
  wait_rpc_ready "balance-history" "http://127.0.0.1:${BH_RPC_PORT}" "get_network_type" "[]"

  (
    cd "$REPO_ROOT"
    cargo run --manifest-path src/btc/Cargo.toml -p usdb-indexer -- \
      --root-dir "$USDB_INDEXER_ROOT" \
      --skip-process-lock
  ) >"${WORK_DIR}/usdb-indexer.log" 2>&1 &
  USDB_INDEXER_PID=$!
  wait_rpc_ready "usdb-indexer" "http://127.0.0.1:${USDB_RPC_PORT}" "get_network_type" "[]"

  local current_height
  current_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  wait_until_balance_history_synced "$current_height"
  wait_until_usdb_synced "$current_height"

  local agent_wallets_csv
  local agent_addresses_csv
  agent_wallets_csv="$(join_by_comma "${AGENT_WALLETS[@]}")"
  agent_addresses_csv="$(join_by_comma "${AGENT_ADDRESSES[@]}")"

  log "Launching world simulator: blocks=${SIM_BLOCKS}, seed=${SIM_SEED}, agents=${AGENT_COUNT}"
  local fail_fast_arg=()
  if [[ "$SIM_FAIL_FAST" == "1" ]]; then
    fail_fast_arg+=(--fail-fast)
  fi
  local report_args=()
  if [[ "$SIM_REPORT_ENABLED" == "1" ]]; then
    report_args+=(--report-file "$SIM_REPORT_FILE")
    report_args+=(--report-flush-every "$SIM_REPORT_FLUSH_EVERY")
  fi

  python3 "$WORLD_SIMULATOR" \
    --btc-cli "$BITCOIN_CLI_BIN" \
    --bitcoin-dir "$BITCOIN_DIR" \
    --btc-rpc-port "$BTC_RPC_PORT" \
    --ord-bin "$ORD_BIN" \
    --ord-data-dir "$ORD_DATA_DIR" \
    --ord-server-url "http://127.0.0.1:${ORD_SERVER_PORT}" \
    --miner-wallet "$MINER_WALLET_NAME" \
    --mining-address "$mining_address" \
    --agent-wallets "$agent_wallets_csv" \
    --agent-addresses "$agent_addresses_csv" \
    --balance-history-rpc-url "http://127.0.0.1:${BH_RPC_PORT}" \
    --usdb-rpc-url "http://127.0.0.1:${USDB_RPC_PORT}" \
    --sync-timeout-sec "$SYNC_TIMEOUT_SEC" \
    --blocks "$SIM_BLOCKS" \
    --seed "$SIM_SEED" \
    --fee-rate "$SIM_FEE_RATE" \
    --max-actions-per-block "$SIM_MAX_ACTIONS_PER_BLOCK" \
    --mint-probability "$SIM_MINT_PROBABILITY" \
    --invalid-mint-probability "$SIM_INVALID_MINT_PROBABILITY" \
    --transfer-probability "$SIM_TRANSFER_PROBABILITY" \
    --remint-probability "$SIM_REMINT_PROBABILITY" \
    --send-probability "$SIM_SEND_PROBABILITY" \
    --spend-probability "$SIM_SPEND_PROBABILITY" \
    --sleep-ms-between-blocks "$SIM_SLEEP_MS_BETWEEN_BLOCKS" \
    --initial-active-agents "$SIM_INITIAL_ACTIVE_AGENTS" \
    --agent-growth-interval-blocks "$SIM_AGENT_GROWTH_INTERVAL_BLOCKS" \
    --agent-growth-step "$SIM_AGENT_GROWTH_STEP" \
    --policy-mode "$SIM_POLICY_MODE" \
    --scripted-cycle "$SIM_SCRIPTED_CYCLE" \
    "${report_args[@]}" \
    --temp-dir "$WORK_DIR" \
    "${fail_fast_arg[@]}"

  log "World simulation finished successfully."
  log "Logs: ${WORK_DIR}/ord-server.log, ${WORK_DIR}/balance-history.log, ${WORK_DIR}/usdb-indexer.log"
}

main "$@"
