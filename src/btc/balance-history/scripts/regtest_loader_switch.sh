#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-loader-switch-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30432}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30433}"
BH_RPC_PORT="${BH_RPC_PORT:-30410}"
WALLET_NAME="${WALLET_NAME:-bhloaderswitch}"
SCENARIO_START_HEIGHT="${SCENARIO_START_HEIGHT:-45}"
LOCAL_LOADER_THRESHOLD="${LOCAL_LOADER_THRESHOLD:-10}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
BALANCE_HISTORY_SERVICE_LOG_FILE="${BALANCE_HISTORY_SERVICE_LOG_FILE:-$BALANCE_HISTORY_ROOT/logs/balance-history_rCURRENT.log}"
REGTEST_LOG_PREFIX="[loader-switch]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_require_cmd grep

  if (( LOCAL_LOADER_THRESHOLD <= 0 )); then
    regtest_log "LOCAL_LOADER_THRESHOLD must be positive"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height stable_prefix_height first_target_height second_target_height
  local receiver_one receiver_two txid_one txid_two second_tip_hash

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  stable_prefix_height=$((current_height + SCENARIO_START_HEIGHT))
  regtest_log "Mining stable prefix to height=${stable_prefix_height}"
  regtest_mine_blocks "$((stable_prefix_height - current_height))" "$mining_address"

  receiver_one="$(regtest_get_new_address)"
  regtest_log "Creating pre-bootstrap transfer via future LocalLoader catch-up: amount=${SEND_AMOUNT_BTC}, address=${receiver_one}"
  txid_one="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_one" "$SEND_AMOUNT_BTC")"
  regtest_log "Created initial tracked txid=${txid_one}"
  regtest_mine_blocks 1 "$mining_address"
  first_target_height=$((stable_prefix_height + 1))

  regtest_create_balance_history_config
  regtest_config_set_sync_value "$BALANCE_HISTORY_ROOT/config.toml" "local_loader_threshold" "$LOCAL_LOADER_THRESHOLD"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$first_target_height"
  regtest_assert_address_balance_btc "$receiver_one" "$first_target_height" "$SEND_AMOUNT_BTC"

  if ! grep -q "Using LocalLoader BTC client as we are behind by more than ${LOCAL_LOADER_THRESHOLD} blocks" "$BALANCE_HISTORY_SERVICE_LOG_FILE"; then
    regtest_log "Expected LocalLoader selection log entry was not found"
    exit 1
  fi
  if ! grep -q "Using BestEffort cache strategy for Local Loader BTC client" "$BALANCE_HISTORY_SERVICE_LOG_FILE"; then
    regtest_log "Expected LocalLoader cache strategy log entry was not found"
    exit 1
  fi

  regtest_log "Stopping service after LocalLoader catch-up so the next incremental sync can exercise the RPC path"
  regtest_stop_balance_history

  receiver_two="$(regtest_get_new_address)"
  regtest_log "Creating incremental transfer for RPC catch-up after restart: amount=${SEND_AMOUNT_BTC}, address=${receiver_two}"
  txid_two="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_two" "$SEND_AMOUNT_BTC")"
  regtest_log "Created restart tracked txid=${txid_two}"
  regtest_mine_blocks 1 "$mining_address"
  second_target_height=$((first_target_height + 1))
  second_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$second_target_height")"

  regtest_restart_balance_history
  regtest_wait_until_synced_height "$second_target_height"
  regtest_wait_until_block_commit_hash "$second_target_height" "$second_tip_hash"
  regtest_assert_address_balance_btc "$receiver_one" "$second_target_height" "$SEND_AMOUNT_BTC"
  regtest_assert_address_balance_btc "$receiver_two" "$second_target_height" "$SEND_AMOUNT_BTC"

  if ! grep -q "Using RPC BTC client" "$BALANCE_HISTORY_SERVICE_LOG_FILE"; then
    regtest_log "Expected RPC selection log entry was not found"
    exit 1
  fi
  if ! grep -q "Using Normal cache strategy for RPC BTC client" "$BALANCE_HISTORY_SERVICE_LOG_FILE"; then
    regtest_log "Expected RPC cache strategy log entry was not found"
    exit 1
  fi

  regtest_log "Loader switch test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"