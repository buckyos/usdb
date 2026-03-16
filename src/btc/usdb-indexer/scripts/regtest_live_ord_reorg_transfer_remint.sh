#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-reorg-transfer-remint-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29532}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29533}"
BH_RPC_PORT="${BH_RPC_PORT:-29510}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29520}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29530}"
WALLET_NAME="${WALLET_NAME:-usdblivereorgminer}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-live-reorg-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-live-reorg-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
REMINT_CONFIRM_BLOCKS="${REMINT_CONFIRM_BLOCKS:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-live-ord-reorg-transfer-remint]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

assert_old_chain_state() {
  local block_height="$1"
  local pass_old="$2"
  local pass_new="$3"

  regtest_assert_usdb_pass_snapshot_state "$pass_old" "$block_height" "dormant"
  regtest_assert_usdb_pass_energy_state "$pass_old" "$block_height" "at_or_before" "dormant"
  regtest_assert_usdb_pass_snapshot_state "$pass_new" "$block_height" "active"
  regtest_assert_usdb_pass_energy_state "$pass_new" "$block_height" "at_or_before" "active"
  regtest_assert_usdb_active_balance_snapshot_positive "$block_height"
  regtest_assert_usdb_pass_stats "$block_height" "2" "1" "1" "0" "0" "0"
}

assert_transfer_only_state() {
  local block_height="$1"
  local pass_old="$2"
  local pass_new="$3"

  regtest_assert_usdb_pass_snapshot_state "$pass_old" "$block_height" "dormant"
  regtest_assert_usdb_pass_energy_state "$pass_old" "$block_height" "at_or_before" "dormant"
  regtest_assert_usdb_pass_snapshot_missing "$pass_new" "$block_height"
  regtest_assert_usdb_pass_energy_not_found "$pass_new" "$block_height" "at_or_before"
  regtest_assert_usdb_active_balance_snapshot_zero "$block_height"
  regtest_assert_usdb_pass_stats "$block_height" "1" "0" "1" "0" "0" "0"
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
  local pass_old pass_new height_mint height_transfer target_height
  local old_hash ancestor_hash replacement_hash continue_address replacement_address
  local old_bh_commit_resp old_bh_commit new_bh_commit_resp new_bh_commit
  local old_snapshot_resp old_snapshot_id new_snapshot_resp new_snapshot_id

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

  mint_content_file="$WORK_DIR/usdb_live_mint.json"
  cat >"$mint_content_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF
  remint_content_file="$WORK_DIR/usdb_live_remint.json"

  pass_old="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_content_file")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_mint="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_ord_send_inscription "$ORD_WALLET_NAME" "$ord_receive_address_b" "$pass_old" >/dev/null
  regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$pass_old"
  height_transfer="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  cat >"$remint_content_file" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x2222222222222222222222222222222222222222","prev":["${pass_old}"]}
EOF
  pass_new="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$remint_content_file")"
  regtest_mine_blocks "$REMINT_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  target_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  if [[ "$height_transfer" -ne $((target_height - 1)) ]]; then
    regtest_log "Unexpected live ord height layout: height_transfer=${height_transfer}, target_height=${target_height}"
    exit 1
  fi

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$target_height"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$target_height"

  old_hash="$(regtest_get_bitcoin_block_hash "$target_height")"
  ancestor_hash="$(regtest_get_bitcoin_block_hash "$height_transfer")"
  old_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${target_height}]")"
  old_bh_commit="$(regtest_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  old_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  old_snapshot_id="$(regtest_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"

  regtest_assert_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('btc_block_hash', ''))" "$old_hash"
  regtest_assert_json_expr "$old_snapshot_resp" "(data.get('result') or {}).get('local_synced_block_height')" "$target_height"
  regtest_assert_json_expr "$old_snapshot_resp" "(data.get('result') or {}).get('stable_block_hash')" "$old_hash"
  regtest_assert_json_expr "$old_snapshot_resp" "(data.get('result') or {}).get('latest_block_commit')" "$old_bh_commit"
  assert_old_chain_state "$target_height" "$pass_old" "$pass_new"

  regtest_log "Triggering live ord height-regression reorg at remint height=${target_height}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$old_hash"

  regtest_wait_until_balance_history_synced_eq "$height_transfer"
  regtest_wait_until_rpc_expr_eq \
    "balance-history snapshot stable hash after rollback" \
    regtest_rpc_call_balance_history \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$ancestor_hash"
  regtest_wait_until_usdb_synced_eq "$height_transfer"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash after rollback" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$ancestor_hash"

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM pass_block_commits WHERE block_height > ${height_transfer};" \
    "0" \
    "future pass_block_commits cleared after live ord rollback"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height > ${height_transfer};" \
    "0" \
    "future active_balance_snapshots cleared after live ord rollback"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM state WHERE name = 'upstream_reorg_recovery_pending_height';" \
    "0" \
    "pending recovery marker cleared after live ord rollback"
  assert_transfer_only_state "$height_transfer" "$pass_old" "$pass_new"

  replacement_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$replacement_address"
  replacement_hash="$(regtest_get_bitcoin_block_hash "$target_height")"
  if [[ "$replacement_hash" == "$old_hash" ]]; then
    regtest_log "Replacement hash unexpectedly matches original remint block hash"
    exit 1
  fi

  regtest_wait_until_balance_history_synced_eq "$target_height"
  regtest_wait_until_balance_history_block_commit_hash "$target_height" "$replacement_hash"
  new_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${target_height}]")"
  new_bh_commit="$(regtest_json_expr "$new_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  regtest_wait_until_usdb_synced_eq "$target_height"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash after replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$replacement_hash"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot latest block commit after replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('latest_block_commit', ''))" \
    "$new_bh_commit"
  regtest_wait_until_rpc_expr_eq \
    "usdb pass block commit anchor after replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_pass_block_commit" \
    "[{\"block_height\":${target_height}}]" \
    "((data.get('result') or {}).get('balance_history_block_commit', ''))" \
    "$new_bh_commit"

  new_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  new_snapshot_id="$(regtest_json_expr "$new_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
  if [[ "$new_snapshot_id" == "$old_snapshot_id" ]]; then
    regtest_log "Snapshot id unexpectedly unchanged after replacement: snapshot_id=${new_snapshot_id}"
    exit 1
  fi

  assert_transfer_only_state "$target_height" "$pass_old" "$pass_new"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((target_height + 1))"
  regtest_wait_until_usdb_synced_eq "$((target_height + 1))"

  regtest_log "USDB indexer live ord height-regression reorg transfer/remint test succeeded."
  regtest_log "Mint height=${height_mint}, transfer height=${height_transfer}, reorged remint height=${target_height}"
  regtest_log "Logs: ${ORD_SERVER_LOG_FILE}, ${BALANCE_HISTORY_LOG_FILE}, ${USDB_INDEXER_LOG_FILE}"
}

main "$@"
