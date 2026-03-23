#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-version-matrix-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30732}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30733}"
BH_RPC_PORT="${BH_RPC_PORT:-30710}"
USDB_RPC_PORT="${USDB_RPC_PORT:-30720}"
ORD_RPC_PORT="${ORD_RPC_PORT:-30730}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorversionmatrix}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-version-matrix-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-version-matrix-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-version-matrix]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

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
  local current_tip_height historical_height continue_height continue_address
  local state_ref_resp pass_snapshot_resp pass_energy_resp payload_file
  local protocol_payload semantics_payload api_payload

  miner_address="$(regtest_get_new_address)"
  regtest_mine_blocks "$PREMINE_BLOCKS" "$miner_address"

  regtest_start_ord_server
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_prepare_ord_wallets

  ord_receive_address="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  regtest_fund_address "$ord_receive_address" "$FUND_ORD_AMOUNT_BTC"
  regtest_mine_blocks "$FUND_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind

  mint_content_file="$WORK_DIR/usdb_validator_block_body_version_matrix_mint.json"
  cat >"$mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x4444444444444444444444444444444444444444","prev":[]}
EOF

  pass_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_content_file")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  current_tip_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  historical_height="$((current_tip_height - 1))"

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

  payload_file="$WORK_DIR/validator_block_body_version_matrix_valid_payload.json"
  regtest_write_validator_payload_v1 "$payload_file" "$state_ref_resp" "$pass_snapshot_resp" "$pass_energy_resp"
  regtest_validate_validator_payload_success "$payload_file"

  protocol_payload="$WORK_DIR/validator_block_body_version_matrix_protocol_payload.json"
  semantics_payload="$WORK_DIR/validator_block_body_version_matrix_semantics_payload.json"
  api_payload="$WORK_DIR/validator_block_body_version_matrix_api_payload.json"
  regtest_write_validator_payload_tampered_external_state_field "$payload_file" "$protocol_payload" "usdb_index_protocol_version" "9.9.9-phase-c"
  regtest_write_validator_payload_tampered_external_state_field "$payload_file" "$semantics_payload" "balance_history_semantics_version" "balance-snapshot-at-or-before:v999"
  regtest_write_validator_payload_tampered_external_state_field "$payload_file" "$api_payload" "balance_history_api_version" "9.9.9-phase-c"

  regtest_validate_validator_payload_consensus_error "$protocol_payload" "-32044" "VERSION_MISMATCH"
  regtest_validate_validator_payload_consensus_error "$semantics_payload" "-32044" "VERSION_MISMATCH"
  regtest_validate_validator_payload_consensus_error "$api_payload" "-32044" "VERSION_MISMATCH"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  continue_height="$((historical_height + 1))"
  regtest_wait_until_balance_history_synced_eq "$continue_height"
  regtest_wait_until_usdb_synced_eq "$continue_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_validate_validator_payload_success "$payload_file"
  regtest_validate_validator_payload_consensus_error "$protocol_payload" "-32044" "VERSION_MISMATCH"
  regtest_validate_validator_payload_consensus_error "$semantics_payload" "-32044" "VERSION_MISMATCH"
  regtest_validate_validator_payload_consensus_error "$api_payload" "-32044" "VERSION_MISMATCH"

  regtest_log "USDB validator block-body version-matrix test succeeded."
}

main "$@"
