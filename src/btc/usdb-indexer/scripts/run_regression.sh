#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

MANIFEST_PATH="${MANIFEST_PATH:-${REPO_ROOT}/src/btc/Cargo.toml}"
RUN_REGTEST_SMOKE="${RUN_REGTEST_SMOKE:-1}"
RUN_LIVE_ORD_E2E="${RUN_LIVE_ORD_E2E:-0}"
RUN_LIVE_ORD_REALWORLD_SUITE="${RUN_LIVE_ORD_REALWORLD_SUITE:-0}"

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

run_live_ord_realworld_suite() {
  local btc_rpc_port_1="${BTC_RPC_PORT:-28132}"
  local btc_p2p_port_1="${BTC_P2P_PORT:-28133}"
  local bh_rpc_port_1="${BH_RPC_PORT:-28110}"
  local usdb_rpc_port_1="${USDB_RPC_PORT:-28120}"
  local ord_server_port_1="${ORD_SERVER_PORT:-28130}"

  local btc_rpc_port_2="${LIVE_SUITE_BTC_RPC_PORT_2:-$((btc_rpc_port_1 + 1000))}"
  local btc_p2p_port_2="${LIVE_SUITE_BTC_P2P_PORT_2:-$((btc_p2p_port_1 + 1000))}"
  local bh_rpc_port_2="${LIVE_SUITE_BH_RPC_PORT_2:-$((bh_rpc_port_1 + 1000))}"
  local usdb_rpc_port_2="${LIVE_SUITE_USDB_RPC_PORT_2:-$((usdb_rpc_port_1 + 1000))}"
  local ord_server_port_2="${LIVE_SUITE_ORD_SERVER_PORT_2:-$((ord_server_port_1 + 1000))}"
  local btc_rpc_port_3="${LIVE_SUITE_BTC_RPC_PORT_3:-$((btc_rpc_port_1 + 2000))}"
  local btc_p2p_port_3="${LIVE_SUITE_BTC_P2P_PORT_3:-$((btc_p2p_port_1 + 2000))}"
  local bh_rpc_port_3="${LIVE_SUITE_BH_RPC_PORT_3:-$((bh_rpc_port_1 + 2000))}"
  local usdb_rpc_port_3="${LIVE_SUITE_USDB_RPC_PORT_3:-$((usdb_rpc_port_1 + 2000))}"
  local ord_server_port_3="${LIVE_SUITE_ORD_SERVER_PORT_3:-$((ord_server_port_1 + 2000))}"
  local btc_rpc_port_4="${LIVE_SUITE_BTC_RPC_PORT_4:-$((btc_rpc_port_1 + 3000))}"
  local btc_p2p_port_4="${LIVE_SUITE_BTC_P2P_PORT_4:-$((btc_p2p_port_1 + 3000))}"
  local bh_rpc_port_4="${LIVE_SUITE_BH_RPC_PORT_4:-$((bh_rpc_port_1 + 3000))}"
  local usdb_rpc_port_4="${LIVE_SUITE_USDB_RPC_PORT_4:-$((usdb_rpc_port_1 + 3000))}"
  local ord_server_port_4="${LIVE_SUITE_ORD_SERVER_PORT_4:-$((ord_server_port_1 + 3000))}"
  local btc_rpc_port_5="${LIVE_SUITE_BTC_RPC_PORT_5:-$((btc_rpc_port_1 + 4000))}"
  local btc_p2p_port_5="${LIVE_SUITE_BTC_P2P_PORT_5:-$((btc_p2p_port_1 + 4000))}"
  local bh_rpc_port_5="${LIVE_SUITE_BH_RPC_PORT_5:-$((bh_rpc_port_1 + 4000))}"
  local usdb_rpc_port_5="${LIVE_SUITE_USDB_RPC_PORT_5:-$((usdb_rpc_port_1 + 4000))}"
  local ord_server_port_5="${LIVE_SUITE_ORD_SERVER_PORT_5:-$((ord_server_port_1 + 4000))}"

  run_cmd env \
    LIVE_SCENARIO=transfer_remint \
    BTC_RPC_PORT="${btc_rpc_port_1}" \
    BTC_P2P_PORT="${btc_p2p_port_1}" \
    BH_RPC_PORT="${bh_rpc_port_1}" \
    USDB_RPC_PORT="${usdb_rpc_port_1}" \
    ORD_SERVER_PORT="${ord_server_port_1}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=invalid_mint \
    BTC_RPC_PORT="${btc_rpc_port_2}" \
    BTC_P2P_PORT="${btc_p2p_port_2}" \
    BH_RPC_PORT="${bh_rpc_port_2}" \
    USDB_RPC_PORT="${usdb_rpc_port_2}" \
    ORD_SERVER_PORT="${ord_server_port_2}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=passive_transfer \
    BTC_RPC_PORT="${btc_rpc_port_3}" \
    BTC_P2P_PORT="${btc_p2p_port_3}" \
    BH_RPC_PORT="${bh_rpc_port_3}" \
    USDB_RPC_PORT="${usdb_rpc_port_3}" \
    ORD_SERVER_PORT="${ord_server_port_3}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=same_owner_multi_mint \
    BTC_RPC_PORT="${btc_rpc_port_4}" \
    BTC_P2P_PORT="${btc_p2p_port_4}" \
    BH_RPC_PORT="${bh_rpc_port_4}" \
    USDB_RPC_PORT="${usdb_rpc_port_4}" \
    ORD_SERVER_PORT="${ord_server_port_4}" \
    "${SCRIPT_DIR}/regtest_live_ord_e2e.sh"

  run_cmd env \
    LIVE_SCENARIO=duplicate_prev_inherit \
    BTC_RPC_PORT="${btc_rpc_port_5}" \
    BTC_P2P_PORT="${btc_p2p_port_5}" \
    BH_RPC_PORT="${bh_rpc_port_5}" \
    USDB_RPC_PORT="${usdb_rpc_port_5}" \
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

  log "Regression suite succeeded."
}

main "$@"
