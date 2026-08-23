#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
export REPO_ROOT
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-spend-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/config-source}"
RESTORE_BALANCE_HISTORY_ROOT="${RESTORE_BALANCE_HISTORY_ROOT:-$WORK_DIR/restore}"
SNAPSHOT_BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-$WORK_DIR/snapshot-builder}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30132}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30133}"
BH_RPC_PORT="${BH_RPC_PORT:-30110}"
RESTORE_BH_RPC_PORT="${RESTORE_BH_RPC_PORT:-30111}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotspend}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/restore.log}"
export REGTEST_LOG_PREFIX="[exact-snapshot-spend]"

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
  regtest_require_cmd python3
  regtest_ensure_workspace_dirs
  mkdir -p "$SNAPSHOT_BUILDER_ROOT" "$RESTORE_BALANCE_HISTORY_ROOT"

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address tracked_address receiver_address tracked_txid tracked_vout
  local snapshot_height snapshot_hash snapshot_report artifact_dir snapshot_file snapshot_path
  local spend_raw spend_signed spend_txid spend_vout spend_height

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  tracked_address="$(regtest_get_new_address)"
  receiver_address="$(regtest_get_new_address)"

  tracked_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -rpcwallet="$WALLET_NAME" sendtoaddress "$tracked_address" 1.0)"
  regtest_mine_blocks 1 "$mining_address"
  tracked_vout="$(regtest_get_tx_vout_for_address "$tracked_txid" "$tracked_address")"
  regtest_lock_wallet_outpoint "$tracked_txid" "$tracked_vout"

  # Move the tracked output's block into the stable range without changing its spend state.
  regtest_mine_blocks 5 "$mining_address"
  snapshot_height=$(( $($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount) - 5 ))
  snapshot_hash="$(regtest_get_block_hash_by_height "$snapshot_height")"
  snapshot_report="$WORK_DIR/create-snapshot.json"

  regtest_create_balance_history_config_at "$BALANCE_HISTORY_ROOT" "$BH_RPC_PORT"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$snapshot_height" \
    --expected-block-hash "$snapshot_hash" \
    --config "$BALANCE_HISTORY_ROOT/config.toml" \
    --poll-interval-secs 1 >"$snapshot_report"
  regtest_assert_json_file "$snapshot_report" "data['utxo_count'] > 0" "True"

  artifact_dir="$(json_field "$snapshot_report" artifact_dir)"
  snapshot_file="$(json_field "$snapshot_report" snapshot_file)"
  snapshot_path="$SNAPSHOT_BUILDER_ROOT/$artifact_dir/$snapshot_file"
  if [[ ! -f "$snapshot_path" ]]; then
    regtest_log "Generated snapshot file is missing: ${snapshot_path}"
    exit 1
  fi

  BALANCE_HISTORY_ROOT="$RESTORE_BALANCE_HISTORY_ROOT"
  BH_RPC_PORT="$RESTORE_BH_RPC_PORT"
  regtest_create_balance_history_config
  regtest_run_balance_history_cli "$RESTORE_BALANCE_HISTORY_ROOT" install-snapshot \
    --file "$snapshot_path"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$snapshot_height"
  regtest_assert_utxo_value_sat "$tracked_txid" "$tracked_vout" "100000000"

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -rpcwallet="$WALLET_NAME" lockunspent true \
    "[{\"txid\":\"${tracked_txid}\",\"vout\":${tracked_vout}}]" >/dev/null
  spend_raw="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    createrawtransaction "[{\"txid\":\"${tracked_txid}\",\"vout\":${tracked_vout}}]" \
    "{\"${receiver_address}\":0.9999}")"
  spend_signed="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -rpcwallet="$WALLET_NAME" signrawtransactionwithwallet "$spend_raw" \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  spend_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    sendrawtransaction "$spend_signed")"
  spend_vout="$(regtest_get_tx_vout_for_address "$spend_txid" "$receiver_address")"

  regtest_mine_blocks 1 "$mining_address"
  spend_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_mine_blocks 5 "$mining_address"
  regtest_wait_until_synced_height "$spend_height"

  regtest_assert_utxo_missing "$tracked_txid" "$tracked_vout"
  regtest_assert_utxo_value_sat "$spend_txid" "$spend_vout" "99990000"
  regtest_log "Installed snapshot successfully spent a pre-snapshot UTXO and continued to height=${spend_height}."
}

main "$@"
