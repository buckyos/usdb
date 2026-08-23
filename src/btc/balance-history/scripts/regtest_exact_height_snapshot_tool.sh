#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-exact-snapshot-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/config-source}"
SNAPSHOT_BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-$WORK_DIR/snapshot-builder}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29832}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29833}"
BH_RPC_PORT="${BH_RPC_PORT:-29810}"
WALLET_NAME="${WALLET_NAME:-bhexactsnapshot}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
export REGTEST_LOG_PREFIX="[exact-height-snapshot]"

# SCRIPT_DIR is resolved from this file at runtime.
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/regtest_lib.sh"

run_snapshot_tool() {
  (
    cd "$REPO_ROOT"
    cargo run --quiet --manifest-path src/btc/Cargo.toml \
      -p balance-history-snapshot-tool -- \
      --root-dir "$SNAPSHOT_BUILDER_ROOT" \
      --json \
      "$@"
  )
}

assert_json_file() {
  local file="$1"
  local expression="$2"
  local expected="$3"
  local actual

  actual="$(python3 - "$file" "$expression" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    data = json.load(source)
print(eval(sys.argv[2], {"__builtins__": {}}, {"data": data}))
PY
)"
  regtest_log "JSON assertion: file=${file}, expr=${expression}, expected=${expected}, actual=${actual}"
  if [[ "$actual" != "$expected" ]]; then
    exit 1
  fi
}

assert_json_files_equal() {
  local left_file="$1"
  local right_file="$2"
  local field="$3"

  python3 - "$left_file" "$right_file" "$field" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    left = json.load(source)
with open(sys.argv[2], encoding="utf-8") as source:
    right = json.load(source)
field = sys.argv[3]
if left[field] != right[field]:
    raise SystemExit(f"JSON field mismatch for {field}: {left[field]!r} != {right[field]!r}")
PY
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd python3

  regtest_ensure_workspace_dirs
  mkdir -p "$SNAPSHOT_BUILDER_ROOT"
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address tip target_h target_h1 hash_h hash_h1
  local report_h report_h_repeat report_h1 verify_h verify_h1 status_file

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  tip="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  target_h=$((tip - 6))
  target_h1=$((target_h + 1))
  hash_h="$(regtest_get_block_hash_by_height "$target_h")"
  hash_h1="$(regtest_get_block_hash_by_height "$target_h1")"

  regtest_create_balance_history_config_at "$BALANCE_HISTORY_ROOT" "$BH_RPC_PORT"

  report_h="$WORK_DIR/create-${target_h}.json"
  report_h_repeat="$WORK_DIR/create-${target_h}-repeat.json"
  report_h1="$WORK_DIR/create-${target_h1}.json"
  verify_h="$WORK_DIR/verify-${target_h}.json"
  verify_h1="$WORK_DIR/verify-${target_h1}.json"
  status_file="$WORK_DIR/status.json"

  regtest_log "Creating initial exact-height snapshot height=${target_h}, hash=${hash_h}"
  run_snapshot_tool create \
    --height "$target_h" \
    --expected-block-hash "$hash_h" \
    --config "$BALANCE_HISTORY_ROOT/config.toml" \
    --poll-interval-secs 1 >"$report_h"
  assert_json_file "$report_h" "data['height']" "$target_h"
  assert_json_file "$report_h" "data['btc_block_hash']" "$hash_h"
  assert_json_file "$report_h" "data['already_complete']" "False"
  assert_json_file "$report_h" "data['utxo_count'] > 0" "True"

  regtest_log "Replaying completed target height=${target_h}"
  run_snapshot_tool create \
    --height "$target_h" \
    --expected-block-hash "$hash_h" \
    --poll-interval-secs 1 >"$report_h_repeat"
  assert_json_file "$report_h_repeat" "data['already_complete']" "True"
  assert_json_files_equal "$report_h" "$report_h_repeat" "file_sha256"

  run_snapshot_tool verify --height "$target_h" --block-hash "$hash_h" >"$verify_h"
  assert_json_file "$verify_h" "data['height']" "$target_h"
  assert_json_file "$verify_h" "data['utxo_count'] > 0" "True"

  regtest_log "Incrementally creating next exact-height snapshot height=${target_h1}, hash=${hash_h1}"
  run_snapshot_tool create \
    --height "$target_h1" \
    --expected-block-hash "$hash_h1" \
    --poll-interval-secs 1 >"$report_h1"
  assert_json_file "$report_h1" "data['height']" "$target_h1"
  assert_json_file "$report_h1" "data['btc_block_hash']" "$hash_h1"
  assert_json_file "$report_h1" "data['already_complete']" "False"

  run_snapshot_tool verify --height "$target_h1" --block-hash "$hash_h1" >"$verify_h1"
  assert_json_file "$verify_h1" "data['height']" "$target_h1"

  run_snapshot_tool status >"$status_file"
  assert_json_file "$status_file" "data['state']['latest_completed']['height']" "$target_h1"
  assert_json_file "$status_file" "data['state']['active_job_height'] is None" "True"

  regtest_log "Exact-height snapshot tool test succeeded. Artifacts: ${SNAPSHOT_BUILDER_ROOT}/snapshots"
}

main "$@"
