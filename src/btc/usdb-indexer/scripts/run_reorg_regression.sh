#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
RUN_SMOKE_REORG_SUITE="${RUN_SMOKE_REORG_SUITE:-1}"
RUN_LIVE_ORD_REORG_SUITE="${RUN_LIVE_ORD_REORG_SUITE:-1}"
RUN_PENDING_RECOVERY_SUITE="${RUN_PENDING_RECOVERY_SUITE:-1}"
RUN_HISTORICAL_VALIDATION_SUITE="${RUN_HISTORICAL_VALIDATION_SUITE:-1}"
RUN_VALIDATOR_BLOCK_BODY_SUITE="${RUN_VALIDATOR_BLOCK_BODY_SUITE:-1}"

BASE_BTC_RPC_PORT="${BASE_BTC_RPC_PORT:-30132}"
BASE_BTC_P2P_PORT="${BASE_BTC_P2P_PORT:-30133}"
BASE_BH_RPC_PORT="${BASE_BH_RPC_PORT:-30110}"
BASE_USDB_RPC_PORT="${BASE_USDB_RPC_PORT:-30120}"
BASE_ORD_RPC_PORT="${BASE_ORD_RPC_PORT:-30130}"
PORT_STRIDE="${PORT_STRIDE:-100}"

log() {
  echo "[usdb-reorg-regression] $*"
}

run_cmd() {
  log "Running: $*"
  "$@"
}

run_case() {
  local slot="$1"
  local script_name="$2"

  local btc_rpc_port="$((BASE_BTC_RPC_PORT + slot * PORT_STRIDE))"
  local btc_p2p_port="$((BASE_BTC_P2P_PORT + slot * PORT_STRIDE))"
  local bh_rpc_port="$((BASE_BH_RPC_PORT + slot * PORT_STRIDE))"
  local usdb_rpc_port="$((BASE_USDB_RPC_PORT + slot * PORT_STRIDE))"
  local ord_rpc_port="$((BASE_ORD_RPC_PORT + slot * PORT_STRIDE))"

  run_cmd env \
    BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR}" \
    ORD_BIN="${ORD_BIN}" \
    BTC_RPC_PORT="${btc_rpc_port}" \
    BTC_P2P_PORT="${btc_p2p_port}" \
    BH_RPC_PORT="${bh_rpc_port}" \
    USDB_RPC_PORT="${usdb_rpc_port}" \
    ORD_RPC_PORT="${ord_rpc_port}" \
    bash "${SCRIPT_DIR}/${script_name}"
}

run_smoke_reorg_suite() {
  local slot=0
  run_case "$slot" "regtest_reorg_smoke.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_same_height_reorg_smoke.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_restart_reorg_smoke.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_restart_same_height_reorg.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_restart_multi_reorg_smoke.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_restart_hybrid_reorg_smoke.sh"
}

run_live_ord_reorg_suite() {
  local slot=10
  run_case "$slot" "regtest_live_ord_reorg_transfer_remint.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_same_height_reorg_transfer_remint.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_multi_block_reorg.sh"
}

run_pending_recovery_suite() {
  local slot=20
  run_case "$slot" "regtest_pending_recovery_energy_failure.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_pending_recovery_transfer_reload_restart.sh"
}

run_historical_validation_suite() {
  local slot=30
  run_case "$slot" "regtest_live_ord_historical_validation_reorg.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_historical_validation_floor_restart.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_historical_validation_history_not_available.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_historical_context_e2e.sh"
}

run_validator_block_body_suite() {
  local slot=40
  run_case "$slot" "regtest_live_ord_validator_block_body_e2e.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_state_advance.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_competing_payloads.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_reorg.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_retention.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_two_pass_competition.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_two_pass_energy_advantage.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_two_pass_competing_payloads.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_two_pass_reorg.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_two_pass_tamper.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_three_pass_candidate_set.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_five_pass_candidate_set_tamper.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_five_pass_candidate_set_reorg.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_protocol_version_mismatch.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_semantics_version_mismatch.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_candidate_set_protocol_version_mismatch.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_candidate_set_semantics_version_mismatch.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_api_version_mismatch.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_version_matrix.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_payload_version_upgrade.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_payload_version_upgrade_restart.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_payload_version_upgrade_reorg.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_restart_consistency.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_not_ready_window.sh"
  slot=$((slot + 1))
  run_case "$slot" "regtest_live_ord_validator_block_body_candidate_set_crash_recovery.sh"
}

main() {
  log "Repo root: ${REPO_ROOT}"
  log "Bitcoin bin dir: ${BITCOIN_BIN_DIR}"
  log "Ord bin: ${ORD_BIN}"

  if [[ "${RUN_SMOKE_REORG_SUITE}" == "1" ]]; then
    run_smoke_reorg_suite
  else
    log "Skipping smoke reorg suite: RUN_SMOKE_REORG_SUITE=${RUN_SMOKE_REORG_SUITE}"
  fi

  if [[ "${RUN_LIVE_ORD_REORG_SUITE}" == "1" ]]; then
    run_live_ord_reorg_suite
  else
    log "Skipping live ord reorg suite: RUN_LIVE_ORD_REORG_SUITE=${RUN_LIVE_ORD_REORG_SUITE}"
  fi

  if [[ "${RUN_PENDING_RECOVERY_SUITE}" == "1" ]]; then
    run_pending_recovery_suite
  else
    log "Skipping pending recovery suite: RUN_PENDING_RECOVERY_SUITE=${RUN_PENDING_RECOVERY_SUITE}"
  fi

  if [[ "${RUN_HISTORICAL_VALIDATION_SUITE}" == "1" ]]; then
    run_historical_validation_suite
  else
    log "Skipping historical validation suite: RUN_HISTORICAL_VALIDATION_SUITE=${RUN_HISTORICAL_VALIDATION_SUITE}"
  fi

  if [[ "${RUN_VALIDATOR_BLOCK_BODY_SUITE}" == "1" ]]; then
    run_validator_block_body_suite
  else
    log "Skipping validator block-body suite: RUN_VALIDATOR_BLOCK_BODY_SUITE=${RUN_VALIDATOR_BLOCK_BODY_SUITE}"
  fi

  log "USDB reorg regression suite succeeded."
}

main "$@"
