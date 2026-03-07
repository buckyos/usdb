#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

MANIFEST_PATH="${MANIFEST_PATH:-${REPO_ROOT}/src/btc/Cargo.toml}"
RUN_REGTEST_SMOKE="${RUN_REGTEST_SMOKE:-1}"

log() {
  echo "[usdb-regression] $*"
}

run_cmd() {
  log "Running: $*"
  "$@"
}

run_core_protocol_tests() {
  local tests=(
    "index::test::indexer_behavior::test_sync_blocks_timeline_mint_transfer_burn_remint_replay"
    "index::test::indexer_behavior::test_sync_blocks_passive_transfer_keeps_receiver_active_and_transferred_pass_dormant"
    "index::test::indexer_behavior::test_sync_blocks_same_owner_multiple_mints_keep_only_latest_active"
    "index::test::indexer_behavior::test_sync_blocks_multi_prev_inherit_sums_energy_and_consumes_all_prev"
    "index::test::indexer_behavior::test_sync_blocks_double_inherit_same_prev_only_first_gets_energy"
    "index::test::indexer_behavior::test_sync_blocks_balance_threshold_and_penalty_applied_before_dormant_transfer"
    "index::test::indexer_behavior::test_sync_blocks_restart_after_failed_block_replay_matches_fresh_run"
  )

  for test_name in "${tests[@]}"; do
    run_cmd cargo test \
      --manifest-path "${MANIFEST_PATH}" \
      -p usdb-indexer \
      "${test_name}" \
      -- --exact
  done
}

run_regtest_smoke_scenarios() {
  run_cmd "${SCRIPT_DIR}/regtest_e2e_smoke.sh"

  run_cmd env \
    SCENARIO_FILE="${SCRIPT_DIR}/scenarios/transfer_balance_assert.json" \
    "${SCRIPT_DIR}/regtest_e2e_smoke.sh"

  run_cmd env \
    SCENARIO_FILE="${SCRIPT_DIR}/scenarios/multi_transfer_balance_assert.json" \
    "${SCRIPT_DIR}/regtest_e2e_smoke.sh"
}

main() {
  log "Repo root: ${REPO_ROOT}"
  log "Manifest path: ${MANIFEST_PATH}"

  run_core_protocol_tests

  if [[ "${RUN_REGTEST_SMOKE}" == "1" ]]; then
    run_regtest_smoke_scenarios
  else
    log "Skipping regtest smoke scenarios: RUN_REGTEST_SMOKE=${RUN_REGTEST_SMOKE}"
  fi

  log "Regression suite succeeded."
}

main "$@"
