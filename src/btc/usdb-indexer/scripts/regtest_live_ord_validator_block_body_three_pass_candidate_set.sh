#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-three-pass-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30052}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30053}"
BH_RPC_PORT="${BH_RPC_PORT:-30050}"
USDB_RPC_PORT="${USDB_RPC_PORT:-30060}"
ORD_RPC_PORT="${ORD_RPC_PORT:-30070}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorthreepass}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-three-pass-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-three-pass-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
AGE_BLOCKS_AFTER_PASS1="${AGE_BLOCKS_AFTER_PASS1:-2}"
AGE_BLOCKS_AFTER_PASS2="${AGE_BLOCKS_AFTER_PASS2:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-three-pass]"

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

  local miner_address ord_receive_address_a ord_receive_address_b
  local mint_receive_address winner_receive_address
  local mint1_file mint2_file mint3_file
  local pass1 pass2 pass3 height_competition height_after_transfer
  local candidate_entries_json winner_id payload_file continue_address
  local winner_wallet winner_destination
  local candidate_ids

  miner_address="$(regtest_get_new_address)"
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

  mint1_file="$WORK_DIR/usdb_validator_three_pass_mint_1.json"
  mint2_file="$WORK_DIR/usdb_validator_three_pass_mint_2.json"
  mint3_file="$WORK_DIR/usdb_validator_three_pass_mint_3.json"
  cat >"$mint1_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF
  cat >"$mint2_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x2222222222222222222222222222222222222222","prev":[]}
EOF
  cat >"$mint3_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x3333333333333333333333333333333333333333","prev":[]}
EOF

  regtest_log "Minting three candidate passes across different historical heights"
  mint_receive_address="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  pass1="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint1_file" "$mint_receive_address")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  if (( AGE_BLOCKS_AFTER_PASS1 > 0 )); then
    regtest_mine_blocks "$AGE_BLOCKS_AFTER_PASS1" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  fi

  mint_receive_address="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  pass2="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$mint2_file" "$mint_receive_address")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  if (( AGE_BLOCKS_AFTER_PASS2 > 0 )); then
    regtest_mine_blocks "$AGE_BLOCKS_AFTER_PASS2" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  fi

  mint_receive_address="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  pass3="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint3_file" "$mint_receive_address")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_competition="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$height_competition"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$height_competition"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  candidate_ids=("$pass1" "$pass2" "$pass3")
  candidate_entries_json="$(regtest_build_validator_candidate_entries_for_passes_at_height "$height_competition" "${candidate_ids[@]}")"
  winner_id="$(regtest_choose_validator_candidate_set_winner_id "$candidate_entries_json")"
  payload_file="$WORK_DIR/validator_block_body_three_pass_candidate_set_payload.json"
  regtest_write_validator_candidate_set_payload_for_passes_at_height "$payload_file" "$height_competition" "$winner_id" "${candidate_ids[@]}"

  regtest_validate_validator_candidate_set_payload_success "$payload_file"
  regtest_assert_json_expr "$(cat "$payload_file")" "((data.get('candidate_passes') or []).__len__())" "3"
  regtest_assert_json_expr "$(cat "$payload_file")" "all(item.get('state') == 'active' for item in (data.get('candidate_passes') or []))" "True"
  regtest_assert_json_expr "$(cat "$payload_file")" "data.get('miner_selection', {}).get('inscription_id')" "$winner_id"

  if [[ "$winner_id" == "$pass2" ]]; then
    winner_wallet="$ORD_WALLET_NAME_B"
    winner_destination="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  else
    winner_wallet="$ORD_WALLET_NAME"
    winner_destination="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  fi

  regtest_log "Advancing current winner state must not break historical 3-pass candidate-set payload"
  regtest_ord_send_inscription "$winner_wallet" "$winner_destination" "$winner_id" >/dev/null
  regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_after_transfer="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_balance_history_synced_eq "$height_after_transfer"
  regtest_wait_until_usdb_synced_eq "$height_after_transfer"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_assert_usdb_pass_snapshot_state "$winner_id" "$height_after_transfer" "dormant"
  regtest_validate_validator_candidate_set_payload_success "$payload_file"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((height_after_transfer + 1))"
  regtest_wait_until_usdb_synced_eq "$((height_after_transfer + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready
  regtest_validate_validator_candidate_set_payload_success "$payload_file"

  regtest_log "USDB validator block-body three-pass candidate-set test succeeded."
  regtest_log "passes=${pass1},${pass2},${pass3}, winner=${winner_id}, competition_height=${height_competition}, transfer_height=${height_after_transfer}"
}

main "$@"
