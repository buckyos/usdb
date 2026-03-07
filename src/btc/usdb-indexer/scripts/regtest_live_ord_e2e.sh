#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-XXXXXX)}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-ord}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
SCENARIO_RUNNER="${SCENARIO_RUNNER:-$REPO_ROOT/src/btc/usdb-indexer/scripts/regtest_scenario_runner.py}"

BTC_RPC_PORT="${BTC_RPC_PORT:-19473}"
BTC_P2P_PORT="${BTC_P2P_PORT:-19474}"
BH_RPC_PORT="${BH_RPC_PORT:-18093}"
USDB_RPC_PORT="${USDB_RPC_PORT:-18113}"
ORD_SERVER_PORT="${ORD_SERVER_PORT:-18094}"

MINER_WALLET_NAME="${MINER_WALLET_NAME:-usdb-live-miner}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-live}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
ORD_FEE_RATE="${ORD_FEE_RATE:-1}"
FUND_ORD_AMOUNT_BTC="${FUND_ORD_AMOUNT_BTC:-5.0}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
CURL_CONNECT_TIMEOUT_SEC="${CURL_CONNECT_TIMEOUT_SEC:-2}"
CURL_MAX_TIME_SEC="${CURL_MAX_TIME_SEC:-8}"

ORD_CONTENT_FILE="${ORD_CONTENT_FILE:-}"

BITCOIND_BIN=""
BITCOIN_CLI_BIN=""
BITCOIND_PID=""
BALANCE_HISTORY_PID=""
USDB_INDEXER_PID=""
ORD_SERVER_PID=""

log() {
  echo "[usdb-live-ord-e2e] $*"
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

cleanup() {
  set +e

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

wait_http_ready() {
  local service_name="$1"
  local url="$2"

  log "Waiting for ${service_name} HTTP readiness"
  for _ in $(seq 1 120); do
    if curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      "$url" >/dev/null 2>&1; then
      return
    fi
    sleep 0.5
  done

  log "${service_name} HTTP is not ready at ${url}"
  exit 1
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
    log "Detected existing ord server on port ${ORD_SERVER_PORT} with chain=${chain}. This live e2e requires an isolated regtest ord server."
    log "Please stop that service or set a different ORD_SERVER_PORT."
    exit 1
  fi

  log "Detected existing regtest ord server on port ${ORD_SERVER_PORT}."
  log "To avoid shared state contamination, this script requires an unused ORD_SERVER_PORT."
  log "Please stop the existing service or set a different ORD_SERVER_PORT."
  exit 1
}

rpc_call() {
  local url="$1"
  local method="$2"
  local params="${3:-[]}"
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    -X POST "$url" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

json_result_u32() {
  python3 -c 'import json,sys; print(int(json.load(sys.stdin).get("result", 0)))'
}

wait_until_balance_history_synced() {
  local target_height="$1"
  local start_ts now resp synced
  start_ts="$(date +%s)"

  while true; do
    resp="$(rpc_call "http://127.0.0.1:${BH_RPC_PORT}" "get_block_height" "[]" || true)"
    synced="$(echo "$resp" | json_result_u32 2>/dev/null || true)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
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

extract_first_match() {
  local pattern="$1"
  python3 - "$pattern" <<'PY'
import re
import sys

pattern = sys.argv[1]
text = sys.stdin.read()
match = re.search(pattern, text)
print(match.group(0) if match else "")
PY
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

extract_inscription_id() {
  local raw="$1"
  local parsed=""
  parsed="$(python3 - "$raw" <<'PY'
import json
import re
import sys

raw = sys.argv[1]
candidates = [raw]
match = re.search(r"\{.*\}", raw, re.S)
if match:
    candidates.insert(0, match.group(0))

for item in candidates:
    try:
        payload = json.loads(item)
    except Exception:
        continue

    keys = [
        payload.get("inscription"),
        payload.get("inscription_id"),
        payload.get("id"),
    ]
    for value in keys:
        if isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}i\d+", value):
            print(value)
            raise SystemExit(0)

    inscriptions = payload.get("inscriptions")
    if isinstance(inscriptions, list):
        for value in inscriptions:
            if isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}i\d+", value):
                print(value)
                raise SystemExit(0)

match = re.search(r"([0-9a-f]{64}i\d+)", raw)
print(match.group(1) if match else "")
PY
)"
  echo "$parsed"
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

run_ord_wallet() {
  run_ord wallet \
    --no-sync \
    --server-url "http://127.0.0.1:${ORD_SERVER_PORT}" \
    --name "$ORD_WALLET_NAME" \
    "$@"
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

build_live_scenario() {
  local scenario_file="$1"
  local inscription_id="$2"
  local block_height="$3"
  cat >"$scenario_file" <<EOF
{
  "name": "live-ord-mint-assert",
  "steps": [
    {
      "type": "wait_balance_history_synced",
      "height": ${block_height}
    },
    {
      "type": "wait_usdb_synced",
      "height": ${block_height}
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id}",
          "at_height": ${block_height}
        }
      ],
      "result_only": true,
      "var": "pass_snapshot"
    },
    {
      "type": "assert_eq",
      "left": "\$pass_snapshot.inscription_id",
      "right": "${inscription_id}"
    },
    {
      "type": "assert_eq",
      "left": "\$pass_snapshot.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id}",
          "block_height": ${block_height},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass_energy"
    },
    {
      "type": "assert_eq",
      "left": "\$pass_energy.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_active_balance_snapshot",
      "params": [
        {
          "block_height": ${block_height}
        }
      ],
      "result_only": true,
      "var": "balance_snapshot"
    },
    {
      "type": "assert_ge",
      "left": "\$balance_snapshot.active_address_count",
      "right": 1
    },
    {
      "type": "assert_gt",
      "left": "\$balance_snapshot.total_balance",
      "right": 0
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_invalid_passes",
      "params": [
        {
          "from_height": 1,
          "to_height": ${block_height},
          "page": 0,
          "page_size": 20
        }
      ],
      "result_only": true,
      "var": "invalid_page"
    },
    {
      "type": "assert_eq",
      "left": "\$invalid_page.resolved_height",
      "right": ${block_height}
    },
    {
      "type": "assert_len",
      "value": "\$invalid_page.items",
      "expected_len": 0
    }
  ]
}
EOF
}

main() {
  trap cleanup EXIT

  resolve_bitcoin_binaries
  require_cmd "$ORD_BIN"
  require_cmd cargo
  require_cmd curl
  require_cmd python3
  assert_ord_server_port_available

  mkdir -p "$WORK_DIR" "$BITCOIN_DIR" "$ORD_DATA_DIR" "$BALANCE_HISTORY_ROOT" "$USDB_INDEXER_ROOT"
  log "Workspace directory: $WORK_DIR"

  log "Starting bitcoind on rpcport=${BTC_RPC_PORT}"
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

  log "Creating/Loading miner wallet ${MINER_WALLET_NAME}"
  if ! "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" getwalletinfo >/dev/null 2>&1; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -named createwallet wallet_name="$MINER_WALLET_NAME" load_on_startup=true >/dev/null 2>&1 || true
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" loadwallet "$MINER_WALLET_NAME" >/dev/null 2>&1 || true
  fi

  local miner_address
  miner_address="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" getnewaddress)"
  log "Premining ${PREMINE_BLOCKS} blocks to ${miner_address}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    generatetoaddress "$PREMINE_BLOCKS" "$miner_address" >/dev/null

  log "Starting temporary ord server for wallet operations on http://127.0.0.1:${ORD_SERVER_PORT}"
  run_ord --index-addresses server --address 127.0.0.1 --http --http-port "$ORD_SERVER_PORT" \
    >"${WORK_DIR}/ord-server.log" 2>&1 &
  ORD_SERVER_PID=$!
  wait_http_ready "ord-server" "http://127.0.0.1:${ORD_SERVER_PORT}/blockcount"
  wait_until_ord_server_synced_to_bitcoind

  log "Preparing ord wallet: ${ORD_WALLET_NAME}"
  run_ord_wallet create >/dev/null 2>&1 || true
  local ord_receive_output ord_receive_address
  ord_receive_output="$(run_ord_wallet receive 2>&1 || true)"
  ord_receive_address="$(extract_bech32_address "$ord_receive_output")"
  if [[ -z "$ord_receive_address" ]]; then
    log "Failed to parse ord wallet receive address from output: ${ord_receive_output}"
    exit 1
  fi
  log "Funding ord wallet address: ${ord_receive_address}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    sendtoaddress "$ord_receive_address" "$FUND_ORD_AMOUNT_BTC" >/dev/null
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    generatetoaddress "$FUND_CONFIRM_BLOCKS" "$miner_address" >/dev/null
  wait_until_ord_server_synced_to_bitcoind

  if [[ -z "$ORD_CONTENT_FILE" ]]; then
    ORD_CONTENT_FILE="$WORK_DIR/usdb_live_mint.json"
    cat >"$ORD_CONTENT_FILE" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF
  fi
  if [[ ! -f "$ORD_CONTENT_FILE" ]]; then
    log "ORD_CONTENT_FILE does not exist: $ORD_CONTENT_FILE"
    exit 1
  fi

  log "Inscribe mint via ord CLI: fee_rate=${ORD_FEE_RATE}, content_file=${ORD_CONTENT_FILE}"
  local inscribe_output inscription_id
  inscribe_output="$(run_ord_wallet inscribe --fee-rate "$ORD_FEE_RATE" --file "$ORD_CONTENT_FILE" 2>&1 || true)"
  inscription_id="$(extract_inscription_id "$inscribe_output")"
  if [[ -z "$inscription_id" ]]; then
    log "Failed to parse inscription id from ord output: ${inscribe_output}"
    exit 1
  fi
  log "Inscribe created inscription_id=${inscription_id}"

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    generatetoaddress "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address" >/dev/null
  wait_until_ord_server_synced_to_bitcoind
  local target_height
  target_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  log "Current chain height after inscribe confirmations: ${target_height}"

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
  wait_until_balance_history_synced "$target_height"

  (
    cd "$REPO_ROOT"
    cargo run --manifest-path src/btc/Cargo.toml -p usdb-indexer -- \
      --root-dir "$USDB_INDEXER_ROOT" \
      --skip-process-lock
  ) >"${WORK_DIR}/usdb-indexer.log" 2>&1 &
  USDB_INDEXER_PID=$!

  wait_rpc_ready "usdb-indexer" "http://127.0.0.1:${USDB_RPC_PORT}" "get_network_type" "[]"

  local scenario_file
  scenario_file="$WORK_DIR/live_ord_mint_assert.json"
  build_live_scenario "$scenario_file" "$inscription_id" "$target_height"

  python3 "$SCENARIO_RUNNER" \
    --btc-cli "$BITCOIN_CLI_BIN" \
    --bitcoin-dir "$BITCOIN_DIR" \
    --btc-rpc-port "$BTC_RPC_PORT" \
    --wallet-name "$MINER_WALLET_NAME" \
    --balance-history-rpc-url "http://127.0.0.1:${BH_RPC_PORT}" \
    --usdb-rpc-url "http://127.0.0.1:${USDB_RPC_PORT}" \
    --target-height "$target_height" \
    --sync-timeout-sec "$SYNC_TIMEOUT_SEC" \
    --send-amount-btc "1.0" \
    --min-spendable-block-height 101 \
    --rpc-connect-timeout-sec "$CURL_CONNECT_TIMEOUT_SEC" \
    --rpc-max-time-sec "$CURL_MAX_TIME_SEC" \
    --mining-address "$miner_address" \
    --skip-initial-usdb-state-assert \
    --scenario-file "$scenario_file"

  log "Live ord mint e2e succeeded: inscription_id=${inscription_id}, target_height=${target_height}"
  log "Logs: ${WORK_DIR}/balance-history.log, ${WORK_DIR}/usdb-indexer.log"
}

main "$@"
