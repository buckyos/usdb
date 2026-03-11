#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-restart-multi-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28632}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28633}"
BH_RPC_PORT="${BH_RPC_PORT:-28610}"
WALLET_NAME="${WALLET_NAME:-bhrestartmultireorg}"
SCENARIO_START_HEIGHT="${SCENARIO_START_HEIGHT:-40}"
REORG_ROUNDS="${REORG_ROUNDS:-2}"
TRACKED_TX_COUNT="${TRACKED_TX_COUNT:-2}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[restart-multi-reorg-smoke]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  if (( REORG_ROUNDS <= 0 )); then
    regtest_log "REORG_ROUNDS must be positive"
    exit 1
  fi
  if (( TRACKED_TX_COUNT <= 0 )); then
    regtest_log "TRACKED_TX_COUNT must be positive"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height target_height round tx_index receiver_address txid original_hash replacement_hash snapshot_hash
  local -a round_receiver_addresses=()
  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  target_height=$((current_height + SCENARIO_START_HEIGHT))
  regtest_log "Mining $((target_height - current_height)) scenario blocks to reach target height=${target_height}"
  regtest_mine_blocks "$((target_height - current_height))" "$mining_address"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$target_height"

  for round in $(seq 1 "$REORG_ROUNDS"); do
    round_receiver_addresses=()
    for tx_index in $(seq 1 "$TRACKED_TX_COUNT"); do
      receiver_address="$(regtest_get_new_address)"
      round_receiver_addresses+=("$receiver_address")
      regtest_log "Round ${round}/${REORG_ROUNDS}: sending ${SEND_AMOUNT_BTC} BTC for tracked transfer ${tx_index}/${TRACKED_TX_COUNT} to address=${receiver_address}"
      txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
      regtest_log "Round ${round}/${REORG_ROUNDS}: created txid=${txid}"
    done

    regtest_mine_blocks 1 "$mining_address"

    current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    regtest_wait_until_synced_height "$current_height"
    for receiver_address in "${round_receiver_addresses[@]}"; do
      regtest_assert_address_balance_btc "$receiver_address" "$current_height" "$SEND_AMOUNT_BTC"
    done

    original_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$current_height")"
    regtest_log "Round ${round}/${REORG_ROUNDS}: stopping service before offline reorg at height=${current_height}, hash=${original_hash}"
    regtest_stop_balance_history

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_hash"
    regtest_mine_empty_block "$(regtest_get_new_address)"

    replacement_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$current_height")"
    regtest_log "Round ${round}/${REORG_ROUNDS}: replacement_hash=${replacement_hash}"
    if [[ "$replacement_hash" == "$original_hash" ]]; then
      regtest_log "Offline reorg round ${round} failed: replacement hash matches original hash"
      exit 1
    fi

    regtest_restart_balance_history
    regtest_wait_until_synced_height "$current_height"
    regtest_wait_until_block_commit_hash "$current_height" "$replacement_hash"
    for receiver_address in "${round_receiver_addresses[@]}"; do
      regtest_assert_address_balance_btc "$receiver_address" "$current_height" "0"
    done

    snapshot_hash="$(regtest_get_snapshot_stable_hash)"
    regtest_log "Round ${round}/${REORG_ROUNDS}: stable_block_hash=${snapshot_hash}"
    if [[ "$snapshot_hash" != "$replacement_hash" ]]; then
      regtest_log "Snapshot info did not converge after offline reorg round ${round}"
      exit 1
    fi
  done

  regtest_log "Restart multi reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"