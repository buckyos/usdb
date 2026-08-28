#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-stable-lag-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29832}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29833}"
BH_RPC_PORT="${BH_RPC_PORT:-29810}"
WALLET_NAME="${WALLET_NAME:-bhstablelag}"
PRE_LAG_TIP_HEIGHT="${PRE_LAG_TIP_HEIGHT:-3}"
TARGET_TIP_HEIGHT="${TARGET_TIP_HEIGHT:-20}"
EXTRA_BLOCKS="${EXTRA_BLOCKS:-3}"
EXPECTED_STABLE_LAG="${EXPECTED_STABLE_LAG:-10}"
REORG_DEPTH="${REORG_DEPTH:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[stable-lag-smoke]"
export REPO_ROOT REGTEST_LOG_PREFIX

# shellcheck source=regtest_lib.sh
source "${SCRIPT_DIR}/regtest_lib.sh"

canonical_rpc_result() {
  local method="$1"
  local params="${2:-[]}"

  regtest_rpc_call_balance_history "$method" "$params" \
    | regtest_json_extract_python 'import json,sys
d=json.load(sys.stdin)
if d.get("error"):
    raise SystemExit("RPC error: " + json.dumps(d["error"], sort_keys=True))
print(json.dumps(d.get("result"), sort_keys=True, separators=(",", ":")))'
}

assert_tip_below_lag_not_ready() {
  local expected_tip_height="$1"
  local expected_stable_lag="$2"
  local actual_tip actual_height snapshot_resp snapshot_code snapshot_message
  local state_ref_resp state_ref_code state_ref_message readiness_resp readiness_summary

  actual_tip="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  actual_height="$(regtest_get_balance_history_height)"
  snapshot_resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  snapshot_code="$(printf '%s' "$snapshot_resp" | regtest_json_extract_python 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("code", ""))')"
  snapshot_message="$(printf '%s' "$snapshot_resp" | regtest_json_extract_python 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("message", ""))')"
  state_ref_resp="$(regtest_rpc_call_balance_history "get_state_ref_at_height" '[{"block_height":0}]')"
  state_ref_code="$(printf '%s' "$state_ref_resp" | regtest_json_extract_python 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("code", ""))')"
  state_ref_message="$(printf '%s' "$state_ref_resp" | regtest_json_extract_python 'import json,sys; print((json.load(sys.stdin).get("error") or {}).get("message", ""))')"
  readiness_resp="$(regtest_rpc_call_balance_history "get_readiness" "[]")"
  readiness_summary="$(printf '%s' "$readiness_resp" | regtest_json_extract_python 'import json,sys
r=(json.load(sys.stdin).get("result") or {})
print("|".join([
    "1" if r.get("consensus_ready") else "0",
    str(r.get("stable_height", "")),
    str(r.get("stable_block_hash") or ""),
    str(r.get("latest_block_commit") or ""),
]))')"

  regtest_log "Below-lag assertion: tip=${actual_tip}, expected_tip=${expected_tip_height}, stable_height=${actual_height}, stable_lag=${expected_stable_lag}, readiness=${readiness_summary}"
  if [[ "$actual_tip" != "$expected_tip_height" ]]; then
    regtest_log "BTC tip mismatch below stable lag: expected=${expected_tip_height}, got=${actual_tip}"
    exit 1
  fi
  if [[ "$actual_height" != "0" ]]; then
    regtest_log "Stable height must saturate to zero below lag: got=${actual_height}"
    exit 1
  fi
  if [[ "$snapshot_code" != "-32041" || "$snapshot_message" != "SNAPSHOT_NOT_READY" ]]; then
    regtest_log "Expected get_snapshot_info SNAPSHOT_NOT_READY below lag: ${snapshot_resp}"
    exit 1
  fi
  if [[ "$state_ref_code" != "-32041" || "$state_ref_message" != "SNAPSHOT_NOT_READY" ]]; then
    regtest_log "Expected get_state_ref_at_height SNAPSHOT_NOT_READY below lag: ${state_ref_resp}"
    exit 1
  fi
  if [[ "$readiness_summary" != "0|0||" ]]; then
    regtest_log "Consensus readiness must fail closed below lag: ${readiness_resp}"
    exit 1
  fi
}

assert_snapshot_matches_expected() {
  local expected_tip_height="$1"
  local expected_stable_height="$2"
  local expected_stable_lag="$3"
  local actual_height snapshot_height snapshot_lag snapshot_hash expected_hash

  actual_height="$(regtest_get_balance_history_height)"
  snapshot_height="$(regtest_get_snapshot_stable_height)"
  snapshot_lag="$(regtest_get_snapshot_stable_lag)"
  snapshot_hash="$(regtest_get_snapshot_stable_hash)"
  expected_hash="$(regtest_get_block_hash_by_height "$expected_stable_height")"

  regtest_log "Stable-lag assertion: tip=${expected_tip_height}, expected_stable=${expected_stable_height}, rpc_height=${actual_height}, snapshot_height=${snapshot_height}, snapshot_lag=${snapshot_lag}"

  if [[ "$actual_height" != "$expected_stable_height" ]]; then
    regtest_log "Stable height mismatch: expected=${expected_stable_height}, got=${actual_height}"
    exit 1
  fi

  if [[ "$snapshot_height" != "$expected_stable_height" ]]; then
    regtest_log "Snapshot stable_height mismatch: expected=${expected_stable_height}, got=${snapshot_height}"
    exit 1
  fi

  if [[ "$snapshot_lag" != "$expected_stable_lag" ]]; then
    regtest_log "Snapshot stable_lag mismatch: expected=${expected_stable_lag}, got=${snapshot_lag}"
    exit 1
  fi

  if [[ "$snapshot_hash" != "$expected_hash" ]]; then
    regtest_log "Snapshot stable_block_hash mismatch: expected=${expected_hash}, got=${snapshot_hash}"
    exit 1
  fi

}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  regtest_ensure_workspace_dirs

  if (( PRE_LAG_TIP_HEIGHT <= 0 || PRE_LAG_TIP_HEIGHT >= EXPECTED_STABLE_LAG )); then
    regtest_log "PRE_LAG_TIP_HEIGHT must be positive and strictly less than EXPECTED_STABLE_LAG"
    exit 1
  fi
  if (( TARGET_TIP_HEIGHT <= EXPECTED_STABLE_LAG )); then
    regtest_log "TARGET_TIP_HEIGHT must be greater than EXPECTED_STABLE_LAG"
    exit 1
  fi
  if (( EXTRA_BLOCKS < REORG_DEPTH )); then
    regtest_log "EXTRA_BLOCKS must be at least REORG_DEPTH so the replacement branch reaches the stable frontier"
    exit 1
  fi

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address
  mining_address="$(regtest_get_new_address)"
  regtest_log "Mining ${PRE_LAG_TIP_HEIGHT} blocks below stable lag to address=${mining_address}"
  regtest_mine_blocks "$PRE_LAG_TIP_HEIGHT" "$mining_address"

  regtest_create_balance_history_config

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  assert_tip_below_lag_not_ready "$PRE_LAG_TIP_HEIGHT" "$EXPECTED_STABLE_LAG"

  regtest_log "Restarting balance-history while BTC tip remains below stable lag"
  regtest_restart_balance_history
  assert_tip_below_lag_not_ready "$PRE_LAG_TIP_HEIGHT" "$EXPECTED_STABLE_LAG"

  local catch_up_blocks
  catch_up_blocks=$((TARGET_TIP_HEIGHT - PRE_LAG_TIP_HEIGHT))
  regtest_log "Mining ${catch_up_blocks} blocks so balance-history catches up to a non-zero stable frontier"
  regtest_mine_blocks "$catch_up_blocks" "$mining_address"

  local stable_lag expected_stable_height
  expected_stable_height=$((TARGET_TIP_HEIGHT - EXPECTED_STABLE_LAG))
  regtest_wait_until_synced_height "$expected_stable_height"
  regtest_wait_balance_history_consensus_ready
  stable_lag="$(regtest_get_snapshot_stable_lag)"
  regtest_log "Observed protocol stable_lag=${stable_lag}"
  if [[ "$stable_lag" != "$EXPECTED_STABLE_LAG" ]]; then
    regtest_log "Protocol stable_lag mismatch: expected=${EXPECTED_STABLE_LAG}, got=${stable_lag}"
    exit 1
  fi
  if (( REORG_DEPTH <= 0 || REORG_DEPTH >= stable_lag )); then
    regtest_log "REORG_DEPTH must be positive and strictly less than stable_lag"
    exit 1
  fi
  expected_stable_height=$((TARGET_TIP_HEIGHT - stable_lag))
  assert_snapshot_matches_expected "$TARGET_TIP_HEIGHT" "$expected_stable_height" "$stable_lag"

  local initial_snapshot initial_state_ref restarted_snapshot restarted_state_ref
  initial_snapshot="$(canonical_rpc_result "get_snapshot_info" "[]")"
  initial_state_ref="$(canonical_rpc_result "get_state_ref_at_height" "[{\"block_height\":${expected_stable_height}}]")"

  regtest_log "Restarting balance-history and replaying snapshot/state-ref from the same database"
  regtest_restart_balance_history
  regtest_wait_until_synced_height "$expected_stable_height"
  regtest_wait_balance_history_consensus_ready
  assert_snapshot_matches_expected "$TARGET_TIP_HEIGHT" "$expected_stable_height" "$stable_lag"
  restarted_snapshot="$(canonical_rpc_result "get_snapshot_info" "[]")"
  restarted_state_ref="$(canonical_rpc_result "get_state_ref_at_height" "[{\"block_height\":${expected_stable_height}}]")"
  if [[ "$restarted_snapshot" != "$initial_snapshot" ]]; then
    regtest_log "Snapshot identity changed across clean restart"
    exit 1
  fi
  if [[ "$restarted_state_ref" != "$initial_state_ref" ]]; then
    regtest_log "Historical state ref changed across clean restart"
    exit 1
  fi

  local replaced_height original_replaced_hash replacement_hash
  replaced_height=$((TARGET_TIP_HEIGHT - REORG_DEPTH + 1))
  original_replaced_hash="$(regtest_get_block_hash_by_height "$replaced_height")"
  regtest_log "Replacing BTC heights ${replaced_height}-${TARGET_TIP_HEIGHT} while they remain above the stable frontier"
  regtest_stop_balance_history
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    invalidateblock "$original_replaced_hash"
  regtest_mine_blocks "$REORG_DEPTH" "$mining_address"
  replacement_hash="$(regtest_get_block_hash_by_height "$replaced_height")"
  if [[ "$replacement_hash" == "$original_replaced_hash" ]]; then
    regtest_log "Expected replacement hash at height=${replaced_height}, but canonical hash did not change"
    exit 1
  fi

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$expected_stable_height"
  regtest_wait_balance_history_consensus_ready
  assert_snapshot_matches_expected "$TARGET_TIP_HEIGHT" "$expected_stable_height" "$stable_lag"
  restarted_snapshot="$(canonical_rpc_result "get_snapshot_info" "[]")"
  restarted_state_ref="$(canonical_rpc_result "get_state_ref_at_height" "[{\"block_height\":${expected_stable_height}}]")"
  if [[ "$restarted_snapshot" != "$initial_snapshot" ]]; then
    regtest_log "Stable snapshot changed after lag-window branch replacement"
    exit 1
  fi
  if [[ "$restarted_state_ref" != "$initial_state_ref" ]]; then
    regtest_log "Historical state ref changed after lag-window branch replacement"
    exit 1
  fi

  regtest_log "Mining ${EXTRA_BLOCKS} extra blocks so the replacement branch enters the stable view"
  regtest_mine_blocks "$EXTRA_BLOCKS" "$mining_address"

  local new_tip_height new_stable_height
  new_tip_height=$((TARGET_TIP_HEIGHT + EXTRA_BLOCKS))
  new_stable_height=$((new_tip_height - stable_lag))
  regtest_wait_until_synced_height "$new_stable_height"
  regtest_wait_balance_history_consensus_ready
  assert_snapshot_matches_expected "$new_tip_height" "$new_stable_height" "$stable_lag"
  restarted_state_ref="$(canonical_rpc_result "get_state_ref_at_height" "[{\"block_height\":${expected_stable_height}}]")"
  if [[ "$restarted_state_ref" != "$initial_state_ref" ]]; then
    regtest_log "Historical state ref changed after stable head advanced"
    exit 1
  fi

  regtest_log "Stable lag smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"
