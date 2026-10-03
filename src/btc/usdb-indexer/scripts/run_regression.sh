#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

MANIFEST_PATH="${MANIFEST_PATH:-${REPO_ROOT}/src/btc/Cargo.toml}"
RUN_REGTEST_SMOKE="${RUN_REGTEST_SMOKE:-1}"
RUN_LIVE_ORD_E2E="${RUN_LIVE_ORD_E2E:-0}"
RUN_LIVE_ORD_REALWORLD_SUITE="${RUN_LIVE_ORD_REALWORLD_SUITE:-0}"
RUN_UIP0001_0004_LIVE_MATRIX="${RUN_UIP0001_0004_LIVE_MATRIX:-0}"
RUN_REORG_REGRESSION="${RUN_REORG_REGRESSION:-0}"
RUN_MINER_PASS_V2_MATRIX="${RUN_MINER_PASS_V2_MATRIX:-0}"

log() {
  echo "[usdb-regression] $*"
}

run_cmd() {
  log "Running: $*"
  "$@"
}

run_core_protocol_tests() {
  local tests=(
    "storage::pass::tests::test_committed_reader_remains_available_during_spilled_savepoint"
    "storage::pass::tests::test_committed_reader_preserves_existing_rollback_journal_database"
    "index::miner_pass_activation::pipeline_rejects_v1_at_every_height_and_accepts_current_schema_from_origin"
    "index::miner_pass_activation::same_reveal_observes_prior_mint_history_and_real_transfer_before_mint"
    "index::miner_pass_eligibility::unsolicited_mint_cannot_replace_existing_active_or_consume_its_prev"
    "index::miner_pass_eligibility::cross_owner_consumes_only_listed_prev_and_inherits_each_after_loss"
    "index::miner_pass_eligibility::repeated_cross_owner_prev_is_consumed_once_even_with_two_fresh_targets"
    "index::miner_pass_eligibility::collab_uses_same_eligibility_and_neither_leader_reference_follows_rotation"
    "index::miner_pass_activation::pipeline_reopen_and_rollback_replay_preserve_identity_and_audit"
  )

  local listed_tests
  listed_tests="$(cargo test --manifest-path "${MANIFEST_PATH}" -p usdb-indexer -- --list)"
  for test_name in "${tests[@]}"; do
    if ! grep -Fxq "${test_name}: test" <<<"$listed_tests"; then
      log "Required protocol regression test is missing: ${test_name}"
      return 1
    fi
    run_cmd cargo test \
      --manifest-path "${MANIFEST_PATH}" \
      -p usdb-indexer \
      "${test_name}" \
      -- --exact
  done

  # Exercise atomic publication and crash recovery alongside the committed-reader checks.
  run_cmd cargo test \
    --manifest-path "${MANIFEST_PATH}" \
    -p usdb-indexer \
    snapshot_anchor_acceptance

  run_cmd cargo test \
    --manifest-path "${MANIFEST_PATH}" \
    -p usdb-indexer-checkpoint-tool \
    test::staged_wal_checkpoint_keeps_inventory_stable_during_validation \
    -- --exact
}

run_regtest_smoke_scenarios() {
  run_cmd "${SCRIPT_DIR}/regtest_e2e_smoke.sh"

  run_cmd env \
    SCENARIO_FILE="${SCRIPT_DIR}/scenarios/transfer_balance_assert.json" \
    "${SCRIPT_DIR}/regtest_e2e_smoke.sh"

  run_cmd env \
    SCENARIO_FILE="${SCRIPT_DIR}/scenarios/multi_transfer_balance_assert.json" \
    "${SCRIPT_DIR}/regtest_e2e_smoke.sh"

  run_cmd env \
    SCENARIO_FILE="${SCRIPT_DIR}/scenarios/energy_rpc_empty_surface_assert.json" \
    "${SCRIPT_DIR}/regtest_e2e_smoke.sh"
}

run_live_ord_realworld_suite() {
  local btc_rpc_port_1="${BTC_RPC_PORT:-28132}"
  local btc_p2p_port_1="${BTC_P2P_PORT:-28133}"
  local bh_rpc_port_1="${BH_RPC_PORT:-28110}"
  local usdb_indexer_rpc_port_1="${USDB_INDEXER_RPC_PORT:-28120}"
  local ord_server_port_1="${ORD_SERVER_PORT:-28130}"

  local btc_rpc_port_2="${LIVE_SUITE_BTC_RPC_PORT_2:-$((btc_rpc_port_1 + 1000))}"
  local btc_p2p_port_2="${LIVE_SUITE_BTC_P2P_PORT_2:-$((btc_p2p_port_1 + 1000))}"
  local bh_rpc_port_2="${LIVE_SUITE_BH_RPC_PORT_2:-$((bh_rpc_port_1 + 1000))}"
  local usdb_indexer_rpc_port_2="${LIVE_SUITE_USDB_INDEXER_RPC_PORT_2:-$((usdb_indexer_rpc_port_1 + 1000))}"
  local ord_server_port_2="${LIVE_SUITE_ORD_SERVER_PORT_2:-$((ord_server_port_1 + 1000))}"
  local btc_rpc_port_3="${LIVE_SUITE_BTC_RPC_PORT_3:-$((btc_rpc_port_1 + 2000))}"
  local btc_p2p_port_3="${LIVE_SUITE_BTC_P2P_PORT_3:-$((btc_p2p_port_1 + 2000))}"
  local bh_rpc_port_3="${LIVE_SUITE_BH_RPC_PORT_3:-$((bh_rpc_port_1 + 2000))}"
  local usdb_indexer_rpc_port_3="${LIVE_SUITE_USDB_INDEXER_RPC_PORT_3:-$((usdb_indexer_rpc_port_1 + 2000))}"
  local ord_server_port_3="${LIVE_SUITE_ORD_SERVER_PORT_3:-$((ord_server_port_1 + 2000))}"
  local btc_rpc_port_4="${LIVE_SUITE_BTC_RPC_PORT_4:-$((btc_rpc_port_1 + 3000))}"
  local btc_p2p_port_4="${LIVE_SUITE_BTC_P2P_PORT_4:-$((btc_p2p_port_1 + 3000))}"
  local bh_rpc_port_4="${LIVE_SUITE_BH_RPC_PORT_4:-$((bh_rpc_port_1 + 3000))}"
  local usdb_indexer_rpc_port_4="${LIVE_SUITE_USDB_INDEXER_RPC_PORT_4:-$((usdb_indexer_rpc_port_1 + 3000))}"
  local ord_server_port_4="${LIVE_SUITE_ORD_SERVER_PORT_4:-$((ord_server_port_1 + 3000))}"
  local btc_rpc_port_5="${LIVE_SUITE_BTC_RPC_PORT_5:-$((btc_rpc_port_1 + 4000))}"
  local btc_p2p_port_5="${LIVE_SUITE_BTC_P2P_PORT_5:-$((btc_p2p_port_1 + 4000))}"
  local bh_rpc_port_5="${LIVE_SUITE_BH_RPC_PORT_5:-$((bh_rpc_port_1 + 4000))}"
  local usdb_indexer_rpc_port_5="${LIVE_SUITE_USDB_INDEXER_RPC_PORT_5:-$((usdb_indexer_rpc_port_1 + 4000))}"
  local ord_server_port_5="${LIVE_SUITE_ORD_SERVER_PORT_5:-$((ord_server_port_1 + 4000))}"

  run_cmd env \
    LIVE_SCENARIO=transfer_remint \
    BTC_RPC_PORT="${btc_rpc_port_1}" \
    BTC_P2P_PORT="${btc_p2p_port_1}" \
    BH_RPC_PORT="${bh_rpc_port_1}" \
    USDB_INDEXER_RPC_PORT="${usdb_indexer_rpc_port_1}" \
    ORD_SERVER_PORT="${ord_server_port_1}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=invalid_mint \
    BTC_RPC_PORT="${btc_rpc_port_2}" \
    BTC_P2P_PORT="${btc_p2p_port_2}" \
    BH_RPC_PORT="${bh_rpc_port_2}" \
    USDB_INDEXER_RPC_PORT="${usdb_indexer_rpc_port_2}" \
    ORD_SERVER_PORT="${ord_server_port_2}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=passive_transfer \
    BTC_RPC_PORT="${btc_rpc_port_3}" \
    BTC_P2P_PORT="${btc_p2p_port_3}" \
    BH_RPC_PORT="${bh_rpc_port_3}" \
    USDB_INDEXER_RPC_PORT="${usdb_indexer_rpc_port_3}" \
    ORD_SERVER_PORT="${ord_server_port_3}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=same_owner_multi_mint \
    BTC_RPC_PORT="${btc_rpc_port_4}" \
    BTC_P2P_PORT="${btc_p2p_port_4}" \
    BH_RPC_PORT="${bh_rpc_port_4}" \
    USDB_INDEXER_RPC_PORT="${usdb_indexer_rpc_port_4}" \
    ORD_SERVER_PORT="${ord_server_port_4}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=duplicate_prev_inherit \
    BTC_RPC_PORT="${btc_rpc_port_5}" \
    BTC_P2P_PORT="${btc_p2p_port_5}" \
    BH_RPC_PORT="${bh_rpc_port_5}" \
    USDB_INDEXER_RPC_PORT="${usdb_indexer_rpc_port_5}" \
    ORD_SERVER_PORT="${ord_server_port_5}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"
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

  if [[ "${RUN_LIVE_ORD_REALWORLD_SUITE}" == "1" ]]; then
    run_live_ord_realworld_suite
  elif [[ "${RUN_LIVE_ORD_E2E}" == "1" ]]; then
    run_cmd "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"
  else
    log "Skipping live ord e2e: RUN_LIVE_ORD_E2E=${RUN_LIVE_ORD_E2E}, RUN_LIVE_ORD_REALWORLD_SUITE=${RUN_LIVE_ORD_REALWORLD_SUITE}"
  fi

  if [[ "${RUN_UIP0001_0004_LIVE_MATRIX}" == "1" ]]; then
    run_cmd "${SCRIPT_DIR}/regtest_live_ord_uip0001_0004_gap_matrix.sh"
  else
    log "Skipping UIP0001-0004 live matrix: RUN_UIP0001_0004_LIVE_MATRIX=${RUN_UIP0001_0004_LIVE_MATRIX}"
  fi

  if [[ "${RUN_MINER_PASS_V2_MATRIX}" == "1" ]]; then
    run_cmd bash "$REPO_ROOT/tests/run_miner_pass_v2_security_matrix.sh"
  fi

  if [[ "${RUN_REORG_REGRESSION}" == "1" ]]; then
    run_cmd bash "${SCRIPT_DIR}/run_reorg_regression.sh"
  else
    log "Skipping reorg regression suite: RUN_REORG_REGRESSION=${RUN_REORG_REGRESSION}"
  fi

  log "Regression suite succeeded."
}

main "$@"
