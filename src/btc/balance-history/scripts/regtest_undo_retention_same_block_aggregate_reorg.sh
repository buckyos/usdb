#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-undo-retention-same-block-aggregate-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30672}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30673}"
BH_RPC_PORT="${BH_RPC_PORT:-30650}"
WALLET_NAME="${WALLET_NAME:-bhundosameblockaggregate}"
UNDO_RETENTION_BLOCKS="${UNDO_RETENTION_BLOCKS:-2}"
UNDO_CLEANUP_INTERVAL_BLOCKS="${UNDO_CLEANUP_INTERVAL_BLOCKS:-1}"
PRUNE_ADVANCE_BLOCKS="${PRUNE_ADVANCE_BLOCKS:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
BALANCE_HISTORY_SERVICE_LOG_FILE="${BALANCE_HISTORY_SERVICE_LOG_FILE:-$BALANCE_HISTORY_ROOT/logs/balance-history_rCURRENT.log}"
REGTEST_LOG_PREFIX="[undo-retention-same-block-aggregate-reorg]"

source "${SCRIPT_DIR}/regtest_lib.sh"

regtest_assert_json_expr() {
  local response="$1"
  local expression="$2"
  local expected="$3"
  local actual

  actual="$(printf '%s' "$response" | python3 -c "import json,sys; data=json.load(sys.stdin); print(${expression})")"
  regtest_log "RPC assertion: expr=${expression}, expected=${expected}, actual=${actual}"
  if [[ "$actual" != "$expected" ]]; then
    regtest_log "RPC assertion failed. response=${response}"
    exit 1
  fi
}

regtest_build_outputs_json() {
  python3 - "$@" <<'PY'
import json
import sys

args = sys.argv[1:]
if len(args) % 2 != 0:
    raise SystemExit("expected alternating address/amount pairs")

outputs = {}
for idx in range(0, len(args), 2):
    outputs[args[idx]] = args[idx + 1]

print(json.dumps(outputs))
PY
}

regtest_spend_multi_input() {
  local inputs_json="$1"
  local outputs_json="$2"
  local raw_tx signed_tx spend_txid

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    lockunspent true "$inputs_json" >/dev/null
  raw_tx="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    createrawtransaction "$inputs_json" "$outputs_json")"
  signed_tx="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    signrawtransactionwithwallet "$raw_tx" | regtest_json_extract_python 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  spend_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" sendrawtransaction "$signed_tx")"
  regtest_log "Created aggregate spend transaction txid=${spend_txid}, inputs=${inputs_json}, outputs=${outputs_json}"
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_require_cmd grep

  if (( PRUNE_ADVANCE_BLOCKS <= 0 )); then
    regtest_log "PRUNE_ADVANCE_BLOCKS must be positive"
    exit 1
  fi

  local mining_address address_a address_b address_c
  local script_hash_a script_hash_b script_hash_c
  local stable_prefix_height warmup_target_height funded_height aggregate_height
  local fund_txid vout_a vout_b inputs_json outputs_json bonus_txid
  local original_tip_hash replacement_tip_hash replacement_address round resp current_height

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  current_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  stable_prefix_height=$((current_height + 45))

  regtest_log "Mining stable prefix to height=${stable_prefix_height}"
  regtest_mine_blocks "$((stable_prefix_height - current_height))" "$mining_address"

  address_a="$(regtest_get_new_address)"
  address_b="$(regtest_get_new_address)"
  address_c="$(regtest_get_new_address)"
  script_hash_a="$(regtest_address_to_script_hash "$address_a")"
  script_hash_b="$(regtest_address_to_script_hash "$address_b")"
  script_hash_c="$(regtest_address_to_script_hash "$address_c")"

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

  fund_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    sendmany "" "$(regtest_build_outputs_json "$address_a" "1.0" "$address_b" "0.6")")"
  regtest_log "Created retained-window dual-funding transaction txid=${fund_txid}"
  regtest_mine_blocks 1 "$mining_address"
  funded_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$funded_height"

  vout_a="$(regtest_get_tx_vout_for_address "$fund_txid" "$address_a")"
  vout_b="$(regtest_get_tx_vout_for_address "$fund_txid" "$address_b")"
  regtest_lock_wallet_outpoint "$fund_txid" "$vout_a"
  regtest_lock_wallet_outpoint "$fund_txid" "$vout_b"

  inputs_json="[{\"txid\":\"${fund_txid}\",\"vout\":${vout_a}},{\"txid\":\"${fund_txid}\",\"vout\":${vout_b}}]"
  outputs_json="$(regtest_build_outputs_json "$address_c" "1.10000000" "$address_a" "0.49998000")"
  regtest_spend_multi_input "$inputs_json" "$outputs_json"
  bonus_txid="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" sendtoaddress "$address_a" 0.05)"
  regtest_log "Created retained-window same-block bonus payment txid=${bonus_txid}"
  regtest_mine_blocks 1 "$mining_address"
  aggregate_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_synced_height "$aggregate_height"
  original_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$aggregate_height")"

  regtest_assert_address_balance_sat "$address_a" "$aggregate_height" "54998000"
  regtest_assert_address_balance_sat "$address_b" "$aggregate_height" "0"
  regtest_assert_address_balance_sat "$address_c" "$aggregate_height" "110000000"

  resp="$(regtest_rpc_call_balance_history "get_address_balance_delta" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":${aggregate_height},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "1"
  regtest_assert_json_expr "$resp" "data['result'][0]['delta']" "-45002000"
  regtest_assert_json_expr "$resp" "data['result'][0]['balance']" "54998000"

  regtest_log "Stopping balance-history before retained-window aggregate reorg: funded_height=${funded_height}, aggregate_height=${aggregate_height}, original_tip_hash=${original_tip_hash}"
  regtest_stop_balance_history

  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_tip_hash"
  regtest_mine_empty_block "$(regtest_get_new_address)"
  replacement_tip_hash="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$aggregate_height")"
  if [[ "$replacement_tip_hash" == "$original_tip_hash" ]]; then
    regtest_log "Undo-retention aggregate reorg failed: replacement tip hash matches original tip hash"
    exit 1
  fi

  regtest_restart_balance_history
  regtest_wait_until_synced_height "$aggregate_height"
  regtest_wait_until_block_commit_hash "$aggregate_height" "$replacement_tip_hash"

  if ! grep -q "BTC reorg detected, rolling back local balance-history state" "$BALANCE_HISTORY_SERVICE_LOG_FILE"; then
    regtest_log "Expected reorg rollback log entry was not found"
    exit 1
  fi

  regtest_assert_address_balance_sat "$address_a" "$aggregate_height" "100000000"
  regtest_assert_address_balance_sat "$address_b" "$aggregate_height" "60000000"
  regtest_assert_address_balance_sat "$address_c" "$aggregate_height" "0"

  resp="$(regtest_rpc_call_balance_history "get_address_balance" "[{\"script_hash\":\"${script_hash_a}\",\"block_height\":null,\"block_range\":{\"start\":${funded_height},\"end\":$((aggregate_height + 1))}}]")"
  regtest_assert_json_expr "$resp" "len(data['result'])" "1"
  regtest_assert_json_expr "$resp" "data['result'][0]['block_height']" "$funded_height"
  regtest_assert_json_expr "$resp" "data['result'][0]['delta']" "100000000"

  resp="$(regtest_rpc_call_balance_history "get_addresses_balances_delta" "[{\"script_hashes\":[\"${script_hash_a}\",\"${script_hash_b}\",\"${script_hash_c}\"],\"block_height\":${aggregate_height},\"block_range\":null}]")"
  regtest_assert_json_expr "$resp" "data['result'][0][0] is None" "True"
  regtest_assert_json_expr "$resp" "data['result'][1][0] is None" "True"
  regtest_assert_json_expr "$resp" "data['result'][2][0] is None" "True"

  if [[ "$(regtest_get_snapshot_stable_hash)" != "$replacement_tip_hash" ]]; then
    regtest_log "Snapshot info did not converge to retained-window replacement tip hash"
    exit 1
  fi

  regtest_log "Undo retention same-block aggregate reorg test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"