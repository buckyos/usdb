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

BTC_RPC_PORT="${BTC_RPC_PORT:-28132}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28133}"
BH_RPC_PORT="${BH_RPC_PORT:-28110}"
USDB_RPC_PORT="${USDB_RPC_PORT:-28120}"
ORD_SERVER_PORT="${ORD_SERVER_PORT:-28130}"

MINER_WALLET_NAME="${MINER_WALLET_NAME:-usdb-live-miner}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-live-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-live-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
ORD_FEE_RATE="${ORD_FEE_RATE:-1}"
FUND_ORD_AMOUNT_BTC="${FUND_ORD_AMOUNT_BTC:-5.0}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
REMINT_CONFIRM_BLOCKS="${REMINT_CONFIRM_BLOCKS:-2}"
PENALTY_FUND_AMOUNT_BTC="${PENALTY_FUND_AMOUNT_BTC:-0.50000000}"
PENALTY_SPEND_AMOUNT_BTC="${PENALTY_SPEND_AMOUNT_BTC:-0.49950000}"
PENALTY_FUND_CONFIRM_BLOCKS="${PENALTY_FUND_CONFIRM_BLOCKS:-1}"
PENALTY_SPEND_CONFIRM_BLOCKS="${PENALTY_SPEND_CONFIRM_BLOCKS:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
CURL_CONNECT_TIMEOUT_SEC="${CURL_CONNECT_TIMEOUT_SEC:-2}"
CURL_MAX_TIME_SEC="${CURL_MAX_TIME_SEC:-8}"
DIAG_TAIL_LINES="${DIAG_TAIL_LINES:-120}"
LIVE_SCENARIO="${LIVE_SCENARIO:-transfer_remint}"

ORD_CONTENT_FILE="${ORD_CONTENT_FILE:-}"

BITCOIND_BIN=""
BITCOIN_CLI_BIN=""
BITCOIND_PID=""
BALANCE_HISTORY_PID=""
USDB_INDEXER_PID=""
ORD_SERVER_PID=""
SCENARIO_FILE_PATH=""
DIAGNOSTIC_PRINTED=0
LAST_ERROR_LINE="unknown"
LAST_ERROR_COMMAND="script_exit"

log() {
  echo "[usdb-live-ord-e2e] $*"
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
  log "Wallet context: miner_wallet=${MINER_WALLET_NAME}, ord_wallet_a=${ORD_WALLET_NAME}, ord_wallet_b=${ORD_WALLET_NAME_B}"

  print_tail_if_exists "ord-server.log" "${WORK_DIR}/ord-server.log"
  print_tail_if_exists "balance-history.log" "${WORK_DIR}/balance-history.log"
  print_tail_if_exists "usdb-indexer.log" "${WORK_DIR}/usdb-indexer.log"
  print_tail_if_exists "bitcoind-debug.log" "${BITCOIN_DIR}/regtest/debug.log"

  if [[ -n "$SCENARIO_FILE_PATH" ]]; then
    print_tail_if_exists "scenario-file" "$SCENARIO_FILE_PATH"
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

rpc_consensus_ready() {
  local url="$1"
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    -X POST "$url" \
    -H 'content-type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"get_readiness","params":[]}' \
    | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit(0)
r = d.get("result") or {}
print(1 if r.get("consensus_ready") else 0)'
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
  local start_ts now resp synced consensus_ready
  start_ts="$(date +%s)"

  while true; do
    resp="$(rpc_call "http://127.0.0.1:${BH_RPC_PORT}" "get_block_height" "[]" || true)"
    synced="$(echo "$resp" | json_result_u32 2>/dev/null || true)"
    synced="${synced:-0}"
    consensus_ready="$(rpc_consensus_ready "http://127.0.0.1:${BH_RPC_PORT}" 2>/dev/null || echo 0)"
    if [[ "$synced" -ge "$target_height" ]] && [[ "$consensus_ready" == "1" ]]; then
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

wait_until_usdb_consensus_ready() {
  local target_height="$1"
  local start_ts now resp synced consensus_ready
  start_ts="$(date +%s)"

  while true; do
    resp="$(rpc_call "http://127.0.0.1:${USDB_RPC_PORT}" "get_synced_block_height" "[]" || true)"
    synced="$(echo "$resp" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit(0)
r = d.get("result")
print(0 if r is None else int(r))' 2>/dev/null || true)"
    synced="${synced:-0}"
    consensus_ready="$(rpc_consensus_ready "http://127.0.0.1:${USDB_RPC_PORT}" 2>/dev/null || echo 0)"
    if [[ "$synced" -ge "$target_height" ]] && [[ "$consensus_ready" == "1" ]]; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "usdb-indexer readiness timeout: target_height=${target_height}, last_synced_response=${resp}"
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

wait_until_ord_wallet_has_inscription() {
  local wallet_name="$1"
  local inscription_id="$2"
  local start_ts now resp
  start_ts="$(date +%s)"
  while true; do
    resp="$(run_ord_wallet_named "$wallet_name" inscriptions 2>/dev/null || true)"
    if [[ "$resp" == *"$inscription_id"* ]]; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      log "ord wallet sync timeout: wallet=${wallet_name}, inscription_id=${inscription_id}, last_response=${resp}"
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

run_ord_wallet_named() {
  local wallet_name="$1"
  shift
  run_ord wallet \
    --no-sync \
    --server-url "http://127.0.0.1:${ORD_SERVER_PORT}" \
    --name "$wallet_name" \
    "$@"
}

extract_txid() {
  local raw="$1"
  python3 - "$raw" <<'PY'
import re
import sys

raw = sys.argv[1]
match = re.search(r"\b([0-9a-f]{64})\b", raw)
print(match.group(1) if match else "")
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
    "rpc_url": "http://127.0.0.1:${ORD_SERVER_PORT}"
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

build_live_transfer_remint_scenario() {
  local scenario_file="$1"
  local inscription_id_1="$2"
  local inscription_id_2="$3"
  local height_mint="$4"
  local height_transfer="$5"
  local height_remint="$6"
  cat >"$scenario_file" <<EOF
{
  "name": "live-ord-transfer-remint-assert",
  "steps": [
    {
      "type": "wait_balance_history_synced",
      "height": ${height_remint}
    },
    {
      "type": "wait_usdb_synced",
      "height": ${height_remint}
    },
    {
      "type": "log",
      "message": "Check mint height snapshot and energy"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_mint}
        }
      ],
      "result_only": true,
      "var": "pass1_mint"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_mint.inscription_id",
      "right": "${inscription_id_1}"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_mint.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "block_height": ${height_mint},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass1_energy_mint"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_energy_mint.state",
      "right": "active"
    },
    {
      "type": "assert_pass_energy_ge",
      "inscription_id": "${inscription_id_1}",
      "block_height": ${height_mint},
      "expected_min_energy": 0,
      "mode": "at_or_before",
      "expected_state": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_active_balance_snapshot",
      "params": [
        {
          "block_height": ${height_mint}
        }
      ],
      "result_only": true,
      "var": "snapshot_mint"
    },
    {
      "type": "assert_ge",
      "left": "\$snapshot_mint.active_address_count",
      "right": 1
    },
    {
      "type": "assert_gt",
      "left": "\$snapshot_mint.total_balance",
      "right": 0
    },
    {
      "type": "log",
      "message": "Check transfer height snapshot and energy"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_transfer}
        }
      ],
      "result_only": true,
      "var": "pass1_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_transfer.state",
      "right": "dormant"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "block_height": ${height_transfer},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass1_energy_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_energy_transfer.state",
      "right": "dormant"
    },
    {
      "type": "assert_pass_energy_delta",
      "inscription_id": "${inscription_id_1}",
      "from_height": ${height_mint},
      "to_height": ${height_transfer},
      "min_delta": 0,
      "mode": "at_or_before"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_active_balance_snapshot",
      "params": [
        {
          "block_height": ${height_transfer}
        }
      ],
      "result_only": true,
      "var": "snapshot_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$snapshot_transfer.active_address_count",
      "right": 0
    },
    {
      "type": "assert_eq",
      "left": "\$snapshot_transfer.total_balance",
      "right": 0
    },
    {
      "type": "log",
      "message": "Check remint(prev) height snapshot and energy"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_remint}
        }
      ],
      "result_only": true,
      "var": "pass1_remint"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_remint.state",
      "right": "dormant"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "block_height": ${height_remint},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass1_energy_remint"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_energy_remint.state",
      "right": "dormant"
    },
    {
      "type": "assert_pass_energy_delta",
      "inscription_id": "${inscription_id_1}",
      "from_height": ${height_transfer},
      "to_height": ${height_remint},
      "expected_delta": 0,
      "mode": "at_or_before"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "at_height": ${height_remint}
        }
      ],
      "result_only": true,
      "var": "pass2_remint"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_remint.inscription_id",
      "right": "${inscription_id_2}"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_remint.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_remint},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_remint"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_remint.state",
      "right": "active"
    },
    {
      "type": "assert_pass_energy_eq",
      "inscription_id": "${inscription_id_2}",
      "block_height": ${height_remint},
      "expected_energy": "\$pass1_energy_remint.energy",
      "mode": "at_or_before",
      "expected_state": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_active_balance_snapshot",
      "params": [
        {
          "block_height": ${height_remint}
        }
      ],
      "result_only": true,
      "var": "snapshot_remint"
    },
    {
      "type": "assert_ge",
      "left": "\$snapshot_remint.active_address_count",
      "right": 1
    },
    {
      "type": "assert_gt",
      "left": "\$snapshot_remint.total_balance",
      "right": 0
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_invalid_passes",
      "params": [
        {
          "from_height": 1,
          "to_height": ${height_remint},
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
      "right": ${height_remint}
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

build_live_invalid_mint_scenario() {
  local scenario_file="$1"
  local inscription_id="$2"
  local block_height="$3"
  cat >"$scenario_file" <<EOF
{
  "name": "live-ord-invalid-mint-assert",
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
      "var": "invalid_pass_snapshot"
    },
    {
      "type": "assert_eq",
      "left": "\$invalid_pass_snapshot.inscription_id",
      "right": "${inscription_id}"
    },
    {
      "type": "assert_eq",
      "left": "\$invalid_pass_snapshot.state",
      "right": "invalid"
    },
    {
      "type": "assert_eq",
      "left": "\$invalid_pass_snapshot.invalid_code",
      "right": "INVALID_ETH_MAIN"
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
      "expected_len": 1
    },
    {
      "type": "assert_eq",
      "left": "\$invalid_page.items.0.inscription_id",
      "right": "${inscription_id}"
    },
    {
      "type": "assert_eq",
      "left": "\$invalid_page.items.0.invalid_code",
      "right": "INVALID_ETH_MAIN"
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
      "type": "assert_eq",
      "left": "\$balance_snapshot.active_address_count",
      "right": 0
    },
    {
      "type": "assert_eq",
      "left": "\$balance_snapshot.total_balance",
      "right": 0
    }
  ]
}
EOF
}

build_live_passive_transfer_scenario() {
  local scenario_file="$1"
  local inscription_id_1="$2"
  local inscription_id_2="$3"
  local height_mint_2="$4"
  local height_transfer="$5"
  cat >"$scenario_file" <<EOF
{
  "name": "live-ord-passive-transfer-assert",
  "steps": [
    {
      "type": "wait_balance_history_synced",
      "height": ${height_transfer}
    },
    {
      "type": "wait_usdb_synced",
      "height": ${height_transfer}
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "at_height": ${height_mint_2}
        }
      ],
      "result_only": true,
      "var": "pass2_mint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_mint2.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_mint_2},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_mint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_mint2.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_transfer}
        }
      ],
      "result_only": true,
      "var": "pass1_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_transfer.state",
      "right": "dormant"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "at_height": ${height_transfer}
        }
      ],
      "result_only": true,
      "var": "pass2_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_transfer.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "block_height": ${height_transfer},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass1_energy_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_energy_transfer.state",
      "right": "dormant"
    },
    {
      "type": "assert_pass_energy_ge",
      "inscription_id": "${inscription_id_1}",
      "block_height": ${height_transfer},
      "expected_min_energy": 0,
      "mode": "at_or_before",
      "expected_state": "dormant"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_transfer},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_transfer.state",
      "right": "active"
    },
    {
      "type": "assert_pass_energy_delta",
      "inscription_id": "${inscription_id_2}",
      "from_height": ${height_mint_2},
      "to_height": ${height_transfer},
      "min_delta": 0,
      "mode": "at_or_before"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_active_balance_snapshot",
      "params": [
        {
          "block_height": ${height_transfer}
        }
      ],
      "result_only": true,
      "var": "balance_snapshot"
    },
    {
      "type": "assert_eq",
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
          "to_height": ${height_transfer},
          "page": 0,
          "page_size": 20
        }
      ],
      "result_only": true,
      "var": "invalid_page"
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

build_live_same_owner_multi_mint_scenario() {
  local scenario_file="$1"
  local inscription_id_1="$2"
  local inscription_id_2="$3"
  local height_mint_1="$4"
  local height_mint_2="$5"
  cat >"$scenario_file" <<EOF
{
  "name": "live-ord-same-owner-multi-mint-assert",
  "steps": [
    {
      "type": "wait_balance_history_synced",
      "height": ${height_mint_2}
    },
    {
      "type": "wait_usdb_synced",
      "height": ${height_mint_2}
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_mint_1}
        }
      ],
      "result_only": true,
      "var": "pass1_mint1"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_mint1.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "block_height": ${height_mint_1},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass1_energy_mint1"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_energy_mint1.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_mint_2}
        }
      ],
      "result_only": true,
      "var": "pass1_mint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_mint2.state",
      "right": "dormant"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "at_height": ${height_mint_2}
        }
      ],
      "result_only": true,
      "var": "pass2_mint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_mint2.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "block_height": ${height_mint_2},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass1_energy_mint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_energy_mint2.state",
      "right": "dormant"
    },
    {
      "type": "assert_pass_energy_delta",
      "inscription_id": "${inscription_id_1}",
      "from_height": ${height_mint_1},
      "to_height": ${height_mint_2},
      "min_delta": 0,
      "mode": "at_or_before"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_mint_2},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_mint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_mint2.state",
      "right": "active"
    },
    {
      "type": "assert_pass_energy_eq",
      "inscription_id": "${inscription_id_2}",
      "block_height": ${height_mint_2},
      "expected_energy": 0,
      "mode": "at_or_before",
      "expected_state": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_active_balance_snapshot",
      "params": [
        {
          "block_height": ${height_mint_2}
        }
      ],
      "result_only": true,
      "var": "balance_snapshot"
    },
    {
      "type": "assert_eq",
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
          "to_height": ${height_mint_2},
          "page": 0,
          "page_size": 20
        }
      ],
      "result_only": true,
      "var": "invalid_page"
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

build_live_duplicate_prev_inherit_scenario() {
  local scenario_file="$1"
  local inscription_id_1="$2"
  local inscription_id_2="$3"
  local inscription_id_3="$4"
  local height_transfer="$5"
  local height_remint_1="$6"
  local height_penalty_baseline="$7"
  local height_penalty="$8"
  local height_remint_2="$9"
  cat >"$scenario_file" <<EOF
{
  "name": "live-ord-duplicate-prev-inherit-assert",
  "steps": [
    {
      "type": "wait_balance_history_synced",
      "height": ${height_remint_2}
    },
    {
      "type": "wait_usdb_synced",
      "height": ${height_remint_2}
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_transfer}
        }
      ],
      "result_only": true,
      "var": "pass1_transfer"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_transfer.state",
      "right": "dormant"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_remint_1}
        }
      ],
      "result_only": true,
      "var": "pass1_remint1"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_remint1.state",
      "right": "consumed"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "at_height": ${height_remint_1}
        }
      ],
      "result_only": true,
      "var": "pass2_remint1"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_remint1.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_remint_1},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_remint1"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_remint1.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_penalty_baseline},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_before_penalty"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_before_penalty.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_penalty},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_penalty"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_penalty.state",
      "right": "active"
    },
    {
      "type": "assert_pass_energy_delta",
      "inscription_id": "${inscription_id_2}",
      "from_height": ${height_penalty_baseline},
      "to_height": ${height_penalty},
      "max_delta": -1,
      "mode": "at_or_before"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "at_height": ${height_remint_2}
        }
      ],
      "result_only": true,
      "var": "pass1_remint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_remint2.state",
      "right": "consumed"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "at_height": ${height_remint_2}
        }
      ],
      "result_only": true,
      "var": "pass2_remint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_remint2.state",
      "right": "dormant"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_snapshot",
      "params": [
        {
          "inscription_id": "${inscription_id_3}",
          "at_height": ${height_remint_2}
        }
      ],
      "result_only": true,
      "var": "pass3_remint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass3_remint2.state",
      "right": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_1}",
          "block_height": ${height_remint_2},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass1_energy_remint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass1_energy_remint2.state",
      "right": "consumed"
    },
    {
      "type": "assert_pass_energy_eq",
      "inscription_id": "${inscription_id_1}",
      "block_height": ${height_remint_2},
      "expected_energy": 0,
      "mode": "at_or_before",
      "expected_state": "consumed"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_2}",
          "block_height": ${height_remint_2},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass2_energy_remint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass2_energy_remint2.state",
      "right": "dormant"
    },
    {
      "type": "assert_pass_energy_delta",
      "inscription_id": "${inscription_id_2}",
      "from_height": ${height_penalty},
      "to_height": ${height_remint_2},
      "min_delta": 0,
      "mode": "at_or_before"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_pass_energy",
      "params": [
        {
          "inscription_id": "${inscription_id_3}",
          "block_height": ${height_remint_2},
          "mode": "at_or_before"
        }
      ],
      "result_only": true,
      "var": "pass3_energy_remint2"
    },
    {
      "type": "assert_eq",
      "left": "\$pass3_energy_remint2.state",
      "right": "active"
    },
    {
      "type": "assert_pass_energy_eq",
      "inscription_id": "${inscription_id_3}",
      "block_height": ${height_remint_2},
      "expected_energy": 0,
      "mode": "at_or_before",
      "expected_state": "active"
    },
    {
      "type": "rpc_call",
      "service": "usdb",
      "method": "get_active_balance_snapshot",
      "params": [
        {
          "block_height": ${height_remint_2}
        }
      ],
      "result_only": true,
      "var": "balance_snapshot"
    },
    {
      "type": "assert_eq",
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
          "to_height": ${height_remint_2},
          "page": 0,
          "page_size": 20
        }
      ],
      "result_only": true,
      "var": "invalid_page"
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
  trap 'on_error $? $LINENO "$BASH_COMMAND"' ERR
  trap on_exit EXIT

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
  run_ord --index-addresses --index-transactions server --address 127.0.0.1 --http --http-port "$ORD_SERVER_PORT" \
    >"${WORK_DIR}/ord-server.log" 2>&1 &
  ORD_SERVER_PID=$!
  wait_http_ready "ord-server" "http://127.0.0.1:${ORD_SERVER_PORT}/blockcount"
  wait_until_ord_server_synced_to_bitcoind

  log "Preparing ord wallets: ${ORD_WALLET_NAME}, ${ORD_WALLET_NAME_B}"
  run_ord_wallet_named "$ORD_WALLET_NAME" create >/dev/null 2>&1 || true
  run_ord_wallet_named "$ORD_WALLET_NAME_B" create >/dev/null 2>&1 || true

  local ord_receive_output_a ord_receive_address_a
  local ord_receive_output_b ord_receive_address_b
  ord_receive_output_a="$(run_ord_wallet_named "$ORD_WALLET_NAME" receive 2>&1 || true)"
  ord_receive_address_a="$(extract_bech32_address "$ord_receive_output_a")"
  if [[ -z "$ord_receive_address_a" ]]; then
    log "Failed to parse ord wallet A receive address from output: ${ord_receive_output_a}"
    exit 1
  fi
  ord_receive_output_b="$(run_ord_wallet_named "$ORD_WALLET_NAME_B" receive 2>&1 || true)"
  ord_receive_address_b="$(extract_bech32_address "$ord_receive_output_b")"
  if [[ -z "$ord_receive_address_b" ]]; then
    log "Failed to parse ord wallet B receive address from output: ${ord_receive_output_b}"
    exit 1
  fi

  log "Funding ord wallet A address: ${ord_receive_address_a}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    sendtoaddress "$ord_receive_address_a" "$FUND_ORD_AMOUNT_BTC" >/dev/null
  log "Funding ord wallet B address: ${ord_receive_address_b}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    sendtoaddress "$ord_receive_address_b" "$FUND_ORD_AMOUNT_BTC" >/dev/null
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    generatetoaddress "$FUND_CONFIRM_BLOCKS" "$miner_address" >/dev/null
  wait_until_ord_server_synced_to_bitcoind

  if [[ -z "$ORD_CONTENT_FILE" ]]; then
    ORD_CONTENT_FILE="$WORK_DIR/usdb_live_mint.json"
    if [[ "$LIVE_SCENARIO" == "invalid_mint" ]]; then
      cat >"$ORD_CONTENT_FILE" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x123","prev":[]}
EOF
    else
      cat >"$ORD_CONTENT_FILE" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF
    fi
  fi
  if [[ ! -f "$ORD_CONTENT_FILE" ]]; then
    log "ORD_CONTENT_FILE does not exist: $ORD_CONTENT_FILE"
    exit 1
  fi

  local first_mint_destination=""
  if [[ "$LIVE_SCENARIO" == "same_owner_multi_mint" ]]; then
    # Keep owner identity stable across repeated mints by pinning both inscriptions to the same destination address.
    first_mint_destination="$ord_receive_address_a"
  fi

  log "Inscribe first mint via ord CLI: wallet=${ORD_WALLET_NAME}, fee_rate=${ORD_FEE_RATE}, content_file=${ORD_CONTENT_FILE}, destination=${first_mint_destination:-<default>}"
  local inscribe_output_1 inscription_id_1
  if [[ -n "$first_mint_destination" ]]; then
    inscribe_output_1="$(run_ord_wallet_named "$ORD_WALLET_NAME" inscribe --fee-rate "$ORD_FEE_RATE" --destination "$first_mint_destination" --file "$ORD_CONTENT_FILE" 2>&1 || true)"
  else
    inscribe_output_1="$(run_ord_wallet_named "$ORD_WALLET_NAME" inscribe --fee-rate "$ORD_FEE_RATE" --file "$ORD_CONTENT_FILE" 2>&1 || true)"
  fi
  inscription_id_1="$(extract_inscription_id "$inscribe_output_1")"
  if [[ -z "$inscription_id_1" ]]; then
    log "Failed to parse first inscription id from ord output: ${inscribe_output_1}"
    exit 1
  fi
  log "First mint inscription_id=${inscription_id_1}"

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
    generatetoaddress "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address" >/dev/null
  wait_until_ord_server_synced_to_bitcoind
  local height_mint_1
  height_mint_1="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  log "Chain height after first mint confirmations: ${height_mint_1}"

  local target_height
  local scenario_file
  local scenario_summary
  scenario_summary="scenario=${LIVE_SCENARIO}, pass1=${inscription_id_1}"
  if [[ "$LIVE_SCENARIO" == "transfer_remint" ]]; then
    log "Transfer first inscription from wallet A to wallet B: inscription_id=${inscription_id_1}"
    local transfer_output transfer_txid
    transfer_output="$(run_ord_wallet_named "$ORD_WALLET_NAME" send --fee-rate "$ORD_FEE_RATE" "$ord_receive_address_b" "$inscription_id_1" 2>&1 || true)"
    transfer_txid="$(extract_txid "$transfer_output")"
    if [[ -z "$transfer_txid" ]]; then
      log "Failed to parse transfer txid from ord output: ${transfer_output}"
      exit 1
    fi
    log "Transfer txid=${transfer_txid}"
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$TRANSFER_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$inscription_id_1"
    local height_transfer_1
    height_transfer_1="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after transfer confirmations: ${height_transfer_1}"

    local remint_content_file
    remint_content_file="$WORK_DIR/usdb_live_remint.json"
    cat >"$remint_content_file" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x2222222222222222222222222222222222222222","prev":["${inscription_id_1}"]}
EOF

    log "Inscribe remint(prev) via ord CLI: wallet=${ORD_WALLET_NAME_B}, prev=${inscription_id_1}"
    local inscribe_output_2 inscription_id_2
    inscribe_output_2="$(run_ord_wallet_named "$ORD_WALLET_NAME_B" inscribe --fee-rate "$ORD_FEE_RATE" --file "$remint_content_file" 2>&1 || true)"
    inscription_id_2="$(extract_inscription_id "$inscribe_output_2")"
    if [[ -z "$inscription_id_2" ]]; then
      log "Failed to parse remint inscription id from ord output: ${inscribe_output_2}"
      exit 1
    fi
    log "Remint inscription_id=${inscription_id_2}"

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$REMINT_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    target_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after remint confirmations: ${target_height}"

    scenario_file="$WORK_DIR/live_ord_transfer_remint_assert.json"
    build_live_transfer_remint_scenario "$scenario_file" "$inscription_id_1" "$inscription_id_2" "$height_mint_1" "$height_transfer_1" "$target_height"
    scenario_summary="${scenario_summary}, pass2=${inscription_id_2}"
  elif [[ "$LIVE_SCENARIO" == "invalid_mint" ]]; then
    target_height="$height_mint_1"
    scenario_file="$WORK_DIR/live_ord_invalid_mint_assert.json"
    build_live_invalid_mint_scenario "$scenario_file" "$inscription_id_1" "$target_height"
    log "Invalid mint scenario selected: target_height=${target_height}, inscription_id=${inscription_id_1}"
  elif [[ "$LIVE_SCENARIO" == "passive_transfer" ]]; then
    local second_mint_content_file
    second_mint_content_file="$WORK_DIR/usdb_live_second_mint.json"
    cat >"$second_mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x3333333333333333333333333333333333333333","prev":[]}
EOF

    log "Inscribe second mint via ord CLI: wallet=${ORD_WALLET_NAME_B}, destination=${ord_receive_address_b}"
    local inscribe_output_2 inscription_id_2
    inscribe_output_2="$(run_ord_wallet_named "$ORD_WALLET_NAME_B" inscribe --fee-rate "$ORD_FEE_RATE" --destination "$ord_receive_address_b" --file "$second_mint_content_file" 2>&1 || true)"
    inscription_id_2="$(extract_inscription_id "$inscribe_output_2")"
    if [[ -z "$inscription_id_2" ]]; then
      log "Failed to parse second inscription id from ord output: ${inscribe_output_2}"
      exit 1
    fi
    log "Second mint inscription_id=${inscription_id_2}"

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    local height_mint_2
    height_mint_2="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after second mint confirmations: ${height_mint_2}"

    log "Transfer first inscription to wallet B receive address for passive transfer validation: inscription_id=${inscription_id_1}"
    local transfer_output transfer_txid
    transfer_output="$(run_ord_wallet_named "$ORD_WALLET_NAME" send --fee-rate "$ORD_FEE_RATE" "$ord_receive_address_b" "$inscription_id_1" 2>&1 || true)"
    transfer_txid="$(extract_txid "$transfer_output")"
    if [[ -z "$transfer_txid" ]]; then
      log "Failed to parse transfer txid from ord output: ${transfer_output}"
      exit 1
    fi
    log "Transfer txid=${transfer_txid}"
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$TRANSFER_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$inscription_id_1"
    target_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after passive transfer confirmations: ${target_height}"

    scenario_file="$WORK_DIR/live_ord_passive_transfer_assert.json"
    build_live_passive_transfer_scenario "$scenario_file" "$inscription_id_1" "$inscription_id_2" "$height_mint_2" "$target_height"
    scenario_summary="${scenario_summary}, pass2=${inscription_id_2}"
  elif [[ "$LIVE_SCENARIO" == "same_owner_multi_mint" ]]; then
    local second_mint_content_file
    second_mint_content_file="$WORK_DIR/usdb_live_second_mint_same_owner.json"
    cat >"$second_mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x5555555555555555555555555555555555555555","prev":[]}
EOF

    log "Inscribe second mint via ord CLI with same owner wallet: wallet=${ORD_WALLET_NAME}, destination=${ord_receive_address_a}"
    local inscribe_output_2 inscription_id_2
    inscribe_output_2="$(run_ord_wallet_named "$ORD_WALLET_NAME" inscribe --fee-rate "$ORD_FEE_RATE" --destination "$ord_receive_address_a" --file "$second_mint_content_file" 2>&1 || true)"
    inscription_id_2="$(extract_inscription_id "$inscribe_output_2")"
    if [[ -z "$inscription_id_2" ]]; then
      log "Failed to parse second same-owner inscription id from ord output: ${inscribe_output_2}"
      exit 1
    fi
    log "Second same-owner mint inscription_id=${inscription_id_2}"

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    target_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after same-owner second mint confirmations: ${target_height}"

    scenario_file="$WORK_DIR/live_ord_same_owner_multi_mint_assert.json"
    build_live_same_owner_multi_mint_scenario "$scenario_file" "$inscription_id_1" "$inscription_id_2" "$height_mint_1" "$target_height"
    scenario_summary="${scenario_summary}, pass2=${inscription_id_2}"
  elif [[ "$LIVE_SCENARIO" == "duplicate_prev_inherit" ]]; then
    log "Transfer first inscription from wallet A to wallet B for inherit-precondition: inscription_id=${inscription_id_1}"
    local transfer_output transfer_txid
    transfer_output="$(run_ord_wallet_named "$ORD_WALLET_NAME" send --fee-rate "$ORD_FEE_RATE" "$ord_receive_address_b" "$inscription_id_1" 2>&1 || true)"
    transfer_txid="$(extract_txid "$transfer_output")"
    if [[ -z "$transfer_txid" ]]; then
      log "Failed to parse transfer txid from ord output: ${transfer_output}"
      exit 1
    fi
    log "Transfer txid=${transfer_txid}"
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$TRANSFER_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$inscription_id_1"
    local height_transfer_1
    height_transfer_1="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after transfer confirmations: ${height_transfer_1}"

    local remint_content_file_1
    remint_content_file_1="$WORK_DIR/usdb_live_remint_first.json"
    cat >"$remint_content_file_1" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x2222222222222222222222222222222222222222","prev":["${inscription_id_1}"]}
EOF
    log "Inscribe first remint(prev) via ord CLI: wallet=${ORD_WALLET_NAME_B}, prev=${inscription_id_1}"
    local inscribe_output_2 inscription_id_2
    inscribe_output_2="$(run_ord_wallet_named "$ORD_WALLET_NAME_B" inscribe --fee-rate "$ORD_FEE_RATE" --destination "$ord_receive_address_b" --file "$remint_content_file_1" 2>&1 || true)"
    inscription_id_2="$(extract_inscription_id "$inscribe_output_2")"
    if [[ -z "$inscription_id_2" ]]; then
      log "Failed to parse first remint inscription id from ord output: ${inscribe_output_2}"
      exit 1
    fi
    log "First remint inscription_id=${inscription_id_2}"

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$REMINT_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    local height_remint_1
    height_remint_1="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after first remint confirmations: ${height_remint_1}"

    log "Funding owner address before penalty spend: address=${ord_receive_address_b}, amount_btc=${PENALTY_FUND_AMOUNT_BTC}"
    local penalty_fund_txid
    penalty_fund_txid="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      sendtoaddress "$ord_receive_address_b" "$PENALTY_FUND_AMOUNT_BTC")"
    if [[ -z "$penalty_fund_txid" ]]; then
      log "Failed to create penalty baseline funding transaction"
      exit 1
    fi
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$PENALTY_FUND_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    local height_penalty_baseline
    height_penalty_baseline="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after penalty baseline funding confirmations: ${height_penalty_baseline}"

    local penalty_fund_vout
    penalty_fund_vout="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      getrawtransaction "$penalty_fund_txid" 2 | python3 - "$ord_receive_address_b" <<'PY'
import json
import sys

target_address = sys.argv[1]
payload = json.load(sys.stdin)
for vout in payload.get("vout", []):
    output_index = vout.get("n")
    if output_index is None:
        continue
    script_pub_key = vout.get("scriptPubKey", {})
    addresses = []
    address = script_pub_key.get("address")
    if isinstance(address, str):
        addresses.append(address)
    extra_addresses = script_pub_key.get("addresses")
    if isinstance(extra_addresses, list):
        addresses.extend([item for item in extra_addresses if isinstance(item, str)])
    if target_address in addresses:
        print(output_index)
        raise SystemExit(0)

print("")
PY
)"
    if [[ -z "$penalty_fund_vout" ]]; then
      log "Failed to locate owner output in penalty funding tx: txid=${penalty_fund_txid}, owner_address=${ord_receive_address_b}"
      exit 1
    fi

    log "Spending funded owner UTXO to trigger negative owner delta: txid=${penalty_fund_txid}, vout=${penalty_fund_vout}, amount_btc=${PENALTY_SPEND_AMOUNT_BTC}"
    local penalty_spend_raw
    penalty_spend_raw="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$ORD_WALLET_NAME_B" \
      createrawtransaction "[{\"txid\":\"${penalty_fund_txid}\",\"vout\":${penalty_fund_vout}}]" "{\"${miner_address}\":${PENALTY_SPEND_AMOUNT_BTC}}")"
    if [[ -z "$penalty_spend_raw" ]]; then
      log "Failed to create penalty spend raw transaction"
      exit 1
    fi

    local penalty_spend_signed
    penalty_spend_signed="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$ORD_WALLET_NAME_B" \
      signrawtransactionwithwallet "$penalty_spend_raw")"
    local penalty_spend_hex
    penalty_spend_hex="$(echo "$penalty_spend_signed" | python3 - <<'PY'
import json
import sys

payload = json.load(sys.stdin)
print(payload.get("hex", ""))
PY
)"
    local penalty_spend_complete
    penalty_spend_complete="$(echo "$penalty_spend_signed" | python3 - <<'PY'
import json
import sys

payload = json.load(sys.stdin)
print("true" if payload.get("complete") else "false")
PY
)"
    if [[ "$penalty_spend_complete" != "true" || -z "$penalty_spend_hex" ]]; then
      log "Failed to sign penalty spend transaction: payload=${penalty_spend_signed}"
      exit 1
    fi

    local penalty_spend_txid
    penalty_spend_txid="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      sendrawtransaction "$penalty_spend_hex")"
    if [[ -z "$penalty_spend_txid" ]]; then
      log "Failed to broadcast penalty spend transaction"
      exit 1
    fi
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$PENALTY_SPEND_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    local height_penalty
    height_penalty="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after penalty spend confirmations: ${height_penalty}"

    local remint_content_file_2
    remint_content_file_2="$WORK_DIR/usdb_live_remint_second.json"
    cat >"$remint_content_file_2" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x4444444444444444444444444444444444444444","prev":["${inscription_id_1}"]}
EOF
    log "Inscribe duplicate remint(prev) via ord CLI: wallet=${ORD_WALLET_NAME_B}, prev=${inscription_id_1}"
    local inscribe_output_3 inscription_id_3
    inscribe_output_3="$(run_ord_wallet_named "$ORD_WALLET_NAME_B" inscribe --fee-rate "$ORD_FEE_RATE" --destination "$ord_receive_address_b" --file "$remint_content_file_2" 2>&1 || true)"
    inscription_id_3="$(extract_inscription_id "$inscribe_output_3")"
    if [[ -z "$inscription_id_3" ]]; then
      log "Failed to parse duplicate remint inscription id from ord output: ${inscribe_output_3}"
      exit 1
    fi
    log "Duplicate remint inscription_id=${inscription_id_3}"

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$MINER_WALLET_NAME" \
      generatetoaddress "$REMINT_CONFIRM_BLOCKS" "$miner_address" >/dev/null
    wait_until_ord_server_synced_to_bitcoind
    target_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    log "Chain height after duplicate remint confirmations: ${target_height}"

    scenario_file="$WORK_DIR/live_ord_duplicate_prev_inherit_assert.json"
    build_live_duplicate_prev_inherit_scenario "$scenario_file" "$inscription_id_1" "$inscription_id_2" "$inscription_id_3" "$height_transfer_1" "$height_remint_1" "$height_penalty_baseline" "$height_penalty" "$target_height"
    scenario_summary="${scenario_summary}, pass2=${inscription_id_2}, pass3=${inscription_id_3}"
  else
    log "Unsupported LIVE_SCENARIO=${LIVE_SCENARIO}, expected transfer_remint/invalid_mint/passive_transfer/same_owner_multi_mint/duplicate_prev_inherit"
    exit 1
  fi

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
  wait_until_usdb_consensus_ready "$target_height"

  SCENARIO_FILE_PATH="$scenario_file"

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

  log "Live ord e2e succeeded: ${scenario_summary}, target_height=${target_height}"
  log "Logs: ${WORK_DIR}/balance-history.log, ${WORK_DIR}/usdb-indexer.log"
}

main "$@"
