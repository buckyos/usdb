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
export REPO_ROOT REGTEST_LOG_PREFIX

# shellcheck source=regtest_lib.sh
source "${SCRIPT_DIR}/regtest_lib.sh"

oracle_apply_through_tip() {
  local oracle_height tip_height height
  oracle_height="$(python3 "$ORACLE_PY" get-current-height --state-file "$ORACLE_STATE_FILE")"
  tip_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  for height in $(seq $((oracle_height + 1)) "$tip_height"); do
    regtest_get_block_json_by_height "$height" \
      | python3 "$ORACLE_PY" apply-block --state-file "$ORACLE_STATE_FILE"
  done
}

oracle_assert_address_state() {
  local address="$1"
  local height="$2"
  local expected_sat expected_delta_json script_hash response

  expected_sat="$(python3 "$ORACLE_PY" get-balance --state-file "$ORACLE_STATE_FILE" --address "$address" --height "$height")"
  regtest_assert_address_balance_sat "$address" "$height" "$expected_sat"

  expected_delta_json="$(python3 "$ORACLE_PY" get-delta --state-file "$ORACLE_STATE_FILE" --address "$address" --height "$height")"
  script_hash="$(regtest_address_to_script_hash "$address")"
  response="$(regtest_rpc_call_balance_history "get_address_balance_delta" "[{\"script_hash\":\"${script_hash}\",\"block_height\":${height},\"block_range\":null}]")"
  python3 - "$address" "$height" "$expected_delta_json" "$response" <<'PY'
import json
import sys

address = sys.argv[1]
height = int(sys.argv[2])
expected = json.loads(sys.argv[3])
response = json.loads(sys.argv[4])
if response.get("error"):
    raise SystemExit(f"delta RPC failed for {address} at {height}: {response['error']}")
rows = response.get("result") or []
actual = rows[0] if rows else None
if not expected["present"]:
    if actual is not None:
        raise SystemExit(f"expected no delta for {address} at {height}, got {actual}")
elif actual is None:
    raise SystemExit(f"expected delta for {address} at {height}, got none")
elif actual["delta"] != expected["delta"] or actual["balance"] != expected["balance"]:
    raise SystemExit(
        f"delta mismatch for {address} at {height}: expected={expected}, actual={actual}"
    )
PY
}

oracle_assert_address_history() {
  local address="$1"
  local start_height="$2"
  local end_height="$3"
  local script_hash expected_history expected_summary history_response summary_response

  script_hash="$(regtest_address_to_script_hash "$address")"
  expected_history="$(python3 "$ORACLE_PY" get-history --state-file "$ORACLE_STATE_FILE" --address "$address" --start-height "$start_height" --end-height "$end_height")"
  expected_summary="$(python3 "$ORACLE_PY" get-summary --state-file "$ORACLE_STATE_FILE" --address "$address" --start-height "$start_height" --end-height "$end_height")"
  history_response="$(regtest_rpc_call_balance_history "get_address_balance_delta" "[{\"script_hash\":\"${script_hash}\",\"block_height\":null,\"block_range\":{\"start\":${start_height},\"end\":${end_height}}}]")"
  summary_response="$(regtest_rpc_call_balance_history "get_address_balance_summary" "[{\"script_hash\":\"${script_hash}\",\"block_range\":{\"start\":${start_height},\"end\":${end_height}}}]")"

  python3 - "$address" "$expected_history" "$history_response" "$expected_summary" "$summary_response" <<'PY'
import json
import sys

address = sys.argv[1]
expected_history = json.loads(sys.argv[2])
history_response = json.loads(sys.argv[3])
expected_summary = json.loads(sys.argv[4])
summary_response = json.loads(sys.argv[5])
if history_response.get("error"):
    raise SystemExit(f"history RPC failed for {address}: {history_response['error']}")
if history_response.get("result") != expected_history:
    raise SystemExit(
        f"history mismatch for {address}: expected={expected_history}, actual={history_response.get('result')}"
    )
if summary_response.get("error"):
    raise SystemExit(f"summary RPC failed for {address}: {summary_response['error']}")
if summary_response.get("result") != expected_summary:
    raise SystemExit(
        f"summary mismatch for {address}: expected={expected_summary}, actual={summary_response.get('result')}"
    )
PY
}

oracle_assert_utxos() {
  local state outpoint address value response expected_script_hash
  for state in live spent; do
    while IFS=$'\t' read -r outpoint address value; do
      [[ -n "$outpoint" ]] || continue
      response="$(regtest_rpc_call_balance_history "get_live_utxo" "[\"${outpoint}\"]")"
      expected_script_hash="$(regtest_address_to_script_hash "$address")"
      python3 - "$state" "$outpoint" "$expected_script_hash" "$value" "$response" <<'PY'
import json
import sys

state, outpoint, expected_script_hash, value, encoded_response = sys.argv[1:]
response = json.loads(encoded_response)
if response.get("error"):
    raise SystemExit(f"UTXO RPC failed for {outpoint}: {response['error']}")
actual = response.get("result")
if state == "spent":
    if actual is not None:
        raise SystemExit(f"spent outpoint {outpoint} is still live: {actual}")
elif actual is None:
    raise SystemExit(f"live outpoint {outpoint} is missing")
elif actual["script_hash"] != expected_script_hash or actual["value"] != int(value):
    raise SystemExit(
        f"live outpoint mismatch for {outpoint}: expected_hash={expected_script_hash}, expected_value={value}, actual={actual}"
    )
PY
    done < <(
      python3 "$ORACLE_PY" dump-utxos --state-file "$ORACLE_STATE_FILE" --state "$state" \
        | python3 -c 'import json,sys
for row in json.load(sys.stdin):
    print("{}\t{}\t{}".format(row["outpoint"], row["address"], row["value"]))'
    )
  done
}

oracle_assert_script_registry() {
  local address expected_found script_hash response
  for address in "$@"; do
    expected_found="$(python3 "$ORACLE_PY" is-seen --state-file "$ORACLE_STATE_FILE" --address "$address")"
    script_hash="$(regtest_address_to_script_hash "$address")"
    response="$(regtest_rpc_call_balance_history "resolve_script_hashes" "[{\"script_hashes\":[\"${script_hash}\"],\"include_script_pubkey\":true}]")"
    python3 - "$address" "$script_hash" "$expected_found" "$response" <<'PY'
import json
import sys

address, script_hash, expected_found, encoded_response = sys.argv[1:]
response = json.loads(encoded_response)
if response.get("error"):
    raise SystemExit(f"script registry RPC failed for {address}: {response['error']}")
item = response["result"]["items"][0]
if item["script_hash"] != script_hash or item["found"] != (expected_found == "true"):
    raise SystemExit(
        f"script registry mismatch for {address}: expected_found={expected_found}, actual={item}"
    )
if item["found"] and item["address"] != address:
    raise SystemExit(f"script registry address mismatch for {address}: {item}")
PY
  done
}

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

  local mining_address current_height start_height current_block_height stable_height stable_lag final_event_height
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
  regtest_wait_until_height_is_stable "$start_height" "$mining_address"
  stable_lag="$(regtest_get_snapshot_stable_lag)"
  oracle_apply_through_tip

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
    final_event_height="$current_block_height"
    oracle_apply_through_tip
    stable_height=$((current_block_height - stable_lag))
    regtest_wait_until_synced_height "$stable_height"
    if [[ "$(regtest_get_block_commit_hash "$stable_height")" != "$(regtest_get_block_hash_by_height "$stable_height")" ]]; then
      regtest_log "Block commit hash mismatch at stable height=${stable_height}"
      exit 1
    fi

    if (( block_index % CHECK_INTERVAL == 0 )) || (( block_index == BLOCK_COUNT )); then
      sample_height=$((RANDOM % (stable_height + 1)))
      regtest_log "Checkpoint validation at stable_height=${stable_height}, sampled_history_height=${sample_height}"
      for receiver_address in "${tracked_addresses[@]}"; do
        oracle_assert_address_state "$receiver_address" "$stable_height"
        oracle_assert_address_state "$receiver_address" "$sample_height"
      done
    fi
  done

  regtest_wait_until_height_is_stable "$final_event_height" "$mining_address"
  oracle_apply_through_tip

  regtest_log "Running final full history verification across all tracked addresses and scenario heights"
  for current_block_height in $(seq "$start_height" "$final_event_height"); do
    for receiver_address in "${tracked_addresses[@]}"; do
      oracle_assert_address_state "$receiver_address" "$current_block_height"
    done
  done

  regtest_log "Cross-checking complete movement ranges, summaries, UTXOs, and script registry"
  for receiver_address in "${tracked_addresses[@]}"; do
    oracle_assert_address_history "$receiver_address" "$start_height" "$((final_event_height + 1))"
  done
  oracle_assert_utxos
  oracle_assert_script_registry "${tracked_addresses[@]}"

  regtest_log "History balance oracle test succeeded."
  regtest_log "Oracle state: ${ORACLE_STATE_FILE}"
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"
