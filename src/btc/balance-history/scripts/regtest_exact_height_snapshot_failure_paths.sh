#!/usr/bin/env bash

set -euo pipefail
ulimit -c 0

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
export REPO_ROOT
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-failures-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/config-source}"
SNAPSHOT_BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-$WORK_DIR/snapshot-builder}"
WRONG_HASH_BUILDER_ROOT="${WRONG_HASH_BUILDER_ROOT:-$WORK_DIR/wrong-hash-builder}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30332}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30333}"
BH_RPC_PORT="${BH_RPC_PORT:-30310}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotfailures}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
export REGTEST_LOG_PREFIX="[exact-snapshot-failures]"

# SCRIPT_DIR is resolved from this file at runtime.
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/regtest_lib.sh"

json_field() {
  local file="$1"
  local field="$2"
  python3 - "$file" "$field" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    data = json.load(source)
print(data[sys.argv[2]])
PY
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd grep
  regtest_require_cmd python3
  regtest_ensure_workspace_dirs
  mkdir -p "$SNAPSHOT_BUILDER_ROOT" "$WRONG_HASH_BUILDER_ROOT"

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address tip target_height next_height target_hash next_hash wrong_hash
  local wrong_hash_output abort_output conflict_output resume_report publish_failure_output
  local publish_status publish_report verify_failure_output final_dir temp_count
  local artifact_dir snapshot_file snapshot_path exit_code

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  tip="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  target_height=$((tip - 6))
  next_height=$((target_height + 1))
  target_hash="$(regtest_get_block_hash_by_height "$target_height")"
  next_hash="$(regtest_get_block_hash_by_height "$next_height")"
  wrong_hash="$(printf '0%.0s' {1..64})"
  if [[ "$wrong_hash" == "$target_hash" ]]; then
    wrong_hash="$(printf 'f%.0s' {1..64})"
  fi

  regtest_create_balance_history_config_at "$BALANCE_HISTORY_ROOT" "$BH_RPC_PORT"

  wrong_hash_output="$WORK_DIR/wrong-hash.out"
  regtest_expect_command_failure "$wrong_hash_output" "Canonical BTC block hash mismatch" \
    regtest_run_snapshot_tool "$WRONG_HASH_BUILDER_ROOT" create \
      --height "$target_height" \
      --expected-block-hash "$wrong_hash" \
      --config "$BALANCE_HISTORY_ROOT/config.toml" \
      --poll-interval-secs 1
  if find "$WRONG_HASH_BUILDER_ROOT/snapshots" -name complete.json -print -quit | grep -q .; then
    regtest_log "Wrong expected hash unexpectedly published an artifact"
    exit 1
  fi

  abort_output="$WORK_DIR/abort-syncing.out"
  export USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT=syncing
  set +e
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$target_height" \
    --expected-block-hash "$target_hash" \
    --config "$BALANCE_HISTORY_ROOT/config.toml" \
    --poll-interval-secs 1 >"$abort_output" 2>&1
  exit_code=$?
  set -e
  unset USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT
  if [[ "$exit_code" -eq 0 ]]; then
    regtest_log "Expected syncing checkpoint abort but create succeeded"
    exit 1
  fi

  conflict_output="$WORK_DIR/conflicting-target.out"
  regtest_expect_command_failure "$conflict_output" "is still active" \
    regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
      --height "$next_height" \
      --expected-block-hash "$next_hash" \
      --poll-interval-secs 1

  resume_report="$WORK_DIR/resume-target.json"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$target_height" \
    --expected-block-hash "$target_hash" \
    --poll-interval-secs 1 >"$resume_report"
  regtest_assert_json_file "$resume_report" "data['height']" "$target_height"

  publish_failure_output="$WORK_DIR/before-publish.out"
  export USDB_BH_SNAPSHOT_TEST_FAIL_AT_CHECKPOINT=before_publish
  regtest_expect_command_failure "$publish_failure_output" \
    "Injected snapshot test failure at checkpoint before_publish" \
    regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
      --height "$next_height" \
      --expected-block-hash "$next_hash" \
      --poll-interval-secs 1
  unset USDB_BH_SNAPSHOT_TEST_FAIL_AT_CHECKPOINT

  final_dir="$SNAPSHOT_BUILDER_ROOT/snapshots/$(printf '%012d' "$next_height")/$next_hash"
  if [[ -e "$final_dir" ]]; then
    regtest_log "before_publish failure exposed a final artifact: ${final_dir}"
    exit 1
  fi
  temp_count="$(find "$SNAPSHOT_BUILDER_ROOT/tmp" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
  if [[ "$temp_count" != "1" ]]; then
    regtest_log "Expected one resumable temporary artifact, got ${temp_count}"
    exit 1
  fi

  publish_status="$WORK_DIR/before-publish-status.json"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" status \
    --height "$next_height" >"$publish_status"
  regtest_assert_json_file "$publish_status" "data['job']['stage']" "verifying"
  regtest_assert_json_file "$publish_status" "data['state']['active_job_height']" "$next_height"

  publish_report="$WORK_DIR/publish-resume.json"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$next_height" \
    --expected-block-hash "$next_hash" \
    --poll-interval-secs 1 >"$publish_report"
  regtest_assert_json_file "$publish_report" "data['height']" "$next_height"
  regtest_assert_json_file "$publish_report" "data['already_complete']" "False"
  temp_count="$(find "$SNAPSHOT_BUILDER_ROOT/tmp" -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
  if [[ "$temp_count" != "0" ]]; then
    regtest_log "Resumed publication left temporary artifact directories: ${temp_count}"
    exit 1
  fi

  artifact_dir="$(json_field "$publish_report" artifact_dir)"
  snapshot_file="$(json_field "$publish_report" snapshot_file)"
  snapshot_path="$SNAPSHOT_BUILDER_ROOT/$artifact_dir/$snapshot_file"
  printf 'tampered-after-publication' >>"$snapshot_path"
  verify_failure_output="$WORK_DIR/tampered-verify.out"
  regtest_expect_command_failure "$verify_failure_output" "Snapshot file hash mismatch" \
    regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" verify \
      --height "$next_height" \
      --block-hash "$next_hash"

  regtest_log "Exact-height snapshot failure-path test succeeded."
}

main "$@"
