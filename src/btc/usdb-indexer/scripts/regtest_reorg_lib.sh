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
USDB_INDEXER_RPC_PORT="${USDB_INDEXER_RPC_PORT:-29320}"
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
REGTEST_FATAL_LOG_PATTERN="${REGTEST_FATAL_LOG_PATTERN:-(^|[[:space:]])thread .* panicked at|panicked at |panic: |fatal runtime error|fatal error:|Fatal:}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
INSCRIPTION_SOURCE="${INSCRIPTION_SOURCE:-bitcoind}"
INSCRIPTION_FIXTURE_FILE="${INSCRIPTION_FIXTURE_FILE:-}"
USDB_ECONOMIC_VIEW_VERSION="${USDB_ECONOMIC_VIEW_VERSION:-uip-0006-usdb-economic-state-view:v1}"
USDB_CANDIDATE_SELECTION_RULE="${USDB_CANDIDATE_SELECTION_RULE:-uip-0006:effective-energy-desc-pass-id-asc:v1}"
USDB_ECONOMIC_PAGE_LIMIT="${USDB_ECONOMIC_PAGE_LIMIT:-2}"

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
    -X POST "http://127.0.0.1:${USDB_INDEXER_RPC_PORT}" \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}"
}

regtest_build_pass_economic_profile_params() {
  local pass_id="$1"
  local block_height="$2"
  local context_json="${3:-}"

  python3 - "$USDB_ECONOMIC_VIEW_VERSION" "$pass_id" "$block_height" "$context_json" <<'PY'
import json
import sys

view_version, pass_id, block_height, context_json = sys.argv[1:]
request = {
    "view_version": view_version,
    "pass_id": pass_id,
    "block_height": int(block_height),
}
if context_json:
    request["context"] = json.loads(context_json)
print(json.dumps([request]))
PY
}

regtest_get_pass_economic_profile_response() {
  local pass_id="$1"
  local block_height="$2"
  local context_json="${3:-}"
  local params

  params="$(regtest_build_pass_economic_profile_params "$pass_id" "$block_height" "$context_json")"
  regtest_rpc_call_usdb_indexer "get_pass_economic_profile" "$params"
}

regtest_build_candidate_set_view_params() {
  local block_height="$1"
  local context_json="$2"
  local cursor="$3"
  local limit="$4"

  python3 - \
    "$USDB_ECONOMIC_VIEW_VERSION" \
    "$USDB_CANDIDATE_SELECTION_RULE" \
    "$block_height" \
    "$context_json" \
    "$cursor" \
    "$limit" <<'PY'
import json
import sys

view_version, selection_rule, block_height, context_json, cursor, limit = sys.argv[1:]
request = {
    "view_version": view_version,
    "selection_rule": selection_rule,
    "cursor": cursor or None,
    "limit": int(limit),
}
if not cursor:
    request["block_height"] = int(block_height)
    if context_json:
        request["context"] = json.loads(context_json)
print(json.dumps([request]))
PY
}

# Collect one immutable candidate-set view across every opaque cursor page.
regtest_collect_candidate_set_view() {
  local block_height="$1"
  local context_json="${2:-}"
  local limit="${3:-$USDB_ECONOMIC_PAGE_LIMIT}"
  local cursor=""
  local aggregate_json=""
  local page_count=0
  local params resp

  while true; do
    params="$(regtest_build_candidate_set_view_params "$block_height" "$context_json" "$cursor" "$limit")"
    resp="$(regtest_rpc_call_usdb_indexer "get_candidate_set_view" "$params")"
    aggregate_json="$(python3 - \
      "$aggregate_json" \
      "$resp" \
      "$USDB_ECONOMIC_VIEW_VERSION" \
      "$USDB_CANDIDATE_SELECTION_RULE" \
      "$limit" <<'PY'
import json
import sys

aggregate_json, response_json, expected_view, expected_rule, expected_limit = sys.argv[1:]
response = json.loads(response_json)
if response.get("error") is not None:
    raise SystemExit(f"get_candidate_set_view failed: {response['error']}")
page = response.get("result") or {}
limit = int(expected_limit)
if page.get("view_version") != expected_view:
    raise SystemExit("candidate page view_version mismatch")
if page.get("selection_rule") != expected_rule:
    raise SystemExit("candidate page selection_rule mismatch")
if page.get("limit") != limit or int(page.get("max_limit", 0)) < limit:
    raise SystemExit("candidate page limit/max_limit mismatch")

if aggregate_json:
    aggregate = json.loads(aggregate_json)
    for field in ("view_version", "external_state", "selection_rule", "total", "limit", "max_limit"):
        if aggregate.get(field) != page.get(field):
            raise SystemExit(f"candidate continuation changed {field}")
else:
    aggregate = {
        "view_version": page["view_version"],
        "external_state": page["external_state"],
        "selection_rule": page["selection_rule"],
        "total": page["total"],
        "limit": page["limit"],
        "max_limit": page["max_limit"],
        "items": [],
    }

aggregate["items"].extend(page.get("items") or [])
aggregate["next_cursor"] = page.get("next_cursor")
if len(aggregate["items"]) > int(aggregate["total"]):
    raise SystemExit("candidate pagination returned more rows than total")
print(json.dumps(aggregate, separators=(",", ":")))
PY
)"

    cursor="$(python3 - "$aggregate_json" <<'PY'
import json
import sys

print(json.loads(sys.argv[1]).get("next_cursor") or "")
PY
)"
    if [[ -z "$cursor" ]]; then
      break
    fi
    page_count=$((page_count + 1))
    if (( page_count > 10000 )); then
      regtest_log "Candidate cursor pagination exceeded safety limit"
      return 1
    fi
  done

  python3 - "$aggregate_json" <<'PY'
import json
import sys

view = json.loads(sys.argv[1])
items = view.get("items") or []
if len(items) != int(view["total"]):
    raise SystemExit(f"candidate pagination incomplete: total={view['total']}, rows={len(items)}")
ids = [item["pass_id"] for item in items]
if len(ids) != len(set(ids)):
    raise SystemExit("candidate pagination returned duplicate pass ids")
expected = sorted(items, key=lambda item: (-int(item["effective_energy"]), item["pass_id"]))
if items != expected:
    raise SystemExit("candidate pagination violated effective_energy/pass_id ordering")
view["next_cursor"] = None
print(json.dumps(view, separators=(",", ":")))
PY
}

regtest_build_collab_breakdown_params() {
  local leader_pass_id="$1"
  local block_height="$2"
  local context_json="$3"
  local sort="$4"
  local cursor="$5"
  local limit="$6"

  python3 - \
    "$USDB_ECONOMIC_VIEW_VERSION" \
    "$leader_pass_id" \
    "$block_height" \
    "$context_json" \
    "$sort" \
    "$cursor" \
    "$limit" <<'PY'
import json
import sys

view_version, leader_pass_id, block_height, context_json, sort, cursor, limit = sys.argv[1:]
request = {
    "view_version": view_version,
    "leader_pass_id": leader_pass_id,
    "sort": sort,
    "cursor": cursor or None,
    "limit": int(limit),
}
if not cursor:
    request["block_height"] = int(block_height)
    if context_json:
        request["context"] = json.loads(context_json)
print(json.dumps([request]))
PY
}

# Collect and independently recompute one Leader's complete collab aggregate.
regtest_collect_collab_breakdown() {
  local leader_pass_id="$1"
  local block_height="$2"
  local context_json="${3:-}"
  local sort="${4:-collab_pass_id_asc}"
  local limit="${5:-$USDB_ECONOMIC_PAGE_LIMIT}"
  local cursor=""
  local aggregate_json=""
  local page_count=0
  local params resp

  while true; do
    params="$(regtest_build_collab_breakdown_params \
      "$leader_pass_id" "$block_height" "$context_json" "$sort" "$cursor" "$limit")"
    resp="$(regtest_rpc_call_usdb_indexer "get_collab_breakdown" "$params")"
    aggregate_json="$(python3 - \
      "$aggregate_json" \
      "$resp" \
      "$USDB_ECONOMIC_VIEW_VERSION" \
      "$leader_pass_id" \
      "$sort" \
      "$limit" <<'PY'
import json
import sys

aggregate_json, response_json, expected_view, expected_leader, expected_sort, expected_limit = sys.argv[1:]
response = json.loads(response_json)
if response.get("error") is not None:
    raise SystemExit(f"get_collab_breakdown failed: {response['error']}")
page = response.get("result") or {}
limit = int(expected_limit)
if page.get("view_version") != expected_view:
    raise SystemExit("breakdown page view_version mismatch")
if page.get("leader_pass_id") != expected_leader:
    raise SystemExit("breakdown page leader_pass_id mismatch")
if page.get("sort") != expected_sort:
    raise SystemExit("breakdown page sort mismatch")
if page.get("limit") != limit or int(page.get("max_limit", 0)) < limit:
    raise SystemExit("breakdown page limit/max_limit mismatch")

if aggregate_json:
    aggregate = json.loads(aggregate_json)
    fields = (
        "view_version", "external_state", "leader_pass_id", "leader_state",
        "leader_pass_kind", "sort", "total", "aggregate_collab_contribution",
        "limit", "max_limit",
    )
    for field in fields:
        if aggregate.get(field) != page.get(field):
            raise SystemExit(f"breakdown continuation changed {field}")
else:
    aggregate = {
        "view_version": page["view_version"],
        "external_state": page["external_state"],
        "leader_pass_id": page["leader_pass_id"],
        "leader_state": page["leader_state"],
        "leader_pass_kind": page["leader_pass_kind"],
        "sort": page["sort"],
        "total": page["total"],
        "aggregate_collab_contribution": page["aggregate_collab_contribution"],
        "limit": page["limit"],
        "max_limit": page["max_limit"],
        "items": [],
    }

aggregate["items"].extend(page.get("items") or [])
aggregate["next_cursor"] = page.get("next_cursor")
if len(aggregate["items"]) > int(aggregate["total"]):
    raise SystemExit("breakdown pagination returned more rows than total")
print(json.dumps(aggregate, separators=(",", ":")))
PY
)"

    cursor="$(python3 - "$aggregate_json" <<'PY'
import json
import sys

print(json.loads(sys.argv[1]).get("next_cursor") or "")
PY
)"
    if [[ -z "$cursor" ]]; then
      break
    fi
    page_count=$((page_count + 1))
    if (( page_count > 10000 )); then
      regtest_log "Collab breakdown cursor pagination exceeded safety limit"
      return 1
    fi
  done

  python3 - "$aggregate_json" <<'PY'
import json
import sys

view = json.loads(sys.argv[1])
items = view.get("items") or []
if len(items) != int(view["total"]):
    raise SystemExit(f"breakdown pagination incomplete: total={view['total']}, rows={len(items)}")
ids = [item["collab_pass_id"] for item in items]
if len(ids) != len(set(ids)):
    raise SystemExit("breakdown pagination returned duplicate collab pass ids")
if view["sort"] == "collab_pass_id_asc":
    expected = sorted(items, key=lambda item: item["collab_pass_id"])
else:
    expected = sorted(items, key=lambda item: (-int(item["collab_contribution"]), item["collab_pass_id"]))
if items != expected:
    raise SystemExit("breakdown pagination violated its declared ordering")
energy_max = (1 << 128) - 1
recomputed = min(sum(int(item["collab_contribution"]) for item in items), energy_max)
if recomputed != int(view["aggregate_collab_contribution"]):
    raise SystemExit("breakdown rows do not recompute aggregate_collab_contribution")
view["next_cursor"] = None
print(json.dumps(view, separators=(",", ":")))
PY
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

regtest_write_validator_payload_v1() {
  local payload_file="$1"
  local state_ref_resp="$2"
  local pass_snapshot_resp="$3"
  local pass_energy_resp="$4"

  python3 - "$payload_file" "$state_ref_resp" "$pass_snapshot_resp" "$pass_energy_resp" <<'PY'
import json
import pathlib
import sys

payload_file = pathlib.Path(sys.argv[1])
state_ref = json.loads(sys.argv[2])["result"]
pass_snapshot = json.loads(sys.argv[3])["result"]
pass_energy = json.loads(sys.argv[4])["result"]

payload = {
    "payload_version": "1.0.0",
    "external_state": {
        "btc_height": state_ref["block_height"],
        "snapshot_id": state_ref["snapshot_info"]["snapshot_id"],
        "stable_block_hash": state_ref["snapshot_info"]["stable_block_hash"],
        "balance_history_api_version": state_ref["snapshot_info"]["consensus_identity"]["balance_history_api_version"],
        "balance_history_semantics_version": state_ref["snapshot_info"]["consensus_identity"]["balance_history_semantics_version"],
        "local_state_commit": state_ref["local_state_commit_info"]["local_state_commit"],
        "system_state_id": state_ref["system_state_info"]["system_state_id"],
        "activation_registry_id": state_ref["local_state_commit_info"]["activation_registry_id"],
        "active_version_set": state_ref["local_state_commit_info"]["active_version_set"],
        "active_version_set_id": state_ref["local_state_commit_info"]["active_version_set_id"],
    },
    "miner_selection": {
        "inscription_id": pass_snapshot["inscription_id"],
        "owner": pass_snapshot["owner"],
        "state": pass_snapshot["state"],
        "raw_energy": pass_energy["raw_energy"],
        "collab_contribution": pass_energy["collab_contribution"],
        "effective_energy": pass_energy["effective_energy"],
        "resolved_height": pass_snapshot["resolved_height"],
        "query_block_height": pass_energy["query_block_height"],
    },
}

payload_file.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

regtest_write_validator_payload_from_profile_v1() {
  local payload_file="$1"
  local profile_resp="$2"

  python3 - "$payload_file" "$profile_resp" "$USDB_ECONOMIC_VIEW_VERSION" <<'PY'
import json
import pathlib
import sys

payload_file = pathlib.Path(sys.argv[1])
response = json.loads(sys.argv[2])
expected_view = sys.argv[3]
if response.get("error") is not None:
    raise SystemExit(f"get_pass_economic_profile failed: {response['error']}")
profile = response.get("result") or {}
if profile.get("view_version") != expected_view:
    raise SystemExit("profile response view_version mismatch")
external_state = profile["external_state"]
pass_profile = profile["pass"]
height = external_state["btc_height"]

payload = {
    "payload_version": "1.0.0",
    "external_state": external_state,
    "miner_selection": {
        "inscription_id": pass_profile["pass_id"],
        "owner": pass_profile["owner_script_hash"],
        "state": pass_profile["state"],
        "raw_energy": pass_profile["raw_energy"],
        "collab_contribution": pass_profile["collab_contribution"],
        "effective_energy": pass_profile["effective_energy"],
        "resolved_height": height,
        "query_block_height": height,
    },
}

payload_file.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

regtest_build_validator_candidate_entry_json() {
  local pass_snapshot_resp="$1"
  local pass_energy_resp="$2"

  python3 - "$pass_snapshot_resp" "$pass_energy_resp" <<'PY'
import json
import sys

pass_snapshot = json.loads(sys.argv[1])["result"]
pass_energy = json.loads(sys.argv[2])["result"]

print(json.dumps({
    "inscription_id": pass_snapshot["inscription_id"],
    "owner": pass_snapshot["owner"],
    "state": pass_snapshot["state"],
    "raw_energy": pass_energy["raw_energy"],
    "collab_contribution": pass_energy["collab_contribution"],
    "effective_energy": pass_energy["effective_energy"],
    "resolved_height": pass_snapshot["resolved_height"],
    "query_block_height": pass_energy["query_block_height"],
}))
PY
}

regtest_write_validator_competition_payload_v1() {
  local payload_file="$1"
  local state_ref_resp="$2"
  local winner_snapshot_resp="$3"
  local winner_energy_resp="$4"
  local candidate_entries_json="$5"

  python3 - "$payload_file" "$state_ref_resp" "$winner_snapshot_resp" "$winner_energy_resp" "$candidate_entries_json" <<'PY'
import json
import pathlib
import sys

payload_file = pathlib.Path(sys.argv[1])
state_ref = json.loads(sys.argv[2])["result"]
winner_snapshot = json.loads(sys.argv[3])["result"]
winner_energy = json.loads(sys.argv[4])["result"]
candidates = json.loads(sys.argv[5])

payload = {
    "payload_version": "1.1.0",
    "external_state": {
        "btc_height": state_ref["block_height"],
        "snapshot_id": state_ref["snapshot_info"]["snapshot_id"],
        "stable_block_hash": state_ref["snapshot_info"]["stable_block_hash"],
        "balance_history_api_version": state_ref["snapshot_info"]["consensus_identity"]["balance_history_api_version"],
        "balance_history_semantics_version": state_ref["snapshot_info"]["consensus_identity"]["balance_history_semantics_version"],
        "local_state_commit": state_ref["local_state_commit_info"]["local_state_commit"],
        "system_state_id": state_ref["system_state_info"]["system_state_id"],
        "activation_registry_id": state_ref["local_state_commit_info"]["activation_registry_id"],
        "active_version_set": state_ref["local_state_commit_info"]["active_version_set"],
        "active_version_set_id": state_ref["local_state_commit_info"]["active_version_set_id"],
    },
    "miner_selection": {
        "inscription_id": winner_snapshot["inscription_id"],
        "owner": winner_snapshot["owner"],
        "state": winner_snapshot["state"],
        "raw_energy": winner_energy["raw_energy"],
        "collab_contribution": winner_energy["collab_contribution"],
        "effective_energy": winner_energy["effective_energy"],
        "resolved_height": winner_snapshot["resolved_height"],
        "query_block_height": winner_energy["query_block_height"],
    },
    "selection_rule": "uip-0006:effective-energy-desc-pass-id-asc:v1",
    "candidate_passes": candidates,
}

payload_file.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

regtest_write_validator_competition_payload_tampered_winner() {
  local src_payload_file="$1"
  local dst_payload_file="$2"
  local winner_inscription_id="$3"

  python3 - "$src_payload_file" "$dst_payload_file" "$winner_inscription_id" <<'PY'
import json
import pathlib
import sys

src = pathlib.Path(sys.argv[1])
dst = pathlib.Path(sys.argv[2])
winner_id = sys.argv[3]
payload = json.loads(src.read_text())
candidates = payload.get("candidate_passes") or []
match = next((item for item in candidates if item.get("inscription_id") == winner_id), None)
if match is None:
    raise SystemExit(f"winner candidate not found: {winner_id}")

payload["miner_selection"] = {
    "inscription_id": match["inscription_id"],
    "owner": match["owner"],
    "state": match["state"],
    "raw_energy": match["raw_energy"],
    "collab_contribution": match["collab_contribution"],
    "effective_energy": match["effective_energy"],
    "resolved_height": match["resolved_height"],
    "query_block_height": match["query_block_height"],
}

dst.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

regtest_write_validator_payload_tampered_external_state_field() {
  local src_payload_file="$1"
  local dst_payload_file="$2"
  local field_name="$3"
  local field_value="$4"

  python3 - "$src_payload_file" "$dst_payload_file" "$field_name" "$field_value" <<'PY'
import json
import pathlib
import sys

src = pathlib.Path(sys.argv[1])
dst = pathlib.Path(sys.argv[2])
field_name = sys.argv[3]
field_value = sys.argv[4]

payload = json.loads(src.read_text())
payload.setdefault("external_state", {})[field_name] = field_value
dst.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

regtest_build_validator_candidate_entries_for_passes_at_height() {
  local block_height="$1"
  shift
  local candidate_ids=("$@")

  local candidate_view
  candidate_view="$(regtest_collect_candidate_set_view "$block_height" "" "$USDB_ECONOMIC_PAGE_LIMIT")"

  python3 - "$candidate_view" "${candidate_ids[@]}" <<'PY'
import json
import sys

view = json.loads(sys.argv[1])
expected_ids = sys.argv[2:]
items = view.get("items") or []
actual_ids = [item["pass_id"] for item in items]
if len(expected_ids) != len(set(expected_ids)):
    raise SystemExit("candidate helper received duplicate pass ids")
if set(actual_ids) != set(expected_ids) or len(actual_ids) != len(expected_ids):
    raise SystemExit(
        f"candidate helper ids do not match canonical view: expected={expected_ids}, actual={actual_ids}"
    )

height = view["external_state"]["btc_height"]
entries = [
    {
        "inscription_id": item["pass_id"],
        "owner": item["owner_script_hash"],
        "state": item["state"],
        "raw_energy": item["raw_energy"],
        "collab_contribution": item["collab_contribution"],
        "effective_energy": item["effective_energy"],
        "resolved_height": height,
        "query_block_height": height,
    }
    for item in items
]
print(json.dumps(entries, separators=(",", ":")))
PY
}

regtest_choose_validator_candidate_set_winner_json() {
  local candidate_entries_json="$1"

  python3 - "$candidate_entries_json" <<'PY'
import json
import sys

candidates = json.loads(sys.argv[1])
winner = min(candidates, key=lambda item: (-int(item["effective_energy"]), item["inscription_id"]))
print(json.dumps(winner))
PY
}

regtest_choose_validator_candidate_set_winner_id() {
  local candidate_entries_json="$1"
  regtest_choose_validator_candidate_set_winner_json "$candidate_entries_json" | \
    python3 -c 'import json,sys; print(json.load(sys.stdin)["inscription_id"])'
}

regtest_write_validator_candidate_set_payload_for_passes_at_height() {
  local payload_file="$1"
  local block_height="$2"
  local winner_id="$3"
  shift 3
  local candidate_ids=("$@")

  local candidate_view

  regtest_wait_usdb_state_ref_available "$block_height"
  candidate_view="$(regtest_collect_candidate_set_view "$block_height" "" "$USDB_ECONOMIC_PAGE_LIMIT")"

  python3 - "$payload_file" "$candidate_view" "$winner_id" "${candidate_ids[@]}" <<'PY'
import json
import pathlib
import sys

payload_file = pathlib.Path(sys.argv[1])
view = json.loads(sys.argv[2])
expected_winner = sys.argv[3]
expected_ids = sys.argv[4:]
items = view.get("items") or []
actual_ids = [item["pass_id"] for item in items]
if not items:
    raise SystemExit("canonical candidate view is empty")
if len(expected_ids) != len(set(expected_ids)):
    raise SystemExit("candidate payload helper received duplicate pass ids")
if set(actual_ids) != set(expected_ids) or len(actual_ids) != len(expected_ids):
    raise SystemExit(
        f"candidate payload ids do not match canonical view: expected={expected_ids}, actual={actual_ids}"
    )
if expected_winner != actual_ids[0]:
    raise SystemExit(
        f"declared winner does not match canonical view: expected={expected_winner}, actual={actual_ids[0]}"
    )

height = view["external_state"]["btc_height"]

def payload_candidate(item):
    return {
        "inscription_id": item["pass_id"],
        "owner": item["owner_script_hash"],
        "state": item["state"],
        "raw_energy": item["raw_energy"],
        "collab_contribution": item["collab_contribution"],
        "effective_energy": item["effective_energy"],
        "resolved_height": height,
        "query_block_height": height,
    }

candidates = [payload_candidate(item) for item in items]
payload = {
    "payload_version": "1.1.0",
    "external_state": view["external_state"],
    "miner_selection": candidates[0],
    "selection_rule": view["selection_rule"],
    "candidate_passes": candidates,
}
payload_file.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

regtest_write_validator_competition_payload_for_passes_at_height() {
  regtest_write_validator_candidate_set_payload_for_passes_at_height "$@"
}

regtest_validator_payload_expr() {
  local payload_file="$1"
  local expression="$2"

  python3 - "$payload_file" "$expression" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
expression = sys.argv[2]
print(eval(expression, {"__builtins__": {}}, {"data": payload}))
PY
}

regtest_validator_payload_version() {
  local payload_file="$1"
  regtest_validator_payload_expr "$payload_file" "data['payload_version']"
}

regtest_validator_payload_context_json() {
  local payload_file="$1"

  python3 - "$payload_file" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
state = payload["external_state"]

print(json.dumps({
    "requested_height": state["btc_height"],
    "expected_state": {
        "snapshot_id": state["snapshot_id"],
        "stable_height": state["btc_height"],
        "stable_block_hash": state["stable_block_hash"],
        "balance_history_api_version": state["balance_history_api_version"],
        "balance_history_semantics_version": state["balance_history_semantics_version"],
        "local_state_commit": state["local_state_commit"],
        "system_state_id": state["system_state_id"],
        "activation_registry_id": state["activation_registry_id"],
        "active_version_set_id": state["active_version_set_id"],
    },
}))
PY
}

regtest_build_validator_state_ref_params() {
  local payload_file="$1"
  local block_height context_json

  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"

  python3 - "$block_height" "$context_json" <<'PY'
import json
import sys

block_height = int(sys.argv[1])
context = json.loads(sys.argv[2])
print(json.dumps([{
    "block_height": block_height,
    "context": context,
}]))
PY
}

regtest_build_validator_pass_economic_profile_params() {
  local payload_file="$1"
  local pass_id="${2:-}"
  local block_height context_json

  if [[ -z "$pass_id" ]]; then
    pass_id="$(regtest_validator_payload_expr "$payload_file" "data['miner_selection']['inscription_id']")"
  fi
  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"
  regtest_build_pass_economic_profile_params "$pass_id" "$block_height" "$context_json"
}

regtest_build_validator_candidate_set_view_params() {
  local payload_file="$1"
  local cursor="${2:-}"
  local limit="${3:-$USDB_ECONOMIC_PAGE_LIMIT}"
  local block_height context_json

  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"
  regtest_build_candidate_set_view_params "$block_height" "$context_json" "$cursor" "$limit"
}

regtest_build_validator_collab_breakdown_params() {
  local payload_file="$1"
  local leader_pass_id="${2:-}"
  local cursor="${3:-}"
  local limit="${4:-$USDB_ECONOMIC_PAGE_LIMIT}"
  local block_height context_json

  if [[ -z "$leader_pass_id" ]]; then
    leader_pass_id="$(regtest_validator_payload_expr "$payload_file" "data['miner_selection']['inscription_id']")"
  fi
  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"
  regtest_build_collab_breakdown_params \
    "$leader_pass_id" \
    "$block_height" \
    "$context_json" \
    "collab_pass_id_asc" \
    "$cursor" \
    "$limit"
}

regtest_collect_candidate_set_view_for_payload() {
  local payload_file="$1"
  local limit="${2:-$USDB_ECONOMIC_PAGE_LIMIT}"
  local block_height context_json

  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"
  regtest_collect_candidate_set_view "$block_height" "$context_json" "$limit"
}

regtest_collect_collab_breakdown_for_payload() {
  local payload_file="$1"
  local leader_pass_id="${2:-}"
  local limit="${3:-$USDB_ECONOMIC_PAGE_LIMIT}"
  local block_height context_json

  if [[ -z "$leader_pass_id" ]]; then
    leader_pass_id="$(regtest_validator_payload_expr "$payload_file" "data['miner_selection']['inscription_id']")"
  fi
  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"
  regtest_collect_collab_breakdown \
    "$leader_pass_id" \
    "$block_height" \
    "$context_json" \
    "collab_pass_id_asc" \
    "$limit"
}

regtest_build_validator_pass_snapshot_params() {
  local payload_file="$1"
  local pass_id block_height context_json

  pass_id="$(regtest_validator_payload_expr "$payload_file" "data['miner_selection']['inscription_id']")"
  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"

  python3 - "$pass_id" "$block_height" "$context_json" <<'PY'
import json
import sys

inscription_id = sys.argv[1]
block_height = int(sys.argv[2])
context = json.loads(sys.argv[3])
print(json.dumps([{
    "inscription_id": inscription_id,
    "at_height": block_height,
    "context": context,
}]))
PY
}

regtest_build_validator_pass_energy_params() {
  local payload_file="$1"
  local pass_id block_height context_json

  pass_id="$(regtest_validator_payload_expr "$payload_file" "data['miner_selection']['inscription_id']")"
  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  context_json="$(regtest_validator_payload_context_json "$payload_file")"

  python3 - "$pass_id" "$block_height" "$context_json" <<'PY'
import json
import sys

inscription_id = sys.argv[1]
block_height = int(sys.argv[2])
context = json.loads(sys.argv[3])
print(json.dumps([{
    "inscription_id": inscription_id,
    "block_height": block_height,
    "mode": "at_or_before",
    "context": context,
}]))
PY
}

regtest_validate_validator_payload_success() {
  local payload_file="$1"
  local block_height
  local expected_snapshot_id expected_system_state_id
  local state_ref_params profile_params resp profile_resp candidate_view breakdown_view

  block_height="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['btc_height']")"
  expected_snapshot_id="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['snapshot_id']")"
  expected_system_state_id="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['system_state_id']")"

  state_ref_params="$(regtest_build_validator_state_ref_params "$payload_file")"
  profile_params="$(regtest_build_validator_pass_economic_profile_params "$payload_file")"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('block_height')" "$block_height"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('snapshot_info') or {}).get('snapshot_id')" "$expected_snapshot_id"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('system_state_info') or {}).get('system_state_id')" "$expected_system_state_id"

  profile_resp="$(regtest_rpc_call_usdb_indexer "get_pass_economic_profile" "$profile_params")"
  candidate_view="$(regtest_collect_candidate_set_view_for_payload "$payload_file")"
  breakdown_view="$(regtest_collect_collab_breakdown_for_payload "$payload_file")"

  python3 - \
    "$payload_file" \
    "$profile_resp" \
    "$candidate_view" \
    "$breakdown_view" \
    "$USDB_ECONOMIC_VIEW_VERSION" \
    "$USDB_CANDIDATE_SELECTION_RULE" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
profile_response = json.loads(sys.argv[2])
candidate_view = json.loads(sys.argv[3])
breakdown = json.loads(sys.argv[4])
expected_view = sys.argv[5]
expected_rule = sys.argv[6]

if profile_response.get("error") is not None:
    raise SystemExit(f"get_pass_economic_profile failed: {profile_response['error']}")
profile = profile_response.get("result") or {}
if profile.get("view_version") != expected_view:
    raise SystemExit("profile view_version mismatch")
if candidate_view.get("view_version") != expected_view or breakdown.get("view_version") != expected_view:
    raise SystemExit("economic view version mismatch across profile/candidate/breakdown")
if candidate_view.get("selection_rule") != expected_rule:
    raise SystemExit("candidate selection_rule mismatch")

external_state = payload["external_state"]
for name, actual in (
    ("profile", profile.get("external_state")),
    ("candidate", candidate_view.get("external_state")),
    ("breakdown", breakdown.get("external_state")),
):
    if actual != external_state:
        raise SystemExit(f"{name} external_state does not match validator payload")

expected = payload["miner_selection"]
pass_profile = profile["pass"]
field_pairs = {
    "inscription_id": "pass_id",
    "owner": "owner_script_hash",
    "state": "state",
    "raw_energy": "raw_energy",
    "collab_contribution": "collab_contribution",
    "effective_energy": "effective_energy",
}
for payload_field, profile_field in field_pairs.items():
    if expected[payload_field] != pass_profile[profile_field]:
        raise SystemExit(f"profile mismatch for {payload_field}")
height = int(external_state["btc_height"])
if int(expected["resolved_height"]) != height or int(expected["query_block_height"]) != height:
    raise SystemExit("validator payload height fields do not match external_state")
if not isinstance(pass_profile.get("level"), int) or not isinstance(pass_profile.get("difficulty_factor_bps"), int):
    raise SystemExit("profile is missing UIP-0005 derived fields")

candidate_matches = [
    item for item in candidate_view.get("items") or []
    if item.get("pass_id") == pass_profile["pass_id"]
]
is_candidate = pass_profile["state"] == "active" and pass_profile["pass_kind"] == "standard"
if is_candidate:
    if len(candidate_matches) != 1:
        raise SystemExit("active standard profile is not represented exactly once in candidate view")
    candidate = candidate_matches[0]
    candidate_pairs = {
        "owner_script_hash": "owner_script_hash",
        "state": "state",
        "pass_kind": "pass_kind",
        "raw_energy": "raw_energy",
        "collab_contribution": "collab_contribution",
        "effective_energy": "effective_energy",
        "level": "level",
        "difficulty_factor_bps": "difficulty_factor_bps",
    }
    for candidate_field, profile_field in candidate_pairs.items():
        if candidate[candidate_field] != pass_profile[profile_field]:
            raise SystemExit(f"candidate/profile mismatch for {candidate_field}")
elif candidate_matches:
    raise SystemExit("non-candidate profile unexpectedly appears in candidate view")

if breakdown["leader_pass_id"] != pass_profile["pass_id"]:
    raise SystemExit("breakdown leader does not match profile pass")
if breakdown["leader_state"] != pass_profile["state"] or breakdown["leader_pass_kind"] != pass_profile["pass_kind"]:
    raise SystemExit("breakdown leader state/kind does not match profile")
if breakdown["aggregate_collab_contribution"] != pass_profile["collab_contribution"]:
    raise SystemExit("breakdown aggregate does not match profile collab_contribution")
if int(breakdown["total"]) != int(pass_profile["collab_breakdown_count"]):
    raise SystemExit("breakdown total does not match profile collab_breakdown_count")
PY

  regtest_log "UIP-0006 economic views validated: height=${block_height}, payload=${payload_file}"
}

regtest_validate_validator_candidate_set_payload_success() {
  local payload_file="$1"
  local candidate_view

  regtest_validate_validator_payload_success "$payload_file"
  candidate_view="$(regtest_collect_candidate_set_view_for_payload "$payload_file")"

  python3 - "$payload_file" "$candidate_view" "$USDB_CANDIDATE_SELECTION_RULE" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
view = json.loads(sys.argv[2])
expected_rule = sys.argv[3]
if payload.get("selection_rule") != expected_rule or view.get("selection_rule") != expected_rule:
    raise SystemExit("candidate payload/view selection_rule mismatch")

payload_candidates = payload.get("candidate_passes") or []
view_candidates = view.get("items") or []
if len(payload_candidates) != int(view["total"]):
    raise SystemExit("payload candidate count does not match canonical candidate view")
payload_by_id = {item["inscription_id"]: item for item in payload_candidates}
view_by_id = {item["pass_id"]: item for item in view_candidates}
if len(payload_by_id) != len(payload_candidates) or set(payload_by_id) != set(view_by_id):
    raise SystemExit("payload candidate ids do not match canonical candidate view")

height = int(view["external_state"]["btc_height"])
field_pairs = {
    "owner": "owner_script_hash",
    "state": "state",
    "raw_energy": "raw_energy",
    "collab_contribution": "collab_contribution",
    "effective_energy": "effective_energy",
}
for pass_id, expected in payload_by_id.items():
    actual = view_by_id[pass_id]
    for payload_field, view_field in field_pairs.items():
        if expected[payload_field] != actual[view_field]:
            raise SystemExit(f"candidate {pass_id} mismatch for {payload_field}")
    if int(expected["resolved_height"]) != height or int(expected["query_block_height"]) != height:
        raise SystemExit(f"candidate {pass_id} height fields do not match external_state")

if not view_candidates:
    raise SystemExit("candidate-set payload cannot validate against an empty candidate view")
winner = payload["miner_selection"]
canonical_winner = view_candidates[0]
if winner["inscription_id"] != canonical_winner["pass_id"]:
    raise SystemExit("payload winner does not match canonical candidate view first row")
for payload_field, view_field in field_pairs.items():
    if winner[payload_field] != canonical_winner[view_field]:
        raise SystemExit(f"winner mismatch for {payload_field}")
PY
}

regtest_validate_validator_competition_payload_success() {
  regtest_validate_validator_candidate_set_payload_success "$@"
}

regtest_validate_validator_candidate_set_payload_tampered_selection() {
  local payload_file="$1"
  local computed_winner_json computed_winner_id winner_id

  regtest_validate_validator_payload_success "$payload_file"

  computed_winner_json="$(python3 - "$payload_file" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
candidates = payload.get("candidate_passes") or []
winner = min(candidates, key=lambda item: (-int(item["effective_energy"]), item["inscription_id"]))
print(json.dumps(winner))
PY
)"
  computed_winner_id="$(printf '%s' "$computed_winner_json" | python3 -c 'import json,sys; print(json.load(sys.stdin)["inscription_id"])')"
  winner_id="$(regtest_validator_payload_expr "$payload_file" "data['miner_selection']['inscription_id']")"

  if [[ "$winner_id" == "$computed_winner_id" ]]; then
    regtest_log "Expected tampered competition payload to disagree with computed winner, but both are ${winner_id}"
    exit 1
  fi
}

regtest_validate_validator_competition_payload_tampered_selection() {
  regtest_validate_validator_candidate_set_payload_tampered_selection "$@"
}

regtest_validate_validator_candidate_set_payload_consensus_error() {
  local payload_file="$1"
  local expected_code="$2"
  local expected_message="$3"
  local candidate_count idx candidate_id profile_params resp

  regtest_validate_validator_payload_consensus_error "$payload_file" "$expected_code" "$expected_message"

  candidate_count="$(regtest_validator_payload_expr "$payload_file" "((data.get('candidate_passes') or []).__len__())")"
  idx=0
  while (( idx < candidate_count )); do
    candidate_id="$(regtest_validator_payload_expr "$payload_file" "data['candidate_passes'][$idx]['inscription_id']")"
    profile_params="$(regtest_build_validator_pass_economic_profile_params "$payload_file" "$candidate_id")"
    resp="$(regtest_rpc_call_usdb_indexer "get_pass_economic_profile" "$profile_params")"
    regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"
    idx=$((idx + 1))
  done
}

regtest_validate_validator_competition_payload_consensus_error() {
  regtest_validate_validator_candidate_set_payload_consensus_error "$@"
}

regtest_validate_validator_payload_versioned_success() {
  local payload_file="$1"
  local payload_version

  payload_version="$(regtest_validator_payload_version "$payload_file")"
  case "$payload_version" in
    1.0.0)
      regtest_validate_validator_payload_success "$payload_file"
      ;;
    1.1.0)
      regtest_validate_validator_competition_payload_success "$payload_file"
      ;;
    *)
      regtest_log "Unsupported validator payload_version for success validation: ${payload_version}"
      exit 1
      ;;
  esac
}

regtest_validate_validator_payload_versioned_consensus_error() {
  local payload_file="$1"
  local expected_code="$2"
  local expected_message="$3"
  local payload_version

  payload_version="$(regtest_validator_payload_version "$payload_file")"
  case "$payload_version" in
    1.0.0)
      regtest_validate_validator_payload_consensus_error "$payload_file" "$expected_code" "$expected_message"
      ;;
    1.1.0)
      regtest_validate_validator_competition_payload_consensus_error "$payload_file" "$expected_code" "$expected_message"
      ;;
    *)
      regtest_log "Unsupported validator payload_version for consensus-error validation: ${payload_version}"
      exit 1
      ;;
  esac
}

regtest_validate_validator_payload_consensus_error() {
  local payload_file="$1"
  local expected_code="$2"
  local expected_message="$3"
  local state_ref_params profile_params candidate_params breakdown_params resp

  state_ref_params="$(regtest_build_validator_state_ref_params "$payload_file")"
  profile_params="$(regtest_build_validator_pass_economic_profile_params "$payload_file")"
  candidate_params="$(regtest_build_validator_candidate_set_view_params "$payload_file")"
  breakdown_params="$(regtest_build_validator_collab_breakdown_params "$payload_file")"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_economic_profile" "$profile_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"

  resp="$(regtest_rpc_call_usdb_indexer "get_candidate_set_view" "$candidate_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"

  resp="$(regtest_rpc_call_usdb_indexer "get_collab_breakdown" "$breakdown_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"
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
  regtest_wait_rpc_ready "usdb-indexer" "http://127.0.0.1:${USDB_INDEXER_RPC_PORT}" "get_network_type" "[]"
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

regtest_wait_usdb_rpc_alive_but_not_consensus_ready() {
  regtest_log "Waiting for usdb-indexer rpc_alive=true and consensus_ready=false"

  local start_ts now readiness_resp rpc_alive consensus_ready
  start_ts="$(date +%s)"
  while true; do
    readiness_resp="$(regtest_rpc_call_usdb_indexer "get_readiness" "[]")"
    rpc_alive="$(echo "$readiness_resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result") or {}; print("1" if r.get("rpc_alive") else "0")')"
    consensus_ready="$(echo "$readiness_resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result") or {}; print("1" if r.get("consensus_ready") else "0")')"
    if [[ "$rpc_alive" == "1" && "$consensus_ready" == "0" ]]; then
      regtest_log "usdb-indexer reached rpc_alive=true, consensus_ready=false"
      return 0
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "Timed out waiting for usdb-indexer readiness window. last_response=${readiness_resp}"
      exit 1
    fi

    sleep 0.2
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
    "rpc_server_port": ${USDB_INDEXER_RPC_PORT},
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

regtest_get_ord_server_block_count() {
  curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
    "http://127.0.0.1:${ORD_RPC_PORT}/blockcount" | tr -d '\n\r '
}

regtest_wait_until_ord_server_synced_to_bitcoind() {
  local start_ts now ord_block_count btc_height expected_ord_block_count
  regtest_log "Waiting until ord server catches up to bitcoind"
  start_ts="$(date +%s)"
  while true; do
    btc_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount 2>/dev/null || echo 0)"
    ord_block_count="$(regtest_get_ord_server_block_count 2>/dev/null || echo 0)"
    if [[ "$ord_block_count" =~ ^[0-9]+$ ]] && [[ "$btc_height" =~ ^[0-9]+$ ]]; then
      expected_ord_block_count=$((btc_height + 1))
      if (( ord_block_count >= expected_ord_block_count )); then
        return 0
      fi
    fi

    now="$(date +%s)"
    if (( now - start_ts > SYNC_TIMEOUT_SEC )); then
      regtest_log "ord server sync timeout: ord_block_count=${ord_block_count:-unknown}, btc_height=${btc_height:-unknown}"
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
    echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} Failed to parse inscription id from ord output: ${output}" >&2
    return 1
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
    echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} Failed to parse transfer txid from ord output: ${output}" >&2
    return 1
  fi

  echo "$txid"
}

regtest_ord_burn_inscription() {
  local wallet_name="$1"
  local inscription_id="$2"
  local output txid

  echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} Burn inscription via ord: wallet=${wallet_name}, inscription_id=${inscription_id}" >&2
  output="$(regtest_run_ord_wallet_named "$wallet_name" burn --fee-rate "$ORD_FEE_RATE" "$inscription_id" 2>&1 || true)"
  txid="$(regtest_extract_txid "$output")"
  if [[ -z "$txid" ]]; then
    echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} Failed to parse burn txid from ord output: ${output}" >&2
    return 1
  fi

  echo "$txid"
}

regtest_start_balance_history() {
  regtest_log "Starting balance-history service (root=${BALANCE_HISTORY_ROOT}, rpc=${BH_RPC_PORT})"
  (
    cd "$REPO_ROOT" || exit 1
    cargo run --manifest-path src/btc/Cargo.toml -p balance-history -- \
      --root-dir "$BALANCE_HISTORY_ROOT" \
      --skip-process-lock
  ) >"${BALANCE_HISTORY_LOG_FILE}" 2>&1 &
  BALANCE_HISTORY_PID=$!
}

regtest_start_usdb_indexer() {
  regtest_log "Starting usdb-indexer service (root=${USDB_INDEXER_ROOT}, rpc=${USDB_INDEXER_RPC_PORT})"
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
    cd "$REPO_ROOT" || exit 1
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

regtest_assert_managed_process_alive() {
  local label="$1"
  local pid="$2"
  local state

  if [[ -z "$pid" ]]; then
    return 0
  fi
  state="$(ps -o stat= -p "$pid" 2>/dev/null | tr -d ' ' || true)"
  if [[ -z "$state" ]] || [[ "$state" == Z* ]] || ! kill -0 "$pid" 2>/dev/null; then
    regtest_log "Managed process exited unexpectedly: service=${label}, pid=${pid}, state=${state:-missing}"
    wait "$pid" >/dev/null 2>&1 || true
    return 1
  fi
}

regtest_assert_log_has_no_fatal() {
  local label="$1"
  local file_path="$2"
  local matches

  if [[ ! -f "$file_path" ]]; then
    return 0
  fi
  matches="$(grep -Ein -m 5 "$REGTEST_FATAL_LOG_PATTERN" "$file_path" || true)"
  if [[ -n "$matches" ]]; then
    regtest_log "Fatal process log detected: service=${label}, log=${file_path}"
    printf '%s\n' "$matches" >&2
    return 1
  fi
}

regtest_assert_managed_processes_alive() {
  local failed=0

  regtest_assert_managed_process_alive "bitcoind" "$BITCOIND_PID" || failed=1
  regtest_assert_managed_process_alive "ord-server" "$ORD_SERVER_PID" || failed=1
  regtest_assert_managed_process_alive "balance-history" "$BALANCE_HISTORY_PID" || failed=1
  regtest_assert_managed_process_alive "usdb-indexer" "$USDB_INDEXER_PID" || failed=1
  return "$failed"
}

regtest_assert_managed_logs_clean() {
  local failed=0

  regtest_assert_log_has_no_fatal "ord-server" "$ORD_SERVER_LOG_FILE" || failed=1
  regtest_assert_log_has_no_fatal "balance-history" "$BALANCE_HISTORY_LOG_FILE" || failed=1
  regtest_assert_log_has_no_fatal "usdb-indexer" "$USDB_INDEXER_LOG_FILE" || failed=1
  regtest_assert_log_has_no_fatal \
    "balance-history-service" \
    "${BALANCE_HISTORY_ROOT}/logs/balance-history_rCURRENT.log" || failed=1
  regtest_assert_log_has_no_fatal "bitcoind" "${BITCOIN_DIR}/regtest/debug.log" || failed=1
  return "$failed"
}

regtest_stop_balance_history() {
  if [[ -n "$BALANCE_HISTORY_PID" ]]; then
    if ! regtest_assert_managed_process_alive "balance-history" "$BALANCE_HISTORY_PID"; then
      BALANCE_HISTORY_PID=""
      return 1
    fi
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
  if [[ -n "$USDB_INDEXER_PID" ]]; then
    if ! regtest_assert_managed_process_alive "usdb-indexer" "$USDB_INDEXER_PID"; then
      USDB_INDEXER_PID=""
      return 1
    fi
    regtest_log "Stopping usdb-indexer process pid=${USDB_INDEXER_PID}"
    curl -s --connect-timeout "$CURL_CONNECT_TIMEOUT_SEC" --max-time "$CURL_MAX_TIME_SEC" \
      -X POST "http://127.0.0.1:${USDB_INDEXER_RPC_PORT}" \
      -H 'content-type: application/json' \
      --data '{"jsonrpc":"2.0","id":1,"method":"stop","params":[]}' >/dev/null 2>&1 || true
    regtest_stop_process "$USDB_INDEXER_PID"
  fi
  USDB_INDEXER_PID=""
}

regtest_crash_balance_history() {
  if [[ -n "$BALANCE_HISTORY_PID" ]]; then
    if ! regtest_assert_managed_process_alive "balance-history" "$BALANCE_HISTORY_PID"; then
      BALANCE_HISTORY_PID=""
      return 1
    fi
    regtest_log "Crashing balance-history process pid=${BALANCE_HISTORY_PID}"
    kill -9 "$BALANCE_HISTORY_PID" >/dev/null 2>&1 || true
    wait "$BALANCE_HISTORY_PID" >/dev/null 2>&1 || true
  fi
  BALANCE_HISTORY_PID=""
}

regtest_crash_usdb_indexer() {
  if [[ -n "$USDB_INDEXER_PID" ]]; then
    if ! regtest_assert_managed_process_alive "usdb-indexer" "$USDB_INDEXER_PID"; then
      USDB_INDEXER_PID=""
      return 1
    fi
    regtest_log "Crashing usdb-indexer process pid=${USDB_INDEXER_PID}"
    kill -9 "$USDB_INDEXER_PID" >/dev/null 2>&1 || true
    wait "$USDB_INDEXER_PID" >/dev/null 2>&1 || true
  fi
  USDB_INDEXER_PID=""
}

regtest_stop_ord_server() {
  local failed=0

  if [[ -n "$ORD_SERVER_PID" ]]; then
    if regtest_assert_managed_process_alive "ord-server" "$ORD_SERVER_PID"; then
      regtest_log "Stopping ord server process pid=${ORD_SERVER_PID}"
      regtest_stop_process "$ORD_SERVER_PID"
    else
      failed=1
    fi
  fi

  # Be defensive: ord server may outlive the original background shell process
  # or respawn under a different pid. Sweep any process still bound to this
  # workspace/data-dir so repeated regtests do not leak regtest ord servers.
  if [[ -n "${ORD_DATA_DIR:-}" ]] && [[ -n "${ORD_RPC_PORT:-}" ]]; then
    while IFS= read -r pid; do
      if [[ -n "$pid" ]] && kill -0 "$pid" 2>/dev/null; then
        regtest_log "Stopping residual ord server process pid=${pid} for data_dir=${ORD_DATA_DIR}, http_port=${ORD_RPC_PORT}"
        regtest_stop_process "$pid"
      fi
    done < <(
      ps -eo pid=,args= | awk -v data_dir="$ORD_DATA_DIR" -v http_port="$ORD_RPC_PORT" '
        index($0, "/ord ") && index($0, " server ") && index($0, " --data-dir " data_dir) && index($0, " --http-port " http_port) {
          print $1
        }
      '
    )
  fi
  ORD_SERVER_PID=""
  return "$failed"
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
  if [[ -n "$BITCOIND_PID" ]] && ! regtest_assert_managed_process_alive "bitcoind" "$BITCOIND_PID"; then
    BITCOIND_PID=""
    return 1
  fi
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

regtest_get_usdb_pass_snapshot_response() {
  local inscription_id="$1"
  local block_height="$2"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "[{\"inscription_id\":\"${inscription_id}\",\"at_height\":${block_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "data.get('result') is not None" "True"
  echo "$resp"
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
  local expected_raw_energy="${5:-}"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${inscription_id}\",\"block_height\":${block_height},\"mode\":\"${mode}\"}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('inscription_id')" "$inscription_id"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('query_block_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('state')" "$expected_state"
  if [[ -n "$expected_raw_energy" ]]; then
    regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('raw_energy')" "$expected_raw_energy"
  fi
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

regtest_assert_usdb_miner_economic_aggregate_positive() {
  local block_height="$1"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_miner_economic_aggregate" "[{\"view_version\":\"uip-0006-usdb-economic-state-view:v1\",\"block_height\":${block_height},\"context\":null}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('external_state') or {}).get('btc_height')" "$block_height"
  regtest_assert_json_expr "$resp" "int(((data.get('result') or {}).get('miner_aggregate') or {}).get('active_miner_owner_count', 0)) > 0" "True"
  regtest_assert_json_expr "$resp" "int(((data.get('result') or {}).get('miner_aggregate') or {}).get('total_miner_btc_sats', '0')) > 0" "True"
}

regtest_assert_usdb_miner_economic_aggregate_zero() {
  local block_height="$1"
  local resp
  resp="$(regtest_rpc_call_usdb_indexer "get_miner_economic_aggregate" "[{\"view_version\":\"uip-0006-usdb-economic-state-view:v1\",\"block_height\":${block_height},\"context\":null}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('external_state') or {}).get('btc_height')" "$block_height"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('miner_aggregate') or {}).get('total_miner_btc_sats')" "0"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('miner_aggregate') or {}).get('active_miner_owner_count')" "0"
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

  regtest_log "Failure diagnostics: exit_code=${exit_code}, work_dir=${WORK_DIR}, btc_rpc_port=${BTC_RPC_PORT}, btc_p2p_port=${BTC_P2P_PORT}, bh_rpc_port=${BH_RPC_PORT}, usdb_indexer_rpc_port=${USDB_INDEXER_RPC_PORT}"
  regtest_print_tail_if_exists "ord server log" "$ORD_SERVER_LOG_FILE"
  regtest_print_tail_if_exists "balance-history log" "$BALANCE_HISTORY_LOG_FILE"
  regtest_print_tail_if_exists "usdb-indexer log" "$USDB_INDEXER_LOG_FILE"
  regtest_print_tail_if_exists "balance-history service log" "${BALANCE_HISTORY_ROOT}/logs/balance-history_rCURRENT.log"
  regtest_print_tail_if_exists "bitcoind debug log" "${BITCOIN_DIR}/regtest/debug.log"
}

regtest_cleanup() {
  local exit_code=$?
  local cleanup_failed=0
  local final_exit_code
  if [[ $# -gt 0 ]]; then
    exit_code="$1"
  fi
  set +e

  if [[ "$exit_code" -eq 0 ]]; then
    regtest_assert_managed_processes_alive || cleanup_failed=1
  fi
  regtest_stop_usdb_indexer || cleanup_failed=1
  regtest_stop_balance_history || cleanup_failed=1
  regtest_stop_ord_server || cleanup_failed=1
  regtest_stop_bitcoind || cleanup_failed=1
  if [[ "$exit_code" -eq 0 ]]; then
    regtest_assert_managed_logs_clean || cleanup_failed=1
  fi

  final_exit_code="$exit_code"
  if [[ "$final_exit_code" -eq 0 ]] && [[ "$cleanup_failed" -ne 0 ]]; then
    final_exit_code=1
  fi
  if [[ "$final_exit_code" -ne 0 ]]; then
    regtest_print_failure_diagnostics "$final_exit_code"
  fi

  # EXIT traps preserve the pre-trap status unless the trap exits explicitly.
  # Disable recursion so cleanup-detected process failures cannot be reported as success.
  trap - EXIT
  exit "$final_exit_code"
}
