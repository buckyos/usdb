#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
export REPO_ROOT
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/config-source}"
SNAPSHOT_BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-$WORK_DIR/snapshot-builder}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30032}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30033}"
BH_RPC_PORT="${BH_RPC_PORT:-30010}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotreorg}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
export REGTEST_LOG_PREFIX="[exact-snapshot-reorg]"

# SCRIPT_DIR is resolved from this file at runtime.
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd python3
  regtest_ensure_workspace_dirs
  mkdir -p "$SNAPSHOT_BUILDER_ROOT"

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address replacement_address tip target_height next_height
  local old_hash new_hash next_hash old_report new_report next_report status_file
  local old_verify new_verify artifact_count ambiguous_log

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  tip="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  target_height=$((tip - 6))
  next_height=$((target_height + 1))
  old_hash="$(regtest_get_block_hash_by_height "$target_height")"
  old_report="$WORK_DIR/create-old.json"
  new_report="$WORK_DIR/create-new.json"
  next_report="$WORK_DIR/create-next.json"
  old_verify="$WORK_DIR/verify-old.json"
  new_verify="$WORK_DIR/verify-new.json"
  status_file="$WORK_DIR/status.json"
  ambiguous_log="$WORK_DIR/verify-ambiguous.log"

  regtest_create_balance_history_config_at "$BALANCE_HISTORY_ROOT" "$BH_RPC_PORT"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$target_height" \
    --expected-block-hash "$old_hash" \
    --config "$BALANCE_HISTORY_ROOT/config.toml" \
    --poll-interval-secs 1 >"$old_report"

  regtest_log "Replacing canonical branch beginning at completed height=${target_height}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    invalidateblock "$old_hash"
  replacement_address="$(regtest_get_new_address)"
  regtest_mine_blocks 7 "$replacement_address"
  new_hash="$(regtest_get_block_hash_by_height "$target_height")"
  next_hash="$(regtest_get_block_hash_by_height "$next_height")"
  if [[ "$new_hash" == "$old_hash" ]]; then
    regtest_log "Expected a new block hash at replacement height=${target_height}"
    exit 1
  fi

  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$target_height" \
    --expected-block-hash "$new_hash" \
    --poll-interval-secs 1 >"$new_report"
  regtest_assert_json_file "$new_report" "data['btc_block_hash']" "$new_hash"

  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" verify \
    --height "$target_height" --block-hash "$old_hash" >"$old_verify"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" verify \
    --height "$target_height" --block-hash "$new_hash" >"$new_verify"
  regtest_assert_json_file "$old_verify" "data['btc_block_hash']" "$old_hash"
  regtest_assert_json_file "$new_verify" "data['btc_block_hash']" "$new_hash"

  artifact_count="$(find "$SNAPSHOT_BUILDER_ROOT/snapshots/$(printf '%012d' "$target_height")" \
    -mindepth 1 -maxdepth 1 -type d | wc -l | tr -d ' ')"
  if [[ "$artifact_count" != "2" ]]; then
    regtest_log "Expected two same-height branch artifacts, got ${artifact_count}"
    exit 1
  fi

  if regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" verify \
    --height "$target_height" >/dev/null 2>"$ambiguous_log"; then
    regtest_log "Expected hash-less verify to reject two same-height branch artifacts"
    exit 1
  fi
  if ! grep -q "specify --block-hash" "$ambiguous_log"; then
    regtest_log "Hash-less verify failed for an unexpected reason: $(cat "$ambiguous_log")"
    exit 1
  fi

  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$next_height" \
    --expected-block-hash "$next_hash" \
    --poll-interval-secs 1 >"$next_report"
  regtest_assert_json_file "$next_report" "data['btc_block_hash']" "$next_hash"

  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" status >"$status_file"
  regtest_assert_json_file "$status_file" "data['state']['latest_completed']['height']" "$next_height"
  regtest_assert_json_file "$status_file" "data['state']['latest_completed']['btc_block_hash']" "$next_hash"
  regtest_log "Same-height replacement and incremental continuation succeeded."
}

main "$@"
