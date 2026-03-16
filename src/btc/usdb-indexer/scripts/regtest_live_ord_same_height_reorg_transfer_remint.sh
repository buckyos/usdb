#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-same-height-reorg-transfer-remint-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29632}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29633}"
BH_RPC_PORT="${BH_RPC_PORT:-29610}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29620}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29630}"
WALLET_NAME="${WALLET_NAME:-usdblivesameheightminer}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-live-same-height-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-live-same-height-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
REMINT_CONFIRM_BLOCKS="${REMINT_CONFIRM_BLOCKS:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-live-ord-same-height-reorg-transfer-remint]"

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
  local pass_old pass_new height_transfer target_height
  local old_hash replacement_hash replacement_address continue_address
  local old_bh_commit_resp old_bh_commit new_bh_commit_resp new_bh_commit
  local old_snapshot_resp old_snapshot_id old_snapshot_commit old_pass_commit_resp old_pass_anchor
  local new_snapshot_resp new_snapshot_id new_snapshot_commit new_pass_commit_resp new_pass_anchor

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
  old_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${target_height}]")"
  old_bh_commit="$(regtest_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  old_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  old_snapshot_id="$(regtest_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
  old_snapshot_commit="$(regtest_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('latest_block_commit', ''))")"
  old_pass_commit_resp="$(regtest_rpc_call_usdb_indexer "get_pass_block_commit" "[{\"block_height\":${target_height}}]")"
  old_pass_anchor="$(regtest_json_expr "$old_pass_commit_resp" "((data.get('result') or {}).get('balance_history_block_commit', ''))")"

  regtest_assert_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('btc_block_hash', ''))" "$old_hash"
  regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('stable_block_hash', ''))" "$old_hash"
  regtest_assert_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('latest_block_commit', ''))" "$old_bh_commit"
  assert_old_chain_state "$target_height" "$pass_old" "$pass_new"

  regtest_log "Triggering live ord same-height reorg at remint height=${target_height}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$old_hash"
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
    "usdb snapshot stable hash after same-height replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$replacement_hash"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot latest block commit after same-height replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('latest_block_commit', ''))" \
    "$new_bh_commit"
  regtest_wait_until_rpc_expr_eq \
    "usdb pass block commit anchor after same-height replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_pass_block_commit" \
    "[{\"block_height\":${target_height}}]" \
    "((data.get('result') or {}).get('balance_history_block_commit', ''))" \
    "$new_bh_commit"

  new_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  new_snapshot_id="$(regtest_json_expr "$new_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
  new_snapshot_commit="$(regtest_json_expr "$new_snapshot_resp" "((data.get('result') or {}).get('latest_block_commit', ''))")"
  new_pass_commit_resp="$(regtest_rpc_call_usdb_indexer "get_pass_block_commit" "[{\"block_height\":${target_height}}]")"
  new_pass_anchor="$(regtest_json_expr "$new_pass_commit_resp" "((data.get('result') or {}).get('balance_history_block_commit', ''))")"

  if [[ "$new_snapshot_id" == "$old_snapshot_id" ]]; then
    regtest_log "Snapshot id did not change after same-height replacement: snapshot_id=${new_snapshot_id}"
    exit 1
  fi
  if [[ "$new_snapshot_commit" == "$old_snapshot_commit" ]]; then
    regtest_log "Snapshot latest_block_commit did not change after same-height replacement: latest_block_commit=${new_snapshot_commit}"
    exit 1
  fi
  if [[ "$new_pass_anchor" == "$old_pass_anchor" ]]; then
    regtest_log "Pass block commit anchor did not change after same-height replacement: balance_history_block_commit=${new_pass_anchor}"
    exit 1
  fi

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM pass_block_commits WHERE block_height = ${target_height};" \
    "1" \
    "exactly one pass_block_commit row at same-height replacement block"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height = ${target_height};" \
    "1" \
    "exactly one active_balance_snapshot row at same-height replacement block"
  assert_transfer_only_state "$target_height" "$pass_old" "$pass_new"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((target_height + 1))"
  regtest_wait_until_usdb_synced_eq "$((target_height + 1))"

  regtest_log "USDB indexer live ord same-height reorg transfer/remint test succeeded."
  regtest_log "Transfer height=${height_transfer}, replaced remint height=${target_height}"
  regtest_log "Logs: ${ORD_SERVER_LOG_FILE}, ${BALANCE_HISTORY_LOG_FILE}, ${USDB_INDEXER_LOG_FILE}"
}

main "$@"
