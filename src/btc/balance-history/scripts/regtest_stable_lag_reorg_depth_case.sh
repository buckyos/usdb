#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-stable-lag-depth-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ONLINE_ROOT="${ONLINE_ROOT:-$WORK_DIR/online}"
OFFLINE_ROOT="${OFFLINE_ROOT:-$WORK_DIR/offline}"
JOINER_ROOT="${JOINER_ROOT:-$WORK_DIR/joiner}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30732}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30733}"
ONLINE_RPC_PORT="${ONLINE_RPC_PORT:-30710}"
OFFLINE_RPC_PORT="${OFFLINE_RPC_PORT:-30711}"
JOINER_RPC_PORT="${JOINER_RPC_PORT:-30712}"
WALLET_NAME="${WALLET_NAME:-bhstablelagdepth}"
EXPECTED_STABLE_LAG="${EXPECTED_STABLE_LAG:-5}"
REORG_DEPTH="${REORG_DEPTH:-4}"
PREFIX_BLOCKS="${PREFIX_BLOCKS:-3}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
ONLINE_LOG="${ONLINE_LOG:-$WORK_DIR/online.log}"
OFFLINE_LOG="${OFFLINE_LOG:-$WORK_DIR/offline.log}"
JOINER_LOG="${JOINER_LOG:-$WORK_DIR/joiner.log}"
REGTEST_LOG_PREFIX="[stable-lag-depth-${REORG_DEPTH}]"
export REPO_ROOT REGTEST_LOG_PREFIX

ONLINE_PID=""
OFFLINE_PID=""
JOINER_PID=""
declare -A CAPTURED_STATE=()

# shellcheck source=regtest_lib.sh
source "${SCRIPT_DIR}/regtest_lib.sh"

canonical_rpc_result_at_port() {
  local rpc_port="$1"
  local method="$2"
  local params="${3:-[]}"

  regtest_rpc_call_balance_history_at_port "$rpc_port" "$method" "$params" \
    | regtest_json_extract_python 'import json,sys
d=json.load(sys.stdin)
if d.get("error"):
    raise SystemExit("RPC error: " + json.dumps(d["error"], sort_keys=True))
if "result" not in d:
    raise SystemExit("RPC response is missing result")
print(json.dumps(d["result"], sort_keys=True, separators=(",", ":")))'
}

start_instance() {
  local root_dir="$1"
  local rpc_port="$2"
  local log_file="$3"
  local label="$4"

  regtest_start_balance_history_instance "$root_dir" "$rpc_port" "$log_file" "$label"
  regtest_wait_balance_history_rpc_ready_at_port "$rpc_port" "$log_file" "$label"
}

stop_instance() {
  local pid="$1"
  local rpc_port="$2"
  local label="$3"

  regtest_stop_balance_history_instance "$pid" "$rpc_port" "$label"
}

wait_for_instance() {
  local rpc_port="$1"
  local log_file="$2"
  local label="$3"
  local stable_height="$4"
  local stable_hash="$5"

  regtest_wait_until_block_commit_hash_at_port \
    "$rpc_port" "$stable_height" "$stable_hash" "$log_file" "$label"
  regtest_wait_balance_history_consensus_ready_at_port "$rpc_port" "$log_file" "$label"
}

assert_equal() {
  local label="$1"
  local expected="$2"
  local actual="$3"

  if [[ "$actual" != "$expected" ]]; then
    regtest_log "${label} mismatch"
    regtest_log "expected=${expected}"
    regtest_log "actual=${actual}"
    exit 1
  fi
}

assert_not_equal() {
  local label="$1"
  local unexpected="$2"
  local actual="$3"

  if [[ "$actual" == "$unexpected" ]]; then
    regtest_log "${label} unexpectedly remained unchanged: ${actual}"
    exit 1
  fi
}

capture_instance_state() {
  local rpc_port="$1"
  local stable_height="$2"
  local receiver_address="$3"
  local output_prefix="$4"
  local snapshot state_ref block_commit balance

  snapshot="$(canonical_rpc_result_at_port "$rpc_port" "get_snapshot_info" "[]")"
  state_ref="$(
    canonical_rpc_result_at_port \
      "$rpc_port" \
      "get_state_ref_at_height" \
      "[{\"block_height\":${stable_height},\"context\":null}]"
  )"
  block_commit="$(
    canonical_rpc_result_at_port "$rpc_port" "get_block_commit" "[${stable_height}]"
  )"
  balance="$(
    regtest_get_address_balance_sat_at_port "$rpc_port" "$receiver_address" "$stable_height"
  )"

  CAPTURED_STATE["${output_prefix}.snapshot"]="$snapshot"
  CAPTURED_STATE["${output_prefix}.state_ref"]="$state_ref"
  CAPTURED_STATE["${output_prefix}.block_commit"]="$block_commit"
  CAPTURED_STATE["${output_prefix}.balance"]="$balance"
}

assert_instance_matches() {
  local label="$1"
  local expected_prefix="$2"
  local actual_prefix="$3"

  assert_equal \
    "${label} snapshot" \
    "${CAPTURED_STATE["${expected_prefix}.snapshot"]}" \
    "${CAPTURED_STATE["${actual_prefix}.snapshot"]}"
  assert_equal \
    "${label} state-ref" \
    "${CAPTURED_STATE["${expected_prefix}.state_ref"]}" \
    "${CAPTURED_STATE["${actual_prefix}.state_ref"]}"
  assert_equal \
    "${label} block commit" \
    "${CAPTURED_STATE["${expected_prefix}.block_commit"]}" \
    "${CAPTURED_STATE["${actual_prefix}.block_commit"]}"
  assert_equal \
    "${label} receiver balance" \
    "${CAPTURED_STATE["${expected_prefix}.balance"]}" \
    "${CAPTURED_STATE["${actual_prefix}.balance"]}"
}

print_case_diagnostics() {
  regtest_print_tail_if_exists "online stdout log" "$ONLINE_LOG"
  regtest_print_tail_if_exists "online service log" "${ONLINE_ROOT}/logs/balance-history_rCURRENT.log"
  regtest_print_tail_if_exists "offline stdout log" "$OFFLINE_LOG"
  regtest_print_tail_if_exists "offline service log" "${OFFLINE_ROOT}/logs/balance-history_rCURRENT.log"
  regtest_print_tail_if_exists "joiner stdout log" "$JOINER_LOG"
  regtest_print_tail_if_exists "joiner service log" "${JOINER_ROOT}/logs/balance-history_rCURRENT.log"
  regtest_print_tail_if_exists "bitcoind debug log" "${BITCOIN_DIR}/regtest/debug.log"
}

cleanup() {
  local exit_code=$?
  set +e

  if [[ "$exit_code" -ne 0 ]]; then
    regtest_log "Case failed: exit=${exit_code}, work_dir=${WORK_DIR}"
    print_case_diagnostics
  fi

  stop_instance "$ONLINE_PID" "$ONLINE_RPC_PORT" "online"
  stop_instance "$OFFLINE_PID" "$OFFLINE_RPC_PORT" "offline-restart"
  stop_instance "$JOINER_PID" "$JOINER_RPC_PORT" "fresh-joiner"
  regtest_stop_bitcoind
}

validate_case_parameters() {
  if (( EXPECTED_STABLE_LAG <= 1 )); then
    regtest_log "EXPECTED_STABLE_LAG must be greater than 1"
    exit 1
  fi
  if (( PREFIX_BLOCKS <= 0 )); then
    regtest_log "PREFIX_BLOCKS must be positive"
    exit 1
  fi
  if (( REORG_DEPTH < EXPECTED_STABLE_LAG - 1 || REORG_DEPTH > EXPECTED_STABLE_LAG + 1 )); then
    regtest_log "REORG_DEPTH must be stable_lag - 1, stable_lag, or stable_lag + 1"
    exit 1
  fi
}

main() {
  trap cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  validate_case_parameters

  mkdir -p "$WORK_DIR" "$BITCOIN_DIR" "$ONLINE_ROOT" "$OFFLINE_ROOT" "$JOINER_ROOT"
  regtest_log "Workspace directory: ${WORK_DIR}"

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address replacement_address receiver_address tracked_txid expected_balance_sat
  local stable_height tip_height affected_height
  local original_stable_hash original_affected_hash replacement_stable_hash replacement_tip_hash
  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  regtest_log "Mining ${PREFIX_BLOCKS} prefix blocks"
  regtest_mine_blocks "$PREFIX_BLOCKS" "$mining_address"

  receiver_address="$(regtest_get_new_address)"
  expected_balance_sat="$(regtest_btc_amount_to_sat "$SEND_AMOUNT_BTC")"
  tracked_txid="$(
    "$BITCOIN_CLI_BIN" \
      -regtest \
      -datadir="$BITCOIN_DIR" \
      -rpcport="$BTC_RPC_PORT" \
      -rpcwallet="$WALLET_NAME" \
      sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC"
  )"
  regtest_log "Created stable-frontier transfer txid=${tracked_txid}, amount_sat=${expected_balance_sat}"
  regtest_mine_blocks 1 "$mining_address"
  stable_height="$(
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount
  )"
  original_stable_hash="$(regtest_get_block_hash_by_height "$stable_height")"

  regtest_mine_blocks "$EXPECTED_STABLE_LAG" "$mining_address"
  tip_height="$(
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount
  )"
  assert_equal "BTC stable frontier" "$stable_height" "$((tip_height - EXPECTED_STABLE_LAG))"

  regtest_create_balance_history_config_at "$ONLINE_ROOT" "$ONLINE_RPC_PORT"
  regtest_create_balance_history_config_at "$OFFLINE_ROOT" "$OFFLINE_RPC_PORT"

  start_instance "$ONLINE_ROOT" "$ONLINE_RPC_PORT" "$ONLINE_LOG" "online"
  ONLINE_PID="$REGTEST_LAST_BALANCE_HISTORY_PID"
  wait_for_instance \
    "$ONLINE_RPC_PORT" "$ONLINE_LOG" "online" "$stable_height" "$original_stable_hash"

  start_instance "$OFFLINE_ROOT" "$OFFLINE_RPC_PORT" "$OFFLINE_LOG" "offline-baseline"
  OFFLINE_PID="$REGTEST_LAST_BALANCE_HISTORY_PID"
  wait_for_instance \
    "$OFFLINE_RPC_PORT" "$OFFLINE_LOG" "offline-baseline" "$stable_height" "$original_stable_hash"

  capture_instance_state "$ONLINE_RPC_PORT" "$stable_height" "$receiver_address" "baseline"
  capture_instance_state \
    "$OFFLINE_RPC_PORT" "$stable_height" "$receiver_address" "offline_before"
  assert_instance_matches "pre-reorg online/offline" "baseline" "offline_before"
  assert_equal \
    "pre-reorg receiver balance" \
    "$expected_balance_sat" \
    "${CAPTURED_STATE["baseline.balance"]}"

  stop_instance "$OFFLINE_PID" "$OFFLINE_RPC_PORT" "offline-baseline"
  OFFLINE_PID=""

  affected_height=$((tip_height - REORG_DEPTH + 1))
  original_affected_hash="$(regtest_get_block_hash_by_height "$affected_height")"
  regtest_log "Replacing BTC heights ${affected_height}-${tip_height}; stable_height=${stable_height}"
  "$BITCOIN_CLI_BIN" \
    -regtest \
    -datadir="$BITCOIN_DIR" \
    -rpcport="$BTC_RPC_PORT" \
    invalidateblock "$original_affected_hash"
  replacement_address="$(regtest_get_new_address)"
  for _ in $(seq 1 "$REORG_DEPTH"); do
    regtest_mine_empty_block "$replacement_address"
  done

  assert_equal \
    "replacement BTC tip height" \
    "$tip_height" \
    "$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  replacement_stable_hash="$(regtest_get_block_hash_by_height "$stable_height")"
  replacement_tip_hash="$(regtest_get_block_hash_by_height "$tip_height")"
  regtest_log "Replacement tip hash=${replacement_tip_hash}, stable hash=${replacement_stable_hash}"

  local expected_balance
  if (( REORG_DEPTH <= EXPECTED_STABLE_LAG )); then
    assert_equal \
      "stable hash for reorg within lag boundary" \
      "$original_stable_hash" \
      "$replacement_stable_hash"
    expected_balance="$expected_balance_sat"
  else
    assert_not_equal \
      "stable hash for reorg beyond lag boundary" \
      "$original_stable_hash" \
      "$replacement_stable_hash"
    expected_balance="0"
  fi

  wait_for_instance \
    "$ONLINE_RPC_PORT" "$ONLINE_LOG" "online" "$stable_height" "$replacement_stable_hash"

  capture_instance_state "$ONLINE_RPC_PORT" "$stable_height" "$receiver_address" "online_after"
  assert_equal \
    "online receiver balance" \
    "$expected_balance" \
    "${CAPTURED_STATE["online_after.balance"]}"
  if (( REORG_DEPTH <= EXPECTED_STABLE_LAG )); then
    assert_instance_matches "within-lag online state" "baseline" "online_after"
  else
    assert_not_equal \
      "beyond-lag online snapshot" \
      "${CAPTURED_STATE["baseline.snapshot"]}" \
      "${CAPTURED_STATE["online_after.snapshot"]}"
    assert_not_equal \
      "beyond-lag online state-ref" \
      "${CAPTURED_STATE["baseline.state_ref"]}" \
      "${CAPTURED_STATE["online_after.state_ref"]}"
    assert_not_equal \
      "beyond-lag online block commit" \
      "${CAPTURED_STATE["baseline.block_commit"]}" \
      "${CAPTURED_STATE["online_after.block_commit"]}"
  fi

  stop_instance "$ONLINE_PID" "$ONLINE_RPC_PORT" "online"
  ONLINE_PID=""

  start_instance "$OFFLINE_ROOT" "$OFFLINE_RPC_PORT" "$OFFLINE_LOG" "offline-restart"
  OFFLINE_PID="$REGTEST_LAST_BALANCE_HISTORY_PID"
  wait_for_instance \
    "$OFFLINE_RPC_PORT" "$OFFLINE_LOG" "offline-restart" "$stable_height" "$replacement_stable_hash"

  capture_instance_state \
    "$OFFLINE_RPC_PORT" "$stable_height" "$receiver_address" "offline_after"
  assert_instance_matches "offline restart convergence" "online_after" "offline_after"
  stop_instance "$OFFLINE_PID" "$OFFLINE_RPC_PORT" "offline-restart"
  OFFLINE_PID=""

  regtest_create_balance_history_config_at "$JOINER_ROOT" "$JOINER_RPC_PORT"
  start_instance "$JOINER_ROOT" "$JOINER_RPC_PORT" "$JOINER_LOG" "fresh-joiner"
  JOINER_PID="$REGTEST_LAST_BALANCE_HISTORY_PID"
  wait_for_instance \
    "$JOINER_RPC_PORT" "$JOINER_LOG" "fresh-joiner" "$stable_height" "$replacement_stable_hash"

  capture_instance_state \
    "$JOINER_RPC_PORT" "$stable_height" "$receiver_address" "joiner_after"
  assert_instance_matches "fresh joiner convergence" "online_after" "joiner_after"

  regtest_log "Stable-lag reorg depth case succeeded."
  regtest_log "depth=${REORG_DEPTH}, lag=${EXPECTED_STABLE_LAG}, stable_height=${stable_height}"
  regtest_log "online/offline-restart/fresh-joiner state identities match."
}

main "$@"
