#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-multi-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28332}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28333}"
BH_RPC_PORT="${BH_RPC_PORT:-28310}"
WALLET_NAME="${WALLET_NAME:-bhmultireorg}"
TARGET_HEIGHT="${TARGET_HEIGHT:-40}"
SCENARIO_START_HEIGHT="${SCENARIO_START_HEIGHT:-40}"
REORG_ROUNDS="${REORG_ROUNDS:-2}"
SEND_AMOUNT_BTC="${SEND_AMOUNT_BTC:-1.25}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[multi-reorg-smoke]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address
  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  local current_height blocks_to_target
  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  TARGET_HEIGHT=$((current_height + SCENARIO_START_HEIGHT))
  blocks_to_target=$((TARGET_HEIGHT - current_height))
  regtest_log "Mining ${blocks_to_target} scenario blocks to reach target height=${TARGET_HEIGHT}"
  regtest_mine_blocks "$blocks_to_target" "$mining_address"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$TARGET_HEIGHT"

  local round current_height confirmed_height current_hash service_hash replacement_hash replacement_address receiver_address txid snapshot_hash
  for round in $(seq 1 "$REORG_ROUNDS"); do
    current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    receiver_address="$(regtest_get_new_address)"
    regtest_log "Round ${round}/${REORG_ROUNDS}: sending ${SEND_AMOUNT_BTC} BTC to address=${receiver_address}"
    txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$SEND_AMOUNT_BTC")"
    regtest_log "Round ${round}/${REORG_ROUNDS}: created txid=${txid}"
    regtest_mine_blocks 1 "$mining_address"

    confirmed_height=$((current_height + 1))
    regtest_wait_until_synced_height "$confirmed_height"
    regtest_assert_address_balance_btc "$receiver_address" "$confirmed_height" "$SEND_AMOUNT_BTC"

    current_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$confirmed_height")"
    service_hash="$(regtest_get_block_commit_hash "$confirmed_height")"

    regtest_log "Round ${round}/${REORG_ROUNDS}: node_hash=${current_hash}, service_hash=${service_hash}"
    if [[ "$service_hash" != "$current_hash" ]]; then
      regtest_log "Service hash mismatch before reorg round ${round}"
      exit 1
    fi

    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$current_hash"
    replacement_address="$(regtest_get_new_address)"
    regtest_log "Round ${round}/${REORG_ROUNDS}: mining empty replacement block to address=${replacement_address}"
    regtest_mine_empty_block "$replacement_address"

    replacement_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$confirmed_height")"
    regtest_log "Round ${round}/${REORG_ROUNDS}: replacement_hash=${replacement_hash}"
    if [[ "$replacement_hash" == "$current_hash" ]]; then
      regtest_log "Reorg round ${round} failed: replacement hash matches previous hash"
      exit 1
    fi

    regtest_wait_until_block_commit_hash "$confirmed_height" "$replacement_hash"
    regtest_assert_address_balance_btc "$receiver_address" "$confirmed_height" "0"

    snapshot_hash="$(regtest_get_snapshot_stable_hash)"
    regtest_log "Round ${round}/${REORG_ROUNDS}: stable_block_hash=${snapshot_hash}"
    if [[ "$snapshot_hash" != "$replacement_hash" ]]; then
      regtest_log "Snapshot info did not converge after round ${round}"
      exit 1
    fi
  done

  regtest_log "Multi reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"