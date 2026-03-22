#!/usr/bin/env bash

if [[ -n "${USDB_INDEXER_REGTEST_REORG_LIB_SH:-}" ]]; then
  return 0
fi
USDB_INDEXER_REGTEST_REORG_LIB_SH=1

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)}"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29332}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29333}"
BH_RPC_PORT="${BH_RPC_PORT:-29310}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29320}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29330}"
WALLET_NAME="${WALLET_NAME:-usdbreorg}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-reorg-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-reorg-b}"
USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES="${USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES:-}"
USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES="${USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES:-}"
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
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
CURL_CONNECT_TIMEOUT_SEC="${CURL_CONNECT_TIMEOUT_SEC:-2}"
CURL_MAX_TIME_SEC="${CURL_MAX_TIME_SEC:-8}"
REGTEST_DIAG_TAIL_LINES="${REGTEST_DIAG_TAIL_LINES:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
INSCRIPTION_SOURCE="${INSCRIPTION_SOURCE:-bitcoind}"
INSCRIPTION_FIXTURE_FILE="${INSCRIPTION_FIXTURE_FILE:-}"

BITCOIND_PID="${BITCOIND_PID:-}"
BALANCE_HISTORY_PID="${BALANCE_HISTORY_PID:-}"
USDB_INDEXER_PID="${USDB_INDEXER_PID:-}"
ORD_SERVER_PID="${ORD_SERVER_PID:-}"
BITCOIND_BIN="${BITCOIND_BIN:-}"
BITCOIN_CLI_BIN="${BITCOIN_CLI_BIN:-}"
REGTEST_DIAGNOSTICS_PRINTED="${REGTEST_DIAGNOSTICS_PRINTED:-0}"

regtest_log() {
  echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} $*"
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
  mkdir -p "$WORK_DIR" "$BITCOIN_DIR" "$ORD_DATA_DIR" "$BALANCE_HISTORY_ROOT" "$USDB_INDEXER_ROOT"
  regtest_log "Workspace directory: $WORK_DIR"
}

regtest_json_extract_python() {
  local script="$1"
  python3 -c "$script"
}

regtest_json_quote() {
  python3 - "$1" <<'PY'
import json
import sys

print(json.dumps(sys.argv[1]))
PY
}

regtest_json_expr() {
  local response="$1"
  local expression="$2"
  printf '%s' "$response" | python3 -c "import json,sys; data=json.load(sys.stdin); print(${expression})"
}

regtest_assert_json_expr() {
  local response="$1"
  local expression="$2"
  local expected="$3"
  local actual

  actual="$(regtest_json_expr "$response" "$expression")"
  regtest_log "RPC assertion: expr=${expression}, expected=${expected}, actual=${actual}"
  if [[ "$actual" != "$expected" ]]; then
    regtest_log "RPC assertion failed. response=${response}"
    exit 1
  fi
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

regtest_rpc_call_usdb_indexer() {
  local method="$1"
  local params="${2:-[]}"
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    -X POST "http://127.0.0.1:${USDB_RPC_PORT}" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

regtest_get_usdb_state_ref_response() {
  local block_height="$1"
  regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "[{\"block_height\":${block_height}}]"
}

regtest_build_consensus_context_json() {
  local requested_height="$1"
  local snapshot_id="$2"
  local stable_block_hash="$3"
  local local_state_commit="$4"
  local system_state_id="$5"

  python3 - "$requested_height" "$snapshot_id" "$stable_block_hash" "$local_state_commit" "$system_state_id" <<'PY'
import json
import sys

requested_height = int(sys.argv[1])
snapshot_id = sys.argv[2]
stable_block_hash = sys.argv[3]
local_state_commit = sys.argv[4]
system_state_id = sys.argv[5]

print(json.dumps({
    "requested_height": requested_height,
    "expected_state": {
        "snapshot_id": snapshot_id,
        "stable_block_hash": stable_block_hash,
        "local_state_commit": local_state_commit,
        "system_state_id": system_state_id,
    },
}))
PY
}

regtest_assert_usdb_consensus_error() {
  local response="$1"
  local expected_code="$2"
  local expected_message="$3"

  regtest_assert_json_expr "$response" "((data.get('error') or {}).get('code'))" "$expected_code"
  regtest_assert_json_expr "$response" "((data.get('error') or {}).get('message'))" "$expected_message"
}

regtest_rpc_call_usdb_json_retry() {
  local method="$1"
  local params="${2:-[]}"
  local attempts="${3:-20}"
  local sleep_sec="${4:-0.2}"
  local resp=""

  for _ in $(seq 1 "$attempts"); do
    resp="$(regtest_rpc_call_usdb_indexer "$method" "$params" || true)"
    if [[ -n "$resp" ]] && printf '%s' "$resp" | python3 -c 'import json,sys; json.load(sys.stdin)' >/dev/null 2>&1; then
      echo "$resp"
      return 0
    fi
    sleep "$sleep_sec"
  done

  echo "$resp"
  return 1
}

regtest_must_rpc_call_usdb_json() {
  local method="$1"
  local params="${2:-[]}"
  local resp

  if ! resp="$(regtest_rpc_call_usdb_json_retry "$method" "$params" 20 0.2)"; then
    regtest_log "Failed to get valid JSON response from usdb-indexer: method=${method}, params=${params}, last_response=${resp:-<empty>}"
    return 1
  fi

  if [[ -z "$resp" ]]; then
    regtest_log "Received empty JSON response from usdb-indexer: method=${method}, params=${params}"
    return 1
  fi

  echo "$resp"
}

regtest_wait_rpc_ready() {
  local service_name="$1"
  local url="$2"
  local method="$3"
  local params="$4"

  regtest_log "Waiting for ${service_name} RPC readiness"
  for _ in $(seq 1 120); do
    if curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "$url" -H 'content-type: application/json' \
      --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done

  regtest_log "${service_name} RPC is not ready at ${url}"
  exit 1
}

regtest_wait_http_ready() {
  local service_name="$1"
  local url="$2"

  regtest_log "Waiting for ${service_name} HTTP readiness"
  for _ in $(seq 1 120); do
    if curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      "$url" >/dev/null 2>&1; then
      return 0
    fi
    sleep 0.5
  done

  regtest_log "${service_name} HTTP is not ready at ${url}"
  exit 1
}

regtest_wait_balance_history_rpc_ready() {
  regtest_wait_rpc_ready "balance-history" "http://127.0.0.1:${BH_RPC_PORT}" "get_network_type" "[]"
}

regtest_wait_usdb_rpc_ready() {
  regtest_wait_rpc_ready "usdb-indexer" "http://127.0.0.1:${USDB_RPC_PORT}" "get_network_type" "[]"
}

regtest_wait_balance_history_consensus_ready() {
  regtest_log "Waiting for balance-history consensus readiness"

  local start_ts now readiness_resp consensus_ready
  start_ts="$(date +%s)"
  while true; do
    readiness_resp="$(regtest_rpc_call_balance_history "get_readiness" "[]")"
    consensus_ready="$(echo "$readiness_resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result") or {}; print("1" if r.get("consensus_ready") else "0")')"
    if [[ "$consensus_ready" == "1" ]]; then
      regtest_log "balance-history is consensus ready"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for balance-history consensus readiness. last_response=${readiness_resp}"
      exit 1
    fi

    sleep 0.5
  done
}

regtest_wait_usdb_consensus_ready() {
  regtest_log "Waiting for usdb-indexer consensus readiness"

  local start_ts now readiness_resp consensus_ready
  start_ts="$(date +%s)"
  while true; do
    readiness_resp="$(regtest_rpc_call_usdb_indexer "get_readiness" "[]")"
    consensus_ready="$(echo "$readiness_resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result") or {}; print("1" if r.get("consensus_ready") else "0")')"
    if [[ "$consensus_ready" == "1" ]]; then
      regtest_log "usdb-indexer is consensus ready"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for usdb-indexer consensus readiness. last_response=${readiness_resp}"
      exit 1
    fi

    sleep 0.5
  done
}

regtest_wait_usdb_state_ref_available() {
  local target_height="$1"
  local start_ts now resp error_code

  regtest_log "Waiting until usdb-indexer historical state ref is available at height ${target_height}"
  start_ts="$(date +%s)"
  while true; do
    resp="$(regtest_get_usdb_state_ref_response "$target_height")"
    error_code="$(regtest_json_expr "$resp" "((data.get('error') or {}).get('code'))")"
    if [[ "$error_code" == "None" ]]; then
      regtest_log "usdb-indexer historical state ref is available at height ${target_height}"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for usdb-indexer historical state ref at height ${target_height}. last_response=${resp}"
      exit 1
    fi

    sleep 0.5
  done
}

regtest_wait_until_rpc_expr_eq() {
  local label="$1"
  local rpc_func="$2"
  local method="$3"
  local params="$4"
  local expression="$5"
  local expected="$6"

  local start_ts now resp actual
  regtest_log "Waiting until ${label} equals ${expected}"
  start_ts="$(date +%s)"
  while true; do
    resp="$("${rpc_func}" "$method" "$params")"
    actual="$(regtest_json_expr "$resp" "$expression")"
    if [[ "$actual" == "$expected" ]]; then
      regtest_log "${label} converged to ${expected}"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for ${label}. last_response=${resp}"
      exit 1
    fi
    sleep 1
  done
}

regtest_wait_until_balance_history_synced_ge() {
  local target_height="$1"
  local start_ts now resp synced
  regtest_log "Waiting until balance-history synced height >= ${target_height}"
  start_ts="$(date +%s)"
  while true; do
    resp="$(regtest_rpc_call_balance_history "get_block_height" "[]")"
    synced="$(echo "$resp" | regtest_parse_json_number_result)"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      regtest_log "balance-history synced height=${synced}"
      return 0
    fi
    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for balance-history synced height >= ${target_height}. last_response=${resp}"
      exit 1
    fi
    sleep 1
  done
}

regtest_wait_until_balance_history_synced_eq() {
  local target_height="$1"
  regtest_wait_until_rpc_expr_eq \
    "balance-history synced height" \
    regtest_rpc_call_balance_history \
    "get_block_height" \
    "[]" \
    "data.get('result', 0)" \
    "$target_height"
}

regtest_wait_until_usdb_synced_ge() {
  local target_height="$1"
  local start_ts now resp synced
  regtest_log "Waiting until usdb-indexer synced height >= ${target_height}"
  start_ts="$(date +%s)"
  while true; do
    resp="$(regtest_rpc_call_usdb_indexer "get_synced_block_height" "[]")"
    synced="$(echo "$resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); print(0 if d.get("result") is None else d.get("result"))')"
    synced="${synced:-0}"
    if [[ "$synced" -ge "$target_height" ]]; then
      regtest_log "usdb-indexer synced height=${synced}"
      return 0
    fi
    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for usdb-indexer synced height >= ${target_height}. last_response=${resp}"
      exit 1
    fi
    sleep 1
  done
}

regtest_wait_until_usdb_synced_eq() {
  local target_height="$1"
  regtest_wait_until_rpc_expr_eq \
    "usdb-indexer synced height" \
    regtest_rpc_call_usdb_indexer \
    "get_synced_block_height" \
    "[]" \
    "(0 if data.get('result') is None else data.get('result'))" \
    "$target_height"
}

regtest_get_bitcoin_block_hash() {
  local block_height="$1"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$block_height"
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

regtest_create_usdb_indexer_config() {
  mkdir -p "$USDB_INDEXER_ROOT"
  local fixture_json="null"
  if [[ -n "$INSCRIPTION_FIXTURE_FILE" ]]; then
    fixture_json="$(regtest_json_quote "$INSCRIPTION_FIXTURE_FILE")"
  fi

  cat >"${USDB_INDEXER_ROOT}/config.json" <<EOF
{
  "isolate": null,
  "bitcoin": {
    "network": "regtest",
    "data_dir": "${BITCOIN_DIR}/regtest",
    "rpc_url": "http://127.0.0.1:${BTC_RPC_PORT}"
  },
  "ordinals": {
    "rpc_url": "http://127.0.0.1:${ORD_RPC_PORT}"
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
    "inscription_source": "${INSCRIPTION_SOURCE}",
    "inscription_fixture_file": ${fixture_json},
    "inscription_source_shadow_compare": false,
    "inscription_source_shadow_fail_fast": false,
    "rpc_server_port": ${USDB_RPC_PORT},
    "rpc_server_enabled": true,
    "monitor_ord_enabled": false
  }
}
EOF
}

regtest_update_usdb_genesis_block_height() {
  local new_height="$1"
  local config_path="${USDB_INDEXER_ROOT}/config.json"

  python3 - "$config_path" "$new_height" <<'PY'
import json
import pathlib
import sys

config_path = pathlib.Path(sys.argv[1])
new_height = int(sys.argv[2])
payload = json.loads(config_path.read_text())
payload.setdefault("usdb", {})["genesis_block_height"] = new_height
config_path.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

regtest_start_bitcoind() {
  regtest_log "Starting bitcoind regtest on rpcport=${BTC_RPC_PORT}, p2pport=${BTC_P2P_PORT}, bin=${BITCOIND_BIN}"
  "$BITCOIND_BIN" \
    -regtest \
    -server=1 \
    -txindex=1 \
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
    regtest_log "Failed to create/load wallet ${WALLET_NAME}"
    exit 1
  fi
}

regtest_get_new_address() {
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" getnewaddress
}

regtest_detect_ord_chain_on_port() {
  local status_html
  status_html="$(curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    "http://127.0.0.1:${ORD_RPC_PORT}/status" 2>/dev/null || true)"
  if [[ -z "$status_html" ]]; then
    echo ""
    return 0
  fi

  python3 - "$status_html" <<'PY'
import re
import sys

html = sys.argv[1]
match = re.search(r"<dt>\s*chain\s*</dt>\s*<dd>\s*([^<\s]+)\s*</dd>", html, re.IGNORECASE)
print(match.group(1).strip().lower() if match else "")
PY
}

regtest_assert_ord_server_port_available() {
  local chain
  chain="$(regtest_detect_ord_chain_on_port)"
  if [[ -z "$chain" ]]; then
    return 0
  fi

  if [[ "$chain" != "regtest" ]]; then
    regtest_log "Detected existing ord server on port ${ORD_RPC_PORT} with chain=${chain}. Please stop that service or change ORD_RPC_PORT."
    exit 1
  fi

  regtest_log "Detected existing regtest ord server on port ${ORD_RPC_PORT}. Please use an unused ORD_RPC_PORT to avoid shared state contamination."
  exit 1
}

regtest_run_ord() {
  "$ORD_BIN" \
    --regtest \
    --bitcoin-rpc-url "http://127.0.0.1:${BTC_RPC_PORT}" \
    --cookie-file "${BITCOIN_DIR}/regtest/.cookie" \
    --bitcoin-data-dir "$BITCOIN_DIR" \
    --data-dir "$ORD_DATA_DIR" \
    "$@"
}

regtest_run_ord_wallet_named() {
  local wallet_name="$1"
  shift
  regtest_run_ord wallet \
    --no-sync \
    --server-url "http://127.0.0.1:${ORD_RPC_PORT}" \
    --name "$wallet_name" \
    "$@"
}

regtest_extract_bech32_address() {
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

regtest_extract_inscription_id() {
  local raw="$1"
  python3 - "$raw" <<'PY'
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

    keys = [payload.get("inscription"), payload.get("inscription_id"), payload.get("id")]
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
}

regtest_extract_txid() {
  local raw="$1"
  python3 - "$raw" <<'PY'
import re
import sys

raw = sys.argv[1]
match = re.search(r"\b([0-9a-f]{64})\b", raw)
print(match.group(1) if match else "")
PY
}

# Resolve the output index in a transaction that pays to the requested address.
regtest_get_tx_vout_for_address() {
  local txid="$1"
  local address="$2"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    getrawtransaction "$txid" true | python3 -c 'import json, sys
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
        break
else:
    print("")' "$address"
}

regtest_start_ord_server() {
  regtest_log "Starting ord server (data_dir=${ORD_DATA_DIR}, http=${ORD_RPC_PORT})"
  regtest_run_ord --index-addresses --index-transactions server \
    --address 127.0.0.1 \
    --http \
    --http-port "$ORD_RPC_PORT" \
    >"${ORD_SERVER_LOG_FILE}" 2>&1 &
  ORD_SERVER_PID=$!
  regtest_wait_http_ready "ord-server" "http://127.0.0.1:${ORD_RPC_PORT}/blockcount"
}

regtest_get_ord_server_block_height() {
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    "http://127.0.0.1:${ORD_RPC_PORT}/blockcount" | tr -d '\n\r '
}

regtest_wait_until_ord_server_synced_to_bitcoind() {
  local start_ts now ord_height btc_height
  regtest_log "Waiting until ord server catches up to bitcoind"
  start_ts="$(date +%s)"
  while true; do
    btc_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount 2>/dev/null || echo 0)"
    ord_height="$(regtest_get_ord_server_block_height 2>/dev/null || echo 0)"
    if [[ "$ord_height" =~ ^[0-9]+$ ]] && [[ "$btc_height" =~ ^[0-9]+$ ]] && [[ "$ord_height" -ge "$btc_height" ]]; then
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "ord server sync timeout: ord_height=${ord_height:-unknown}, btc_height=${btc_height:-unknown}"
      exit 1
    fi
    sleep 1
  done
}

regtest_prepare_ord_wallets() {
  regtest_log "Preparing ord wallets: ${ORD_WALLET_NAME}, ${ORD_WALLET_NAME_B}"
  regtest_run_ord_wallet_named "$ORD_WALLET_NAME" create >/dev/null 2>&1 || true
  regtest_run_ord_wallet_named "$ORD_WALLET_NAME_B" create >/dev/null 2>&1 || true
}

regtest_get_ord_wallet_receive_address() {
  local wallet_name="$1"
  local output address

  output="$(regtest_run_ord_wallet_named "$wallet_name" receive 2>&1 || true)"
  address="$(regtest_extract_bech32_address "$output")"
  if [[ -z "$address" ]]; then
    regtest_log "Failed to parse ord wallet receive address: wallet=${wallet_name}, output=${output}"
    exit 1
  fi

  echo "$address"
}

regtest_fund_address() {
  local address="$1"
  local amount_btc="$2"
  regtest_log "Funding address=${address}, amount_btc=${amount_btc}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    sendtoaddress "$address" "$amount_btc" >/dev/null
}

regtest_wait_until_ord_wallet_has_inscription() {
  local wallet_name="$1"
  local inscription_id="$2"
  local start_ts now resp
  regtest_log "Waiting until ord wallet=${wallet_name} contains inscription_id=${inscription_id}"
  start_ts="$(date +%s)"
  while true; do
    resp="$(regtest_run_ord_wallet_named "$wallet_name" inscriptions 2>/dev/null || true)"
    if [[ "$resp" == *"$inscription_id"* ]]; then
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "ord wallet sync timeout: wallet=${wallet_name}, inscription_id=${inscription_id}, last_response=${resp}"
      exit 1
    fi
    sleep 1
  done
}

regtest_ord_inscribe_file() {
  local wallet_name="$1"
  local file_path="$2"
  local destination="${3:-}"
  local output inscription_id

  echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} Inscribe file via ord: wallet=${wallet_name}, file=${file_path}, destination=${destination:-<default>}" >&2
  if [[ -n "$destination" ]]; then
    output="$(regtest_run_ord_wallet_named "$wallet_name" inscribe --fee-rate "$ORD_FEE_RATE" --destination "$destination" --file "$file_path" 2>&1 || true)"
  else
    output="$(regtest_run_ord_wallet_named "$wallet_name" inscribe --fee-rate "$ORD_FEE_RATE" --file "$file_path" 2>&1 || true)"
  fi
  inscription_id="$(regtest_extract_inscription_id "$output")"
  if [[ -z "$inscription_id" ]]; then
    regtest_log "Failed to parse inscription id from ord output: ${output}"
    exit 1
  fi

  echo "$inscription_id"
}

regtest_ord_send_inscription() {
  local wallet_name="$1"
  local destination="$2"
  local inscription_id="$3"
  local output txid

  echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} Transfer inscription via ord: wallet=${wallet_name}, inscription_id=${inscription_id}, destination=${destination}" >&2
  output="$(regtest_run_ord_wallet_named "$wallet_name" send --fee-rate "$ORD_FEE_RATE" "$destination" "$inscription_id" 2>&1 || true)"
  txid="$(regtest_extract_txid "$output")"
  if [[ -z "$txid" ]]; then
    regtest_log "Failed to parse transfer txid from ord output: ${output}"
    exit 1
  fi

  echo "$txid"
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

regtest_start_usdb_indexer() {
  regtest_log "Starting usdb-indexer service (root=${USDB_INDEXER_ROOT}, rpc=${USDB_RPC_PORT})"
  local -a env_args=()
  if [[ -n "$USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES" ]]; then
    env_args+=(
      "USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES=${USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES}"
    )
  fi
  if [[ -n "$USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES" ]]; then
    env_args+=(
      "USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES=${USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES}"
    )
  fi
  (
    cd "$REPO_ROOT"
    env "${env_args[@]}" cargo run --manifest-path src/btc/Cargo.toml -p usdb-indexer -- \
      --root-dir "$USDB_INDEXER_ROOT" \
      --skip-process-lock
  ) >"${USDB_INDEXER_LOG_FILE}" 2>&1 &
  USDB_INDEXER_PID=$!
}

regtest_stop_process() {
  local pid="$1"
  if [[ -z "$pid" ]]; then
    return 0
  fi
  if ! kill -0 "$pid" 2>/dev/null; then
    wait "$pid" >/dev/null 2>&1 || true
    return 0
  fi

  kill "$pid" >/dev/null 2>&1 || true
  for _ in $(seq 1 30); do
    if [[ "$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ')" == Z* ]]; then
      break
    fi
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

regtest_stop_balance_history() {
  if [[ -n "$BALANCE_HISTORY_PID" ]] && kill -0 "$BALANCE_HISTORY_PID" 2>/dev/null; then
    regtest_log "Stopping balance-history process pid=${BALANCE_HISTORY_PID}"
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${BH_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1 || true
    regtest_stop_process "$BALANCE_HISTORY_PID"
  fi
  BALANCE_HISTORY_PID=""
}

regtest_stop_usdb_indexer() {
  if [[ -n "$USDB_INDEXER_PID" ]] && kill -0 "$USDB_INDEXER_PID" 2>/dev/null; then
    regtest_log "Stopping usdb-indexer process pid=${USDB_INDEXER_PID}"
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${USDB_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1 || true
    regtest_stop_process "$USDB_INDEXER_PID"
  fi
  USDB_INDEXER_PID=""
}

regtest_stop_ord_server() {
  if [[ -n "$ORD_SERVER_PID" ]]; then
    regtest_log "Stopping ord server process pid=${ORD_SERVER_PID}"
    regtest_stop_process "$ORD_SERVER_PID"
  fi
  ORD_SERVER_PID=""
}

regtest_restart_balance_history() {
  regtest_stop_balance_history
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_balance_history_consensus_ready
}

regtest_restart_usdb_indexer() {
  regtest_stop_usdb_indexer
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_usdb_consensus_ready
}

regtest_stop_bitcoind() {
  if [[ -n "$BITCOIN_CLI_BIN" ]] && [[ -x "$BITCOIN_CLI_BIN" ]]; then
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" stop >/dev/null 2>&1 || true
  fi
  if [[ -n "$BITCOIND_PID" ]]; then
    regtest_stop_process "$BITCOIND_PID"
  fi
  BITCOIND_PID=""
}

regtest_wait_until_balance_history_block_commit_hash() {
  local block_height="$1"
  local expected_hash="$2"
  regtest_wait_until_rpc_expr_eq \
    "balance-history block commit hash at height ${block_height}" \
    regtest_rpc_call_balance_history \
    "get_block_commit" \
    "[${block_height}]" \
    "((data.get('result') or {}).get('btc_block_hash', ''))" \
    "$expected_hash"
}

regtest_wait_until_file_contains() {
  local label="$1"
  local file_path="$2"
  local needle="$3"
  local start_ts now

  regtest_log "Waiting until ${label} contains: ${needle}"
  start_ts="$(date +%s)"
  while true; do
    if [[ -f "$file_path" ]] && grep -Fq "$needle" "$file_path"; then
      regtest_log "${label} contains expected text"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for ${label} to contain expected text. file=${file_path}, needle=${needle}"
      exit 1
    fi
    sleep 1
  done
}

regtest_usdb_miner_pass_db_path() {
  local preferred_path="${USDB_INDEXER_ROOT}/data/miner_pass.db"
  local legacy_path="${USDB_INDEXER_ROOT}/miner_pass.db"

  if [[ -f "$preferred_path" ]]; then
    echo "$preferred_path"
    return 0
  fi

  if [[ -f "$legacy_path" ]]; then
    echo "$legacy_path"
    return 0
  fi

  regtest_log "usdb-indexer miner_pass.db not found under ${USDB_INDEXER_ROOT}"
  exit 1
}

regtest_usdb_db_scalar() {
  local sql="$1"
  local db_path
  db_path="$(regtest_usdb_miner_pass_db_path)"

  python3 - "$db_path" "$sql" <<'PY'
import sqlite3
import sys

db_path = sys.argv[1]
sql = sys.argv[2]
conn = sqlite3.connect(db_path)
try:
    row = conn.execute(sql).fetchone()
finally:
    conn.close()

if row is None or row[0] is None:
    print("")
else:
    print(row[0])
PY
}

regtest_usdb_db_exec() {
  local sql="$1"
  local db_path
  db_path="$(regtest_usdb_miner_pass_db_path)"

  python3 - "$db_path" "$sql" <<'PY'
import sqlite3
import sys

db_path = sys.argv[1]
sql = sys.argv[2]
conn = sqlite3.connect(db_path)
try:
    conn.executescript(sql)
    conn.commit()
finally:
    conn.close()
PY
}

regtest_assert_usdb_db_scalar() {
  local sql="$1"
  local expected="$2"
  local label="$3"
  local actual

  actual="$(regtest_usdb_db_scalar "$sql")"
  regtest_log "SQLite assertion: label=${label}, expected=${expected}, actual=${actual}, sql=${sql}"
  if [[ "$actual" != "$expected" ]]; then
    regtest_log "SQLite assertion failed: label=${label}"
    exit 1
  fi
}

regtest_wait_until_usdb_db_scalar_eq() {
  local sql="$1"
  local expected="$2"
  local label="$3"
  local start_ts now actual

  regtest_log "Waiting until SQLite scalar equals expected: label=${label}, expected=${expected}, sql=${sql}"
  start_ts="$(date +%s)"
  while true; do
    actual="$(regtest_usdb_db_scalar "$sql")"
    if [[ "$actual" == "$expected" ]]; then
      regtest_log "SQLite scalar converged: label=${label}, actual=${actual}"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for SQLite scalar: label=${label}, expected=${expected}, actual=${actual}, sql=${sql}"
      exit 1
    fi
    sleep 1
  done
}

regtest_assert_usdb_pass_snapshot_state() {
  local inscription_id="$1"
  local block_height="$2"
  local expected_state="$3"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "[{\"inscription_id\":\"${inscription_id}\",\"at_height\":${block_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "data.get('result') is not None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('inscription_id')" "$inscription_id"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('resolved_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('state')" "$expected_state"
}

regtest_assert_usdb_pass_snapshot_missing() {
  local inscription_id="$1"
  local block_height="$2"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "[{\"inscription_id\":\"${inscription_id}\",\"at_height\":${block_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "data.get('result') is None" "True"
}

regtest_assert_usdb_pass_energy_state() {
  local inscription_id="$1"
  local block_height="$2"
  local mode="$3"
  local expected_state="$4"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${inscription_id}\",\"block_height\":${block_height},\"mode\":\"${mode}\"}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('inscription_id')" "$inscription_id"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('query_block_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('state')" "$expected_state"
}

regtest_assert_usdb_pass_energy_not_found() {
  local inscription_id="$1"
  local block_height="$2"
  local mode="$3"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${inscription_id}\",\"block_height\":${block_height},\"mode\":\"${mode}\"}]")"
  regtest_assert_json_expr "$resp" "((data.get('error') or {}).get('code'))" "-32012"
  regtest_assert_json_expr "$resp" "((data.get('error') or {}).get('message'))" "ENERGY_NOT_FOUND"
  regtest_assert_json_expr "$resp" "(((data.get('error') or {}).get('data') or {}).get('inscription_id'))" "$inscription_id"
  regtest_assert_json_expr "$resp" "(((data.get('error') or {}).get('data') or {}).get('query_block_height'))" "$block_height"
}

regtest_assert_usdb_pass_stats() {
  local block_height="$1"
  local total_count="$2"
  local active_count="$3"
  local dormant_count="$4"
  local consumed_count="$5"
  local burned_count="$6"
  local invalid_count="$7"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_stats_at_height" "[{\"at_height\":${block_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('resolved_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('total_count')" "$total_count"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('active_count')" "$active_count"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('dormant_count')" "$dormant_count"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('consumed_count')" "$consumed_count"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('burned_count')" "$burned_count"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('invalid_count')" "$invalid_count"
}

regtest_assert_usdb_active_balance_snapshot_positive() {
  local block_height="$1"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_active_balance_snapshot" "[{\"block_height\":${block_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('block_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('active_address_count', 0) > 0" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('total_balance', 0) > 0" "True"
}

regtest_assert_usdb_active_balance_snapshot_zero() {
  local block_height="$1"
  local resp
  resp="$(regtest_rpc_call_usdb_indexer "get_active_balance_snapshot" "[{\"block_height\":${block_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('block_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('total_balance')" "0"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('active_address_count')" "0"
}

regtest_assert_usdb_pass_stats_zero() {
  local block_height="$1"
  local resp
  resp="$(regtest_rpc_call_usdb_indexer "get_pass_stats_at_height" "[{\"at_height\":${block_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('resolved_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('total_count')" "0"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('active_count')" "0"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('dormant_count')" "0"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('consumed_count')" "0"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('burned_count')" "0"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('invalid_count')" "0"
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

  regtest_log "Failure diagnostics: exit_code=${exit_code}, work_dir=${WORK_DIR}, btc_rpc_port=${BTC_RPC_PORT}, btc_p2p_port=${BTC_P2P_PORT}, bh_rpc_port=${BH_RPC_PORT}, usdb_rpc_port=${USDB_RPC_PORT}"
  regtest_print_tail_if_exists "ord server log" "$ORD_SERVER_LOG_FILE"
  regtest_print_tail_if_exists "balance-history log" "$BALANCE_HISTORY_LOG_FILE"
  regtest_print_tail_if_exists "usdb-indexer log" "$USDB_INDEXER_LOG_FILE"
  regtest_print_tail_if_exists "balance-history service log" "${BALANCE_HISTORY_ROOT}/logs/balance-history_rCURRENT.log"
  regtest_print_tail_if_exists "bitcoind debug log" "${BITCOIN_DIR}/regtest/debug.log"
}

regtest_cleanup() {
  local exit_code=$?
  set +e
  if [[ "$exit_code" -ne 0 ]]; then
    regtest_print_failure_diagnostics "$exit_code"
  fi
  regtest_stop_usdb_indexer
  regtest_stop_balance_history
  regtest_stop_ord_server
  regtest_stop_bitcoind
}
