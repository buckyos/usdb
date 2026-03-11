#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-history-balance-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28932}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28933}"
BH_RPC_PORT="${BH_RPC_PORT:-28910}"
WALLET_NAME="${WALLET_NAME:-bhhistoryoracle}"
ADDRESS_COUNT="${ADDRESS_COUNT:-8}"
UNTRACKED_ADDRESS_COUNT="${UNTRACKED_ADDRESS_COUNT:-4}"
BLOCK_COUNT="${BLOCK_COUNT:-18}"
TXS_PER_BLOCK="${TXS_PER_BLOCK:-3}"
CHECK_INTERVAL="${CHECK_INTERVAL:-3}"
SEED="${SEED:-20260311}"
SEND_AMOUNTS_BTC="${SEND_AMOUNTS_BTC:-0.10 0.25 0.50 1.00}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
ORACLE_STATE_FILE="${ORACLE_STATE_FILE:-$WORK_DIR/history_oracle.json}"
ORACLE_PY="${SCRIPT_DIR}/regtest_balance_oracle.py"
REGTEST_LOG_PREFIX="[history-balance-oracle]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  if (( ADDRESS_COUNT <= 0 )); then
    regtest_log "ADDRESS_COUNT must be positive"
    exit 1
  fi
  if (( BLOCK_COUNT <= 0 )); then
    regtest_log "BLOCK_COUNT must be positive"
    exit 1
  fi
  if (( TXS_PER_BLOCK <= 0 )); then
    regtest_log "TXS_PER_BLOCK must be positive"
    exit 1
  fi
  if (( CHECK_INTERVAL <= 0 )); then
    regtest_log "CHECK_INTERVAL must be positive"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address current_height start_height current_block_height
  local -a tracked_addresses=()
  local -a untracked_addresses=()
  local -a send_amounts=()
  local address_json receiver_address amount_btc txid block_index tx_index sample_height expected_sat

  RANDOM="$SEED"
  read -r -a send_amounts <<<"$SEND_AMOUNTS_BTC"
  if (( ${#send_amounts[@]} == 0 )); then
    regtest_log "SEND_AMOUNTS_BTC must contain at least one amount"
    exit 1
  fi

  regtest_log "Scenario seed=${SEED}, address_count=${ADDRESS_COUNT}, untracked_address_count=${UNTRACKED_ADDRESS_COUNT}, block_count=${BLOCK_COUNT}, txs_per_block=${TXS_PER_BLOCK}, check_interval=${CHECK_INTERVAL}"

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  start_height="$current_height"

  for _ in $(seq 1 "$ADDRESS_COUNT"); do
    tracked_addresses+=("$(regtest_get_new_address)")
  done
  for _ in $(seq 1 "$UNTRACKED_ADDRESS_COUNT"); do
    untracked_addresses+=("$(regtest_get_new_address)")
  done

  address_json="$(printf '%s\n' "${tracked_addresses[@]}" | python3 -c 'import json,sys; print(json.dumps([line.strip() for line in sys.stdin if line.strip()]))')"
  python3 "$ORACLE_PY" init --state-file "$ORACLE_STATE_FILE" --start-height "$start_height" --addresses-json "$address_json"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$start_height"

  for block_index in $(seq 1 "$BLOCK_COUNT"); do
    for tx_index in $(seq 1 "$TXS_PER_BLOCK"); do
      amount_btc="${send_amounts[$((RANDOM % ${#send_amounts[@]}))]}"
      if (( ${#untracked_addresses[@]} > 0 )) && (( RANDOM % 4 == 0 )); then
        receiver_address="${untracked_addresses[$((RANDOM % ${#untracked_addresses[@]}))]}"
      else
        receiver_address="${tracked_addresses[$((RANDOM % ${#tracked_addresses[@]}))]}"
      fi

      txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$receiver_address" "$amount_btc")"
      regtest_log "Block ${block_index}/${BLOCK_COUNT}: queued tx ${tx_index}/${TXS_PER_BLOCK}, txid=${txid}, receiver=${receiver_address}, amount_btc=${amount_btc}"
    done

    regtest_mine_blocks 1 "$mining_address"
    current_block_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
    regtest_get_block_json_by_height "$current_block_height" | python3 "$ORACLE_PY" apply-block --state-file "$ORACLE_STATE_FILE"
    regtest_wait_until_synced_height "$current_block_height"

    if (( block_index % CHECK_INTERVAL == 0 )) || (( block_index == BLOCK_COUNT )); then
      sample_height=$((start_height + (RANDOM % (current_block_height - start_height + 1))))
      regtest_log "Checkpoint validation at height=${current_block_height}, sampled_history_height=${sample_height}"
      for receiver_address in "${tracked_addresses[@]}"; do
        expected_sat="$(python3 "$ORACLE_PY" get-balance --state-file "$ORACLE_STATE_FILE" --address "$receiver_address" --height "$current_block_height")"
        regtest_assert_address_balance_sat "$receiver_address" "$current_block_height" "$expected_sat"

        expected_sat="$(python3 "$ORACLE_PY" get-balance --state-file "$ORACLE_STATE_FILE" --address "$receiver_address" --height "$sample_height")"
        regtest_assert_address_balance_sat "$receiver_address" "$sample_height" "$expected_sat"
      done
    fi
  done

  regtest_log "Running final full history verification across all tracked addresses and scenario heights"
  for current_block_height in $(seq "$start_height" $((start_height + BLOCK_COUNT))); do
    for receiver_address in "${tracked_addresses[@]}"; do
      expected_sat="$(python3 "$ORACLE_PY" get-balance --state-file "$ORACLE_STATE_FILE" --address "$receiver_address" --height "$current_block_height")"
      regtest_assert_address_balance_sat "$receiver_address" "$current_block_height" "$expected_sat"
    done
  done

  regtest_log "History balance oracle test succeeded."
  regtest_log "Oracle state: ${ORACLE_STATE_FILE}"
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"