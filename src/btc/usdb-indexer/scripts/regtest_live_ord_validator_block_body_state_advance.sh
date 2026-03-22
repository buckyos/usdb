#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-state-advance-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29842}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29843}"
BH_RPC_PORT="${BH_RPC_PORT:-29840}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29850}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29860}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatoradvance}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-advance-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-advance-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
REMINT_CONFIRM_BLOCKS="${REMINT_CONFIRM_BLOCKS:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-state-advance]"

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

assert_current_pass_owner_matches_payload_relation() {
  local payload_file="$1"
  local inscription_id="$2"
  local current_height="$3"
  local relation="$4"
  local resp payload_owner current_owner

  payload_owner="$(regtest_validator_payload_expr "$payload_file" "data['miner_selection']['owner']")"
  resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "[{\"inscription_id\":\"${inscription_id}\",\"at_height\":${current_height}}]")"
  regtest_assert_json_expr "$resp" "data.get('error') is None" "True"
  current_owner="$(regtest_json_expr "$resp" "(data.get('result') or {}).get('owner')")"
  regtest_log "Comparing current owner with historical payload: payload_owner=${payload_owner}, current_owner=${current_owner}, relation=${relation}, height=${current_height}, inscription_id=${inscription_id}"

  case "$relation" in
    equal)
      if [[ "$current_owner" != "$payload_owner" ]]; then
        regtest_log "Expected current owner to equal payload owner"
        exit 1
      fi
      ;;
    different)
      if [[ "$current_owner" == "$payload_owner" ]]; then
        regtest_log "Expected current owner to differ from payload owner"
        exit 1
      fi
      ;;
    *)
      regtest_log "Unsupported owner relation assertion: ${relation}"
      exit 1
      ;;
  esac
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
  local mint_content_file remint_content_file
  local pass1 pass2 height_mint height_transfer height_remint
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

  mint_content_file="$WORK_DIR/usdb_validator_block_body_state_advance_mint.json"
  cat >"$mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF
  remint_content_file="$WORK_DIR/usdb_validator_block_body_state_advance_remint.json"

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
  regtest_log "Wrote validator payload for mint height=${height_mint}: ${payload_mint}"
  regtest_validate_validator_payload_success "$payload_mint"

  regtest_log "Triggering a real pass state change via transfer at H+1"
  regtest_ord_send_inscription "$ORD_WALLET_NAME" "$ord_receive_address_b" "$pass1" >/dev/null
  regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$pass1"
  height_transfer="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_balance_history_synced_eq "$height_transfer"
  regtest_wait_until_usdb_synced_eq "$height_transfer"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Historical H payload must remain valid after transfer changes owner/state"
  regtest_validate_validator_payload_success "$payload_mint"
  regtest_assert_usdb_pass_snapshot_state "$pass1" "$height_transfer" "dormant"
  regtest_assert_usdb_pass_energy_state "$pass1" "$height_transfer" "at_or_before" "dormant"
  assert_current_pass_owner_matches_payload_relation "$payload_mint" "$pass1" "$height_transfer" "different"

  payload_transfer="$WORK_DIR/validator_block_body_payload_transfer.json"
  write_validator_payload_for_pass_at_height "$payload_transfer" "$pass1" "$height_transfer"
  regtest_log "Wrote validator payload for transfer height=${height_transfer}: ${payload_transfer}"
  regtest_validate_validator_payload_success "$payload_transfer"

  regtest_log "Triggering another real pass graph change via remint(prev) at H+2"
  cat >"$remint_content_file" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x2222222222222222222222222222222222222222","prev":["${pass1}"]}
EOF
  pass2="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$remint_content_file" "$ord_receive_address_b")"
  regtest_mine_blocks "$REMINT_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_remint="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_balance_history_synced_eq "$height_remint"
  regtest_wait_until_usdb_synced_eq "$height_remint"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Both historical payloads must remain valid after later remint(prev)"
  regtest_validate_validator_payload_success "$payload_mint"
  regtest_validate_validator_payload_success "$payload_transfer"

  regtest_assert_usdb_pass_snapshot_state "$pass1" "$height_remint" "consumed"
  regtest_assert_usdb_pass_energy_state "$pass1" "$height_remint" "at_or_before" "consumed"
  assert_current_pass_owner_matches_payload_relation "$payload_mint" "$pass1" "$height_remint" "different"
  assert_current_pass_owner_matches_payload_relation "$payload_transfer" "$pass1" "$height_remint" "equal"

  regtest_assert_usdb_pass_snapshot_state "$pass2" "$height_remint" "active"
  regtest_assert_usdb_pass_energy_state "$pass2" "$height_remint" "at_or_before" "active"
  regtest_assert_usdb_active_balance_snapshot_positive "$height_remint"
  regtest_assert_usdb_pass_stats "$height_remint" "2" "1" "0" "1" "0" "0"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((height_remint + 1))"
  regtest_wait_until_usdb_synced_eq "$((height_remint + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Historical payloads must still validate after one more empty-head advance"
  regtest_validate_validator_payload_success "$payload_mint"
  regtest_validate_validator_payload_success "$payload_transfer"

  regtest_log "USDB validator block-body state-advance test succeeded."
  regtest_log "pass1=${pass1}, pass2=${pass2}, mint_height=${height_mint}, transfer_height=${height_transfer}, remint_height=${height_remint}"
}

main "$@"
