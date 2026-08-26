#!/usr/bin/env bash

set -euo pipefail
ulimit -c 0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
export REPO_ROOT
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-restart-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/config-source}"
SNAPSHOT_BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-$WORK_DIR/snapshot-builder}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29932}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29933}"
BH_RPC_PORT="${BH_RPC_PORT:-29910}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotrestart}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
export REGTEST_LOG_PREFIX="[exact-snapshot-restart]"

# SCRIPT_DIR is resolved from this file at runtime.
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/regtest_lib.sh"

expected_stage_for_checkpoint() {
  case "$1" in
    syncing) echo "syncing" ;;
    sealed) echo "sealed" ;;
    building) echo "building" ;;
    verifying | published) echo "verifying" ;;
    job_complete) echo "complete" ;;
    *) return 1 ;;
  esac
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

  local mining_address tip base_height base_hash base_report
  local checkpoint expected_stage target_height target_hash crash_log crash_stdout
  local status_file resume_report exit_code index
  local checkpoints=(syncing sealed building verifying published job_complete)

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  regtest_mine_blocks 6 "$mining_address"
  tip="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  base_height=$((tip - 12))
  base_hash="$(regtest_get_block_hash_by_height "$base_height")"
  base_report="$WORK_DIR/base-${base_height}.json"

  regtest_create_balance_history_config_at "$BALANCE_HISTORY_ROOT" "$BH_RPC_PORT"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$base_height" \
    --expected-block-hash "$base_hash" \
    --config "$BALANCE_HISTORY_ROOT/config.toml" \
    --poll-interval-secs 1 >"$base_report"
  regtest_assert_json_file "$base_report" "data['already_complete']" "False"

  for index in "${!checkpoints[@]}"; do
    checkpoint="${checkpoints[$index]}"
    expected_stage="$(expected_stage_for_checkpoint "$checkpoint")"
    target_height=$((base_height + index + 1))
    target_hash="$(regtest_get_block_hash_by_height "$target_height")"
    crash_log="$WORK_DIR/crash-${checkpoint}.log"
    crash_stdout="$WORK_DIR/crash-${checkpoint}.json"
    status_file="$WORK_DIR/status-${checkpoint}.json"
    resume_report="$WORK_DIR/resume-${checkpoint}.json"

    regtest_log "Aborting target=${target_height} after durable checkpoint=${checkpoint}"
    export USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT="$checkpoint"
    set +e
    regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
      --height "$target_height" \
      --expected-block-hash "$target_hash" \
      --poll-interval-secs 1 >"$crash_stdout" 2>"$crash_log"
    exit_code=$?
    set -e
    unset USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT
    if [[ "$exit_code" -eq 0 ]]; then
      regtest_log "Expected checkpoint ${checkpoint} to abort, but create succeeded"
      exit 1
    fi

    regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" status \
      --height "$target_height" >"$status_file"
    regtest_assert_json_file "$status_file" "data['job']['stage']" "$expected_stage"
    regtest_assert_json_file "$status_file" "data['state']['active_job_height']" "$target_height"

    regtest_log "Resuming target=${target_height} after checkpoint=${checkpoint}"
    if [[ "$expected_stage" == "verifying" ]]; then
      regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" resume-verify \
        --height "$target_height" \
        --expected-block-hash "$target_hash" >"$resume_report"
    else
      regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
        --height "$target_height" \
        --expected-block-hash "$target_hash" \
        --poll-interval-secs 1 >"$resume_report"
    fi
    regtest_assert_json_file "$resume_report" "data['height']" "$target_height"
    regtest_assert_json_file "$resume_report" "data['btc_block_hash']" "$target_hash"

    regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" status >"$status_file"
    regtest_assert_json_file "$status_file" "data['state']['latest_completed']['height']" "$target_height"
    regtest_assert_json_file "$status_file" "data['state']['active_job_height'] is None" "True"
  done

  regtest_log "All durable checkpoint restart cases succeeded."
}

main "$@"
