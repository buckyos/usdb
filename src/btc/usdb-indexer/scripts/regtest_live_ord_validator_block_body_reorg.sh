#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29732}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29733}"
BH_RPC_PORT="${BH_RPC_PORT:-29710}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29720}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29730}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorreorg}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-reorg-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-reorg-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-reorg]"

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
  local current_tip_height historical_height historical_hash
  local replacement_address payload_file state_ref_resp pass_snapshot_resp pass_energy_resp
  local target_post_reorg_height

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

  mint_content_file="$WORK_DIR/usdb_validator_block_body_reorg_mint.json"
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

  payload_file="$WORK_DIR/ethw_validator_block_body_reorg_payload.json"
  regtest_write_validator_payload_v1 "$payload_file" "$state_ref_resp" "$pass_snapshot_resp" "$pass_energy_resp"
  regtest_log "Wrote validator block-body reorg payload v1: ${payload_file}"

  regtest_log "Validator block-body payload must validate before reorg"
  regtest_validate_validator_payload_success "$payload_file"

  regtest_log "Triggering same-height reorg at validator payload height=${historical_height}"
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

  regtest_log "Old validator block-body payload must fail after same-height replacement"
  regtest_validate_validator_payload_consensus_error "$payload_file" "-32042" "SNAPSHOT_ID_MISMATCH"

  regtest_log "USDB validator block-body reorg test succeeded."
}

main "$@"
