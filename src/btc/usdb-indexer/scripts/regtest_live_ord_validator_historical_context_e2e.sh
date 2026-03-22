#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-historical-context-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29932}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29933}"
BH_RPC_PORT="${BH_RPC_PORT:-29910}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29920}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29930}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorhist}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-historical-e2e]"

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

write_validator_block_payload() {
  local payload_file="$1"
  local state_ref_resp="$2"
  local pass_snapshot_resp="$3"
  local pass_energy_resp="$4"

  python3 - "$payload_file" "$state_ref_resp" "$pass_snapshot_resp" "$pass_energy_resp" <<'PY'
import json
import pathlib
import sys

payload_file = pathlib.Path(sys.argv[1])
state_ref = json.loads(sys.argv[2])["result"]
pass_snapshot = json.loads(sys.argv[3])["result"]
pass_energy = json.loads(sys.argv[4])["result"]

payload = {
    "btc_external_state": {
        "height": state_ref["block_height"],
        "snapshot_id": state_ref["snapshot_info"]["snapshot_id"],
        "stable_block_hash": state_ref["snapshot_info"]["stable_block_hash"],
        "local_state_commit": state_ref["local_state_commit_info"]["local_state_commit"],
        "system_state_id": state_ref["system_state_info"]["system_state_id"],
    },
    "miner_pass": {
        "inscription_id": pass_snapshot["inscription_id"],
        "owner": pass_snapshot["owner"],
        "state": pass_snapshot["state"],
        "resolved_height": pass_snapshot["resolved_height"],
        "energy": pass_energy["energy"],
        "query_block_height": pass_energy["query_block_height"],
    },
}

payload_file.write_text(json.dumps(payload, indent=2) + "\n")
PY
}

validator_payload_expr() {
  local payload_file="$1"
  local expression="$2"

  python3 - "$payload_file" "$expression" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
expression = sys.argv[2]
print(eval(expression, {"__builtins__": {}}, {"data": payload}))
PY
}

validator_payload_context_json() {
  local payload_file="$1"

  python3 - "$payload_file" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
state = payload["btc_external_state"]
print(json.dumps({
    "requested_height": state["height"],
    "expected_state": {
        "snapshot_id": state["snapshot_id"],
        "stable_block_hash": state["stable_block_hash"],
        "local_state_commit": state["local_state_commit"],
        "system_state_id": state["system_state_id"],
    },
}))
PY
}

assert_validator_payload_success() {
  local payload_file="$1"

  local block_height pass_id context_json state_ref_params snapshot_params energy_params
  local expected_owner expected_state expected_energy resp

  block_height="$(validator_payload_expr "$payload_file" "data['btc_external_state']['height']")"
  pass_id="$(validator_payload_expr "$payload_file" "data['miner_pass']['inscription_id']")"
  expected_owner="$(validator_payload_expr "$payload_file" "data['miner_pass']['owner']")"
  expected_state="$(validator_payload_expr "$payload_file" "data['miner_pass']['state']")"
  expected_energy="$(validator_payload_expr "$payload_file" "data['miner_pass']['energy']")"
  context_json="$(validator_payload_context_json "$payload_file")"

  state_ref_params="$(build_context_params "$block_height" "$context_json")"
  snapshot_params="$(build_pass_snapshot_params "$pass_id" "$block_height" "$context_json")"
  energy_params="$(build_pass_energy_params "$pass_id" "$block_height" "$context_json")"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('block_height')" "$block_height"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('snapshot_info') or {}).get('snapshot_id')" \
    "$(validator_payload_expr "$payload_file" "data['btc_external_state']['snapshot_id']")"
  regtest_assert_json_expr "$resp" "((data.get('result') or {}).get('system_state_info') or {}).get('system_state_id')" \
    "$(validator_payload_expr "$payload_file" "data['btc_external_state']['system_state_id']")"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "$snapshot_params")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('inscription_id')" "$pass_id"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('owner')" "$expected_owner"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('state')" "$expected_state"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('resolved_height')" "$block_height"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "$energy_params")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('inscription_id')" "$pass_id"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('query_block_height')" "$block_height"
  regtest_assert_json_expr "$resp" "(data.get('result') or {}).get('energy')" "$expected_energy"
}

assert_validator_payload_snapshot_mismatch() {
  local payload_file="$1"

  local block_height pass_id context_json state_ref_params snapshot_params energy_params resp

  block_height="$(validator_payload_expr "$payload_file" "data['btc_external_state']['height']")"
  pass_id="$(validator_payload_expr "$payload_file" "data['miner_pass']['inscription_id']")"
  context_json="$(validator_payload_context_json "$payload_file")"

  state_ref_params="$(build_context_params "$block_height" "$context_json")"
  snapshot_params="$(build_pass_snapshot_params "$pass_id" "$block_height" "$context_json")"
  energy_params="$(build_pass_energy_params "$pass_id" "$block_height" "$context_json")"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_usdb_consensus_error "$resp" "-32042" "SNAPSHOT_ID_MISMATCH"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "$snapshot_params")"
  regtest_assert_usdb_consensus_error "$resp" "-32042" "SNAPSHOT_ID_MISMATCH"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "$energy_params")"
  regtest_assert_usdb_consensus_error "$resp" "-32042" "SNAPSHOT_ID_MISMATCH"
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
  local historical_height historical_hash replacement_address continue_address current_tip_height
  local state_ref_resp pass_snapshot_resp pass_energy_resp payload_file target_post_reorg_height

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

  mint_content_file="$WORK_DIR/usdb_validator_historical_mint.json"
  cat >"$mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF

  pass_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_content_file")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  current_tip_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  historical_height="$((current_tip_height - 1))"
  historical_hash="$(regtest_get_bitcoin_block_hash "$historical_height")"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$current_tip_height"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$current_tip_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready
  regtest_wait_usdb_state_ref_available "$historical_height"

  state_ref_resp="$(regtest_get_usdb_state_ref_response "$historical_height")"
  regtest_assert_json_expr "$state_ref_resp" "data.get('error') is None" "True"
  pass_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "[{\"inscription_id\":\"${pass_id}\",\"at_height\":${historical_height}}]")"
  regtest_assert_json_expr "$pass_snapshot_resp" "data.get('error') is None" "True"
  pass_energy_resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${pass_id}\",\"block_height\":${historical_height},\"mode\":\"at_or_before\"}]")"
  regtest_assert_json_expr "$pass_energy_resp" "data.get('error') is None" "True"

  payload_file="$WORK_DIR/ethw_validator_block_payload.json"
  write_validator_block_payload "$payload_file" "$state_ref_resp" "$pass_snapshot_resp" "$pass_energy_resp"
  regtest_log "Wrote validator block payload: ${payload_file}"

  regtest_log "Validator payload must validate at the original historical state"
  assert_validator_payload_success "$payload_file"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((historical_height + 1))"
  regtest_wait_until_usdb_synced_eq "$((historical_height + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Validator payload must still validate after BTC head advances"
  assert_validator_payload_success "$payload_file"

  regtest_log "Triggering same-height reorg below the validator payload height=${historical_height}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$historical_hash"
  target_post_reorg_height="$((historical_height + 2))"
  while [[ "$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)" -lt "$target_post_reorg_height" ]]; do
    replacement_address="$(regtest_get_new_address)"
    regtest_mine_empty_block "$replacement_address"
  done
  regtest_wait_until_balance_history_synced_eq "$target_post_reorg_height"
  regtest_wait_until_usdb_synced_eq "$target_post_reorg_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Old validator payload must fail after the historical BTC state changes"
  assert_validator_payload_snapshot_mismatch "$payload_file"

  regtest_log "USDB validator historical-context e2e test succeeded."
}

main "$@"
