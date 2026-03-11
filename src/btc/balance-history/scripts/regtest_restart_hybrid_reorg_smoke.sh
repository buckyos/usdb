#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-restart-hybrid-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28732}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28733}"
BH_RPC_PORT="${BH_RPC_PORT:-28710}"
WALLET_NAME="${WALLET_NAME:-bhrestarthybridreorg}"
SCENARIO_START_HEIGHT="${SCENARIO_START_HEIGHT:-45}"
TIP_TX_COUNT="${TIP_TX_COUNT:-2}"
DEEP_REORG_DEPTH="${DEEP_REORG_DEPTH:-3}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[restart-hybrid-reorg-smoke]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  if (( TIP_TX_COUNT <= 0 )); then
    regtest_log "TIP_TX_COUNT must be positive"
    exit 1
  fi
  if (( DEEP_REORG_DEPTH <= 0 )); then
    regtest_log "DEEP_REORG_DEPTH must be positive"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height stable_prefix_height target_height affected_height receiver_address txid original_tip_hash replacement_tip_hash replacement_address round
  local tip_block_hash tip_txid tip_vout deep_txid deep_vout deep_receiver_address
  local -a tip_receiver_addresses=()
  local -a tip_outpoints=()
  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  stable_prefix_height=$((current_height + SCENARIO_START_HEIGHT))
  target_height=$((stable_prefix_height + DEEP_REORG_DEPTH))
  affected_height=$((stable_prefix_height + 1))

  regtest_log "Mining stable prefix to height=${stable_prefix_height}"
  regtest_mine_blocks "$((stable_prefix_height - current_height))" "$mining_address"

  for round in $(seq 1 "$TIP_TX_COUNT"); do
    receiver_address="$(regtest_get_new_address)"
    tip_receiver_addresses+=("$receiver_address")
    regtest_log "Creating tip tracked transfer ${round}/${TIP_TX_COUNT} of ${SEND_AMOUNT_BTC} BTC to address=${receiver_address}"
    txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
    tip_vout="$(regtest_get_tx_vout_for_address "$txid" "$receiver_address")"
    if [[ -z "$tip_vout" ]]; then
      regtest_log "Failed to locate tracked tip output for txid=${txid}, address=${receiver_address}"
      exit 1
    fi
    tip_outpoints+=("${txid}:${tip_vout}")
  done
  regtest_mine_blocks 1 "$mining_address"

  for outpoint in "${tip_outpoints[@]}"; do
    tip_txid="${outpoint%%:*}"
    tip_vout="${outpoint##*:}"
    regtest_log "Locking tracked tip outpoint ${tip_txid}:${tip_vout} to prevent wallet respends before reorg"
    regtest_lock_wallet_outpoint "$tip_txid" "$tip_vout"
  done

  deep_receiver_address="$(regtest_get_new_address)"
  regtest_log "Creating deep tracked transfer of ${SEND_AMOUNT_BTC} BTC to address=${deep_receiver_address}"
  deep_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$deep_receiver_address" "$SEND_AMOUNT_BTC")"
  deep_vout="$(regtest_get_tx_vout_for_address "$deep_txid" "$deep_receiver_address")"
  if [[ -z "$deep_vout" ]]; then
    regtest_log "Failed to locate deep tracked output for txid=${deep_txid}, address=${deep_receiver_address}"
    exit 1
  fi
  regtest_mine_blocks 1 "$mining_address"

  if (( DEEP_REORG_DEPTH > 2 )); then
    regtest_log "Mining remaining $((DEEP_REORG_DEPTH - 2)) tail blocks to reach target height=${target_height}"
    regtest_mine_blocks $((DEEP_REORG_DEPTH - 2)) "$mining_address"
  fi

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$target_height"

  tip_block_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$((stable_prefix_height + 1))")"
  original_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$target_height")"

  for receiver_address in "${tip_receiver_addresses[@]}"; do
    regtest_assert_address_balance_btc "$receiver_address" "$target_height" "$SEND_AMOUNT_BTC"
  done
  regtest_assert_address_balance_btc "$deep_receiver_address" "$target_height" "$SEND_AMOUNT_BTC"

  for outpoint in "${tip_outpoints[@]}"; do
    tip_txid="${outpoint%%:*}"
    tip_vout="${outpoint##*:}"
    regtest_assert_utxo_value_sat "$tip_txid" "$tip_vout" "125000000"
  done
  regtest_assert_utxo_value_sat "$deep_txid" "$deep_vout" "125000000"

  regtest_log "Stopping balance-history before hybrid offline reorg: affected_height=${affected_height}, original_tip_hash=${original_tip_hash}"
  regtest_stop_balance_history

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$tip_block_hash"
  regtest_mine_empty_block "$(regtest_get_new_address)"

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$affected_height")"
  for round in $(seq 1 "$DEEP_REORG_DEPTH"); do
    replacement_address="$(regtest_get_new_address)"
    regtest_log "Mining hybrid replacement block ${round}/${DEEP_REORG_DEPTH} to address=${replacement_address}"
    regtest_mine_empty_block "$replacement_address"
  done

  replacement_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$target_height")"
  if [[ "$replacement_tip_hash" == "$original_tip_hash" ]]; then
    regtest_log "Hybrid reorg failed: replacement tip hash matches original tip hash"
    exit 1
  fi

  regtest_restart_balance_history
  regtest_wait_until_synced_height "$target_height"
  regtest_wait_until_block_commit_hash "$target_height" "$replacement_tip_hash"

  for receiver_address in "${tip_receiver_addresses[@]}"; do
    regtest_assert_address_balance_btc "$receiver_address" "$target_height" "0"
  done
  regtest_assert_address_balance_btc "$deep_receiver_address" "$target_height" "0"

  for outpoint in "${tip_outpoints[@]}"; do
    tip_txid="${outpoint%%:*}"
    tip_vout="${outpoint##*:}"
    regtest_assert_utxo_missing "$tip_txid" "$tip_vout"
  done
  regtest_assert_utxo_missing "$deep_txid" "$deep_vout"

  if [[ "$(regtest_get_snapshot_stable_hash)" != "$replacement_tip_hash" ]]; then
    regtest_log "Snapshot info did not converge to hybrid replacement tip hash"
    exit 1
  fi

  regtest_log "Restart hybrid reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"