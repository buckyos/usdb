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
TARGET_TIP_HEIGHT="${TARGET_TIP_HEIGHT:-20}"
EXTRA_BLOCKS="${EXTRA_BLOCKS:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[stable-lag-smoke]"

source "${SCRIPT_DIR}/regtest_lib.sh"

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

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address
  mining_address="$(regtest_get_new_address)"
  regtest_log "Mining ${TARGET_TIP_HEIGHT} blocks to address=${mining_address}"
  regtest_mine_blocks "$TARGET_TIP_HEIGHT" "$mining_address"

  regtest_create_balance_history_config

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready

  local stable_lag expected_stable_height
  stable_lag="$(regtest_get_snapshot_stable_lag)"
  regtest_log "Observed protocol stable_lag=${stable_lag}"
  expected_stable_height=$((TARGET_TIP_HEIGHT - stable_lag))
  regtest_wait_until_synced_height "$expected_stable_height"
  regtest_wait_balance_history_consensus_ready
  assert_snapshot_matches_expected "$TARGET_TIP_HEIGHT" "$expected_stable_height" "$stable_lag"

  regtest_log "Mining ${EXTRA_BLOCKS} extra blocks to verify the service keeps exposing tip-lag"
  regtest_mine_blocks "$EXTRA_BLOCKS" "$mining_address"

  local new_tip_height new_stable_height
  new_tip_height=$((TARGET_TIP_HEIGHT + EXTRA_BLOCKS))
  new_stable_height=$((new_tip_height - stable_lag))
  regtest_wait_until_synced_height "$new_stable_height"
  regtest_wait_balance_history_consensus_ready
  assert_snapshot_matches_expected "$new_tip_height" "$new_stable_height" "$stable_lag"

  regtest_log "Stable lag smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"
