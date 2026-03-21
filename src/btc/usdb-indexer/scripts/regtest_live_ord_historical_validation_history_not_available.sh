#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-historical-validation-gap-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29872}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29873}"
BH_RPC_PORT="${BH_RPC_PORT:-29870}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29880}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29890}"
WALLET_NAME="${WALLET_NAME:-usdbhistexactgap}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-hist-gap-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-hist-gap-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-historical-validation-gap]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

build_context_params() {
  local block_height="$1"
  local context_json="$2"

  python3 - "$block_height" "$context_json" <<'PY'
import json
import sys

block_height = int(sys.argv[1])
context = json.loads(sys.argv[2])
print(json.dumps([{
    "block_height": block_height,
    "context": context,
}]))
PY
}

build_pass_snapshot_params() {
  local inscription_id="$1"
  local block_height="$2"
  local context_json="$3"

  python3 - "$inscription_id" "$block_height" "$context_json" <<'PY'
import json
import sys

inscription_id = sys.argv[1]
block_height = int(sys.argv[2])
context = json.loads(sys.argv[3])
print(json.dumps([{
    "inscription_id": inscription_id,
    "at_height": block_height,
    "context": context,
}]))
PY
}

build_pass_energy_params() {
  local inscription_id="$1"
  local block_height="$2"
  local context_json="$3"

  python3 - "$inscription_id" "$block_height" "$context_json" <<'PY'
import json
import sys

inscription_id = sys.argv[1]
block_height = int(sys.argv[2])
context = json.loads(sys.argv[3])
print(json.dumps([{
    "inscription_id": inscription_id,
    "block_height": block_height,
    "mode": "at_or_before",
    "context": context,
}]))
PY
}

assert_historical_context_success() {
  local pass_id="$1"
  local block_height="$2"
  local context_json="$3"

  local state_ref_params snapshot_params energy_params resp
  state_ref_params="$(build_context_params "$block_height" "$context_json")"
  snapshot_params="$(build_pass_snapshot_params "$pass_id" "$block_height" "$context_json")"
  energy_params="$(build_pass_energy_params "$pass_id" "$block_height" "$context_json")"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "$snapshot_params")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "$energy_params")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
}

assert_historical_context_history_not_available() {
  local pass_id="$1"
  local block_height="$2"
  local context_json="$3"

  local state_ref_params snapshot_params energy_params resp
  state_ref_params="$(build_context_params "$block_height" "$context_json")"
  snapshot_params="$(build_pass_snapshot_params "$pass_id" "$block_height" "$context_json")"
  energy_params="$(build_pass_energy_params "$pass_id" "$block_height" "$context_json")"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_usdb_consensus_error "$resp" "-32049" "HISTORY_NOT_AVAILABLE"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "$snapshot_params")"
  regtest_assert_usdb_consensus_error "$resp" "-32049" "HISTORY_NOT_AVAILABLE"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "$energy_params")"
  regtest_assert_usdb_consensus_error "$resp" "-32049" "HISTORY_NOT_AVAILABLE"
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  if [[ ! -x "$ORD_BIN" ]]; then
    echo "Missing required ORD_BIN executable: $ORD_BIN" >&2
    exit 1
  fi
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_assert_ord_server_port_available

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local miner_address ord_receive_address mint_content_file pass_id
  local historical_height state_ref_resp snapshot_id stable_block_hash local_state_commit
  local system_state_id context_json continue_address

  miner_address="$(regtest_get_new_address)"
  regtest_log "Premining ${PREMINE_BLOCKS} blocks to address=${miner_address}"
  regtest_mine_blocks "$PREMINE_BLOCKS" "$miner_address"

  regtest_start_ord_server
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_prepare_ord_wallets

  ord_receive_address="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  regtest_fund_address "$ord_receive_address" "$FUND_ORD_AMOUNT_BTC"
  regtest_mine_blocks "$FUND_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind

  mint_content_file="$WORK_DIR/usdb_hist_validation_gap_mint.json"
  cat >"$mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF

  pass_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_content_file")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  historical_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$historical_height"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$historical_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  state_ref_resp="$(regtest_get_usdb_state_ref_response "$historical_height")"
  regtest_assert_json_expr "$state_ref_resp" "data.get('error') is None" "True"
  snapshot_id="$(regtest_json_expr "$state_ref_resp" "((data.get('result') or {}).get('snapshot_info') or {}).get('snapshot_id', '')")"
  stable_block_hash="$(regtest_json_expr "$state_ref_resp" "((data.get('result') or {}).get('snapshot_info') or {}).get('stable_block_hash', '')")"
  local_state_commit="$(regtest_json_expr "$state_ref_resp" "((data.get('result') or {}).get('local_state_commit_info') or {}).get('local_state_commit', '')")"
  system_state_id="$(regtest_json_expr "$state_ref_resp" "((data.get('result') or {}).get('system_state_info') or {}).get('system_state_id', '')")"
  context_json="$(regtest_build_consensus_context_json "$historical_height" "$snapshot_id" "$stable_block_hash" "$local_state_commit" "$system_state_id")"

  assert_historical_context_success "$pass_id" "$historical_height" "$context_json"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_blocks 1 "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((historical_height + 1))"
  regtest_wait_until_usdb_synced_eq "$((historical_height + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Historical context remains valid after head advance"
  assert_historical_context_success "$pass_id" "$historical_height" "$context_json"

  regtest_log "Deleting one retained historical active-balance snapshot row to simulate auxiliary state loss"
  regtest_stop_usdb_indexer
  regtest_usdb_db_exec "DELETE FROM active_balance_snapshots WHERE block_height = ${historical_height};"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$((historical_height + 1))"
  regtest_wait_usdb_consensus_ready

  regtest_log "Historical queries within the retained window but missing auxiliary state must return HISTORY_NOT_AVAILABLE"
  assert_historical_context_history_not_available "$pass_id" "$historical_height" "$context_json"

  regtest_log "USDB historical validation history-not-available test succeeded."
}

main "$@"
