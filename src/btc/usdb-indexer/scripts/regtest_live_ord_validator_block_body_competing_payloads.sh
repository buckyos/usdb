#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-competing-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29852}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29853}"
BH_RPC_PORT="${BH_RPC_PORT:-29870}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29880}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29890}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorcompeting}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-competing-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-competing-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-competing]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

write_validator_payload_for_pass_at_height() {
  local payload_file="$1"
  local inscription_id="$2"
  local block_height="$3"
  local state_ref_resp pass_snapshot_resp pass_energy_resp

  regtest_wait_usdb_state_ref_available "$block_height"
  state_ref_resp="$(regtest_get_usdb_state_ref_response "$block_height")"
  regtest_assert_json_expr "$state_ref_resp" "data.get('error') is None" "True"

  pass_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "[{\"inscription_id\":\"${inscription_id}\",\"at_height\":${block_height}}]")"
  regtest_assert_json_expr "$pass_snapshot_resp" "data.get('error') is None" "True"

  pass_energy_resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${inscription_id}\",\"block_height\":${block_height},\"mode\":\"at_or_before\"}]")"
  regtest_assert_json_expr "$pass_energy_resp" "data.get('error') is None" "True"

  regtest_write_validator_payload_v1 "$payload_file" "$state_ref_resp" "$pass_snapshot_resp" "$pass_energy_resp"
}

build_payload_context_with_requested_height() {
  local payload_file="$1"
  local requested_height="$2"
  local snapshot_id stable_block_hash local_state_commit system_state_id

  snapshot_id="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['snapshot_id']")"
  stable_block_hash="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['stable_block_hash']")"
  local_state_commit="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['local_state_commit']")"
  system_state_id="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['system_state_id']")"

  regtest_build_consensus_context_json \
    "$requested_height" \
    "$snapshot_id" \
    "$stable_block_hash" \
    "$local_state_commit" \
    "$system_state_id"
}

assert_payload_context_mismatch() {
  local payload_file="$1"
  local target_height="$2"
  local inscription_id="$3"
  local expected_code="$4"
  local expected_message="$5"
  local context_json state_ref_params snapshot_params energy_params resp

  context_json="$(build_payload_context_with_requested_height "$payload_file" "$target_height")"

  state_ref_params="$(python3 - "$target_height" "$context_json" <<'PY'
import json
import sys

print(json.dumps([{
    "block_height": int(sys.argv[1]),
    "context": json.loads(sys.argv[2]),
}]))
PY
)"

  snapshot_params="$(python3 - "$inscription_id" "$target_height" "$context_json" <<'PY'
import json
import sys

print(json.dumps([{
    "inscription_id": sys.argv[1],
    "at_height": int(sys.argv[2]),
    "context": json.loads(sys.argv[3]),
}]))
PY
)"

  energy_params="$(python3 - "$inscription_id" "$target_height" "$context_json" <<'PY'
import json
import sys

print(json.dumps([{
    "inscription_id": sys.argv[1],
    "block_height": int(sys.argv[2]),
    "mode": "at_or_before",
    "context": json.loads(sys.argv[3]),
}]))
PY
)"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "$snapshot_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "$energy_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"
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

  local miner_address ord_receive_address_a ord_receive_address_b
  local mint_content_file pass1 height_mint height_transfer
  local payload_mint payload_transfer continue_address

  miner_address="$(regtest_get_new_address)"
  regtest_log "Premining ${PREMINE_BLOCKS} blocks to address=${miner_address}"
  regtest_mine_blocks "$PREMINE_BLOCKS" "$miner_address"

  regtest_start_ord_server
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_prepare_ord_wallets

  ord_receive_address_a="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  ord_receive_address_b="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  regtest_fund_address "$ord_receive_address_a" "$FUND_ORD_AMOUNT_BTC"
  regtest_fund_address "$ord_receive_address_b" "$FUND_ORD_AMOUNT_BTC"
  regtest_mine_blocks "$FUND_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind

  mint_content_file="$WORK_DIR/usdb_validator_block_body_competing_mint.json"
  cat >"$mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF

  pass1="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_content_file" "$ord_receive_address_a")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_mint="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$height_mint"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$height_mint"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  payload_mint="$WORK_DIR/validator_block_body_payload_mint.json"
  write_validator_payload_for_pass_at_height "$payload_mint" "$pass1" "$height_mint"
  regtest_validate_validator_payload_success "$payload_mint"

  regtest_log "Advancing the same pass to a competing historical state via transfer at H+1"
  regtest_ord_send_inscription "$ORD_WALLET_NAME" "$ord_receive_address_b" "$pass1" >/dev/null
  regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$pass1"
  height_transfer="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_balance_history_synced_eq "$height_transfer"
  regtest_wait_until_usdb_synced_eq "$height_transfer"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  payload_transfer="$WORK_DIR/validator_block_body_payload_transfer.json"
  write_validator_payload_for_pass_at_height "$payload_transfer" "$pass1" "$height_transfer"
  regtest_validate_validator_payload_success "$payload_transfer"

  regtest_log "Competing payloads must describe different historical states for the same pass"
  regtest_assert_json_expr \
    "{\"result\": {\"left\": \"$(regtest_validator_payload_expr "$payload_mint" "data['external_state']['snapshot_id']")\", \"right\": \"$(regtest_validator_payload_expr "$payload_transfer" "data['external_state']['snapshot_id']")\"}}" \
    "(data.get('result') or {}).get('left') != (data.get('result') or {}).get('right')" \
    "True"
  regtest_assert_json_expr \
    "{\"result\": {\"left\": \"$(regtest_validator_payload_expr "$payload_mint" "data['external_state']['system_state_id']")\", \"right\": \"$(regtest_validator_payload_expr "$payload_transfer" "data['external_state']['system_state_id']")\"}}" \
    "(data.get('result') or {}).get('left') != (data.get('result') or {}).get('right')" \
    "True"
  regtest_assert_json_expr \
    "{\"result\": {\"left\": \"$(regtest_validator_payload_expr "$payload_mint" "data['miner_selection']['owner']")\", \"right\": \"$(regtest_validator_payload_expr "$payload_transfer" "data['miner_selection']['owner']")\"}}" \
    "(data.get('result') or {}).get('left') != (data.get('result') or {}).get('right')" \
    "True"
  regtest_assert_json_expr \
    "{\"result\": {\"left\": \"$(regtest_validator_payload_expr "$payload_mint" "data['miner_selection']['state']")\", \"right\": \"$(regtest_validator_payload_expr "$payload_transfer" "data['miner_selection']['state']")\"}}" \
    "(data.get('result') or {}).get('left') != (data.get('result') or {}).get('right')" \
    "True"

  regtest_log "Mint-height payload context cannot be reused to validate transfer-height state"
  assert_payload_context_mismatch "$payload_mint" "$height_transfer" "$pass1" "-32042" "SNAPSHOT_ID_MISMATCH"

  regtest_log "Transfer-height payload context cannot be reused to validate mint-height state"
  assert_payload_context_mismatch "$payload_transfer" "$height_mint" "$pass1" "-32042" "SNAPSHOT_ID_MISMATCH"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((height_transfer + 1))"
  regtest_wait_until_usdb_synced_eq "$((height_transfer + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Both competing payloads must remain individually valid after another head advance"
  regtest_validate_validator_payload_success "$payload_mint"
  regtest_validate_validator_payload_success "$payload_transfer"

  regtest_log "USDB validator block-body competing-payloads test succeeded."
  regtest_log "pass1=${pass1}, mint_height=${height_mint}, transfer_height=${height_transfer}"
}

main "$@"
