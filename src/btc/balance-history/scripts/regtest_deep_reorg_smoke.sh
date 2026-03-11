#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-deep-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28432}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28433}"
BH_RPC_PORT="${BH_RPC_PORT:-28410}"
WALLET_NAME="${WALLET_NAME:-bhdeepreorg}"
TARGET_HEIGHT="${TARGET_HEIGHT:-45}"
SCENARIO_START_HEIGHT="${SCENARIO_START_HEIGHT:-45}"
REORG_DEPTH="${REORG_DEPTH:-3}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[deep-reorg-smoke]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  if (( REORG_DEPTH <= 0 )); then
    regtest_log "REORG_DEPTH must be positive"
    exit 1
  fi
  if (( TARGET_HEIGHT <= REORG_DEPTH )); then
    regtest_log "TARGET_HEIGHT must be greater than REORG_DEPTH"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height stable_prefix_height affected_height receiver_address txid original_affected_hash original_tip_hash replacement_tip_hash round replacement_address
  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  TARGET_HEIGHT=$((current_height + SCENARIO_START_HEIGHT))
  stable_prefix_height=$((TARGET_HEIGHT - REORG_DEPTH))
  affected_height=$((stable_prefix_height + 1))

  regtest_log "Mining stable prefix to height=${stable_prefix_height}"
  regtest_mine_blocks "$((stable_prefix_height - current_height))" "$mining_address"

  receiver_address="$(regtest_get_new_address)"
  regtest_log "Creating tracked transfer of ${SEND_AMOUNT_BTC} BTC to address=${receiver_address}"
  txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
  regtest_log "Created tracked txid=${txid}"

  regtest_mine_blocks 1 "$mining_address"
  if (( REORG_DEPTH > 1 )); then
    regtest_log "Mining remaining ${REORG_DEPTH}-1 tail blocks to reach target height=${TARGET_HEIGHT}"
    regtest_mine_blocks $((REORG_DEPTH - 1)) "$mining_address"
  fi

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$TARGET_HEIGHT"

  regtest_assert_address_balance_btc "$receiver_address" "$TARGET_HEIGHT" "$SEND_AMOUNT_BTC"

  original_affected_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$affected_height")"
  original_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$TARGET_HEIGHT")"
  regtest_log "Triggering deep reorg: affected_height=${affected_height}, original_affected_hash=${original_affected_hash}, original_tip_hash=${original_tip_hash}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_affected_hash"

  for round in $(seq 1 "$REORG_DEPTH"); do
    replacement_address="$(regtest_get_new_address)"
    regtest_log "Mining empty replacement block ${round}/${REORG_DEPTH} to address=${replacement_address}"
    regtest_mine_empty_block "$replacement_address"
  done

  replacement_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$TARGET_HEIGHT")"
  regtest_log "Replacement tip hash=${replacement_tip_hash}"
  if [[ "$replacement_tip_hash" == "$original_tip_hash" ]]; then
    regtest_log "Deep reorg failed: replacement tip hash matches original tip hash"
    exit 1
  fi

  regtest_wait_until_synced_height "$TARGET_HEIGHT"
  regtest_wait_until_block_commit_hash "$TARGET_HEIGHT" "$replacement_tip_hash"
  regtest_assert_address_balance_btc "$receiver_address" "$TARGET_HEIGHT" "0"

  if [[ "$(regtest_get_snapshot_stable_hash)" != "$replacement_tip_hash" ]]; then
    regtest_log "Snapshot info did not converge to replacement tip hash"
    exit 1
  fi

  regtest_log "Deep reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"