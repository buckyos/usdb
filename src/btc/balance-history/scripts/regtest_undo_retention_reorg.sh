#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-undo-retention-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30332}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30333}"
BH_RPC_PORT="${BH_RPC_PORT:-30310}"
WALLET_NAME="${WALLET_NAME:-bhundoreorg}"
SCENARIO_START_HEIGHT="${SCENARIO_START_HEIGHT:-45}"
REORG_DEPTH="${REORG_DEPTH:-2}"
TRACKED_TX_COUNT="${TRACKED_TX_COUNT:-2}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
UNDO_RETENTION_BLOCKS="${UNDO_RETENTION_BLOCKS:-2}"
UNDO_CLEANUP_INTERVAL_BLOCKS="${UNDO_CLEANUP_INTERVAL_BLOCKS:-1}"
PRUNE_ADVANCE_BLOCKS="${PRUNE_ADVANCE_BLOCKS:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
BALANCE_HISTORY_SERVICE_LOG_FILE="${BALANCE_HISTORY_SERVICE_LOG_FILE:-$BALANCE_HISTORY_ROOT/logs/balance-history_rCURRENT.log}"
REGTEST_LOG_PREFIX="[undo-retention-reorg]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_require_cmd grep

  if (( REORG_DEPTH <= 0 )); then
    regtest_log "REORG_DEPTH must be positive"
    exit 1
  fi
  if (( TRACKED_TX_COUNT <= 0 )); then
    regtest_log "TRACKED_TX_COUNT must be positive"
    exit 1
  fi
  if (( REORG_DEPTH != TRACKED_TX_COUNT )); then
    regtest_log "This scenario expects REORG_DEPTH to equal TRACKED_TX_COUNT"
    exit 1
  fi
  if (( PRUNE_ADVANCE_BLOCKS <= 0 )); then
    regtest_log "PRUNE_ADVANCE_BLOCKS must be positive"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height stable_prefix_height warmup_target_height target_height affected_height tx_index receiver_address txid original_affected_hash original_tip_hash replacement_tip_hash replacement_address round
  local -a receiver_addresses=()

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  stable_prefix_height=$((current_height + SCENARIO_START_HEIGHT))

  regtest_log "Mining stable prefix to height=${stable_prefix_height}"
  regtest_mine_blocks "$((stable_prefix_height - current_height))" "$mining_address"

  regtest_create_balance_history_config
  regtest_config_set_sync_value "$BALANCE_HISTORY_ROOT/config.toml" "undo_retention_blocks" "$UNDO_RETENTION_BLOCKS"
  regtest_config_set_sync_value "$BALANCE_HISTORY_ROOT/config.toml" "undo_cleanup_interval_blocks" "$UNDO_CLEANUP_INTERVAL_BLOCKS"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$stable_prefix_height"

  warmup_target_height=$((stable_prefix_height + PRUNE_ADVANCE_BLOCKS))
  regtest_log "Advancing canonical tip online to height=${warmup_target_height} so old undo entries are pruned"
  for round in $(seq 1 "$PRUNE_ADVANCE_BLOCKS"); do
    replacement_address="$(regtest_get_new_address)"
    regtest_mine_empty_block "$replacement_address"
  done
  regtest_wait_until_synced_height "$warmup_target_height"

  if ! grep -q "Undo retention prune finished" "$BALANCE_HISTORY_SERVICE_LOG_FILE"; then
    regtest_log "Expected undo retention prune log entry was not found"
    exit 1
  fi

  target_height=$warmup_target_height
  for tx_index in $(seq 1 "$TRACKED_TX_COUNT"); do
    receiver_address="$(regtest_get_new_address)"
    receiver_addresses+=("$receiver_address")
    regtest_log "Creating retained-window transfer ${tx_index}/${TRACKED_TX_COUNT} of ${SEND_AMOUNT_BTC} BTC to address=${receiver_address}"
    txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
    regtest_log "Created tracked txid=${txid}"
    regtest_mine_blocks 1 "$mining_address"
    target_height=$((target_height + 1))
  done
  affected_height=$((target_height - REORG_DEPTH + 1))
  regtest_wait_until_synced_height "$target_height"

  for receiver_address in "${receiver_addresses[@]}"; do
    regtest_assert_address_balance_btc "$receiver_address" "$target_height" "$SEND_AMOUNT_BTC"
  done

  original_affected_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$affected_height")"
  original_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$target_height")"
  regtest_log "Stopping balance-history before retained-window offline reorg: affected_height=${affected_height}, original_affected_hash=${original_affected_hash}, original_tip_hash=${original_tip_hash}"
  regtest_stop_balance_history

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_affected_hash"
  for round in $(seq 1 "$REORG_DEPTH"); do
    replacement_address="$(regtest_get_new_address)"
    regtest_log "Mining retained-window replacement block ${round}/${REORG_DEPTH} to address=${replacement_address}"
    regtest_mine_empty_block "$replacement_address"
  done

  replacement_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$target_height")"
  if [[ "$replacement_tip_hash" == "$original_tip_hash" ]]; then
    regtest_log "Undo retention reorg failed: replacement tip hash matches original tip hash"
    exit 1
  fi

  regtest_restart_balance_history
  regtest_wait_until_synced_height "$target_height"
  regtest_wait_until_block_commit_hash "$target_height" "$replacement_tip_hash"

  if ! grep -q "BTC reorg detected, rolling back local balance-history state" "$BALANCE_HISTORY_SERVICE_LOG_FILE"; then
    regtest_log "Expected reorg rollback log entry was not found"
    exit 1
  fi

  for receiver_address in "${receiver_addresses[@]}"; do
    regtest_assert_address_balance_btc "$receiver_address" "$target_height" "0"
  done

  if [[ "$(regtest_get_snapshot_stable_hash)" != "$replacement_tip_hash" ]]; then
    regtest_log "Snapshot info did not converge to retained-window replacement tip hash"
    exit 1
  fi

  regtest_log "Undo retention reorg test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"