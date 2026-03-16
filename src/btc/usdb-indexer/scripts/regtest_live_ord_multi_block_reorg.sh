#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-multi-block-reorg-XXXXXX)}"
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
WALLET_NAME="${WALLET_NAME:-usdblivemultireorgminer}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-live-multi-reorg-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-live-multi-reorg-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
REMINT_CONFIRM_BLOCKS="${REMINT_CONFIRM_BLOCKS:-1}"
PENALTY_FUND_CONFIRM_BLOCKS="${PENALTY_FUND_CONFIRM_BLOCKS:-1}"
PENALTY_SPEND_CONFIRM_BLOCKS="${PENALTY_SPEND_CONFIRM_BLOCKS:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-live-ord-multi-block-reorg]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

assert_leaderboard_top1_matches_pass_energy() {
  local block_height="$1"
  local scope="$2"
  local expected_total="$3"
  local inscription_id="$4"
  local expected_state="$5"
  local leaderboard_resp pass_energy_resp expected_energy

  leaderboard_resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy_leaderboard" "[{\"at_height\":${block_height},\"scope\":\"${scope}\",\"page\":0,\"page_size\":5}]")"
  regtest_assert_json_expr "$leaderboard_resp" "data.get('error') is None" "True"
  regtest_assert_json_expr "$leaderboard_resp" "(data.get('result') or {}).get('resolved_height')" "$block_height"
  regtest_assert_json_expr "$leaderboard_resp" "(data.get('result') or {}).get('total')" "$expected_total"
  regtest_assert_json_expr "$leaderboard_resp" "len((data.get('result') or {}).get('items') or [])" "$expected_total"
  regtest_assert_json_expr "$leaderboard_resp" "(((data.get('result') or {}).get('items') or [{}])[0].get('inscription_id'))" "$inscription_id"
  regtest_assert_json_expr "$leaderboard_resp" "(((data.get('result') or {}).get('items') or [{}])[0].get('state'))" "$expected_state"

  pass_energy_resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${inscription_id}\",\"block_height\":${block_height},\"mode\":\"at_or_before\"}]")"
  regtest_assert_json_expr "$pass_energy_resp" "data.get('error') is None" "True"
  expected_energy="$(regtest_json_expr "$pass_energy_resp" "((data.get('result') or {}).get('energy', ''))")"
  regtest_assert_json_expr "$leaderboard_resp" "(((data.get('result') or {}).get('items') or [{}])[0].get('energy'))" "$expected_energy"
}

assert_current_db_state_old_chain() {
  local pass_consumed="$1"
  local pass_dormant="$2"
  local pass_active="$3"

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM miner_passes;" \
    "3" \
    "current miner_passes row count on old chain"
  regtest_assert_usdb_db_scalar \
    "SELECT state FROM miner_passes WHERE inscription_id = '${pass_consumed}';" \
    "consumed" \
    "current state of original pass on old chain"
  regtest_assert_usdb_db_scalar \
    "SELECT state FROM miner_passes WHERE inscription_id = '${pass_dormant}';" \
    "dormant" \
    "current state of first remint pass on old chain"
  regtest_assert_usdb_db_scalar \
    "SELECT state FROM miner_passes WHERE inscription_id = '${pass_active}';" \
    "active" \
    "current state of duplicate remint pass on old chain"
}

assert_current_db_state_replacement() {
  local pass_consumed="$1"
  local pass_active="$2"
  local pass_removed="$3"

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM miner_passes;" \
    "2" \
    "current miner_passes row count on replacement chain"
  regtest_assert_usdb_db_scalar \
    "SELECT state FROM miner_passes WHERE inscription_id = '${pass_consumed}';" \
    "consumed" \
    "current state of original pass on replacement chain"
  regtest_assert_usdb_db_scalar \
    "SELECT state FROM miner_passes WHERE inscription_id = '${pass_active}';" \
    "active" \
    "current state of surviving remint pass on replacement chain"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM miner_passes WHERE inscription_id = '${pass_removed}';" \
    "0" \
    "duplicate remint pass removed from current table on replacement chain"
}

assert_old_chain_state() {
  local block_height="$1"
  local pass1="$2"
  local pass2="$3"
  local pass3="$4"

  regtest_assert_usdb_pass_snapshot_state "$pass1" "$block_height" "consumed"
  regtest_assert_usdb_pass_energy_state "$pass1" "$block_height" "at_or_before" "consumed"
  regtest_assert_usdb_pass_snapshot_state "$pass2" "$block_height" "dormant"
  regtest_assert_usdb_pass_energy_state "$pass2" "$block_height" "at_or_before" "dormant"
  regtest_assert_usdb_pass_snapshot_state "$pass3" "$block_height" "active"
  regtest_assert_usdb_pass_energy_state "$pass3" "$block_height" "at_or_before" "active"
  regtest_assert_usdb_active_balance_snapshot_positive "$block_height"
  regtest_assert_usdb_pass_stats "$block_height" "3" "1" "1" "1" "0" "0"
  assert_leaderboard_top1_matches_pass_energy "$block_height" "active" "1" "$pass3" "active"
}

assert_replacement_chain_state() {
  local block_height="$1"
  local pass1="$2"
  local pass2="$3"
  local pass3="$4"

  regtest_assert_usdb_pass_snapshot_state "$pass1" "$block_height" "consumed"
  regtest_assert_usdb_pass_energy_state "$pass1" "$block_height" "at_or_before" "consumed"
  regtest_assert_usdb_pass_snapshot_state "$pass2" "$block_height" "active"
  regtest_assert_usdb_pass_energy_state "$pass2" "$block_height" "at_or_before" "active"
  regtest_assert_usdb_pass_snapshot_missing "$pass3" "$block_height"
  regtest_assert_usdb_pass_energy_not_found "$pass3" "$block_height" "at_or_before"
  regtest_assert_usdb_active_balance_snapshot_positive "$block_height"
  regtest_assert_usdb_pass_stats "$block_height" "2" "1" "0" "1" "0" "0"
  assert_leaderboard_top1_matches_pass_energy "$block_height" "active" "1" "$pass2" "active"
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
  local mint_content_file remint_content_file_1 remint_content_file_2
  local pass1 pass2 pass3
  local height_transfer height_remint_1 height_penalty_baseline height_penalty target_height
  local rollback_height rollback_block_hash rollback_ancestor_hash
  local old_hash replacement_hash continue_address replacement_address
  local old_bh_commit_resp old_bh_commit new_bh_commit_resp new_bh_commit
  local old_snapshot_resp old_snapshot_id new_snapshot_resp new_snapshot_id
  local old_pass_commit_resp old_pass_anchor new_pass_commit_resp new_pass_anchor
  local penalty_fund_txid penalty_fund_vout penalty_spend_raw penalty_spend_signed penalty_spend_hex
  local penalty_spend_complete penalty_spend_txid

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
  remint_content_file_1="$WORK_DIR/usdb_live_remint_first.json"
  remint_content_file_2="$WORK_DIR/usdb_live_remint_second.json"

  pass1="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_content_file")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind

  regtest_ord_send_inscription "$ORD_WALLET_NAME" "$ord_receive_address_b" "$pass1" >/dev/null
  regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$pass1"
  height_transfer="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  cat >"$remint_content_file_1" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x2222222222222222222222222222222222222222","prev":["${pass1}"]}
EOF
  pass2="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$remint_content_file_1" "$ord_receive_address_b")"
  regtest_mine_blocks "$REMINT_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_remint_1="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_log "Funding owner address before penalty spend: address=${ord_receive_address_b}, amount_btc=${PENALTY_FUND_AMOUNT_BTC}"
  penalty_fund_txid="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$WALLET_NAME" \
    sendtoaddress "$ord_receive_address_b" "$PENALTY_FUND_AMOUNT_BTC")"
  if [[ -z "$penalty_fund_txid" ]]; then
    regtest_log "Failed to create penalty baseline funding transaction"
    exit 1
  fi
  regtest_mine_blocks "$PENALTY_FUND_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_penalty_baseline="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  penalty_fund_vout="$(regtest_get_tx_vout_for_address "$penalty_fund_txid" "$ord_receive_address_b")"
  if [[ -z "$penalty_fund_vout" ]]; then
    regtest_log "Failed to locate owner output in penalty funding tx: txid=${penalty_fund_txid}, owner_address=${ord_receive_address_b}"
    exit 1
  fi

  regtest_log "Spending funded owner UTXO to trigger negative owner delta: txid=${penalty_fund_txid}, vout=${penalty_fund_vout}, amount_btc=${PENALTY_SPEND_AMOUNT_BTC}"
  penalty_spend_raw="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$ORD_WALLET_NAME_B" \
    createrawtransaction "[{\"txid\":\"${penalty_fund_txid}\",\"vout\":${penalty_fund_vout}}]" "{\"${miner_address}\":${PENALTY_SPEND_AMOUNT_BTC}}")"
  if [[ -z "$penalty_spend_raw" ]]; then
    regtest_log "Failed to create penalty spend raw transaction"
    exit 1
  fi

  penalty_spend_signed="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet="$ORD_WALLET_NAME_B" \
    signrawtransactionwithwallet "$penalty_spend_raw")"
  penalty_spend_hex="$(printf '%s' "$penalty_spend_signed" | python3 -c 'import json,sys; print(json.load(sys.stdin).get("hex", ""))')"
  penalty_spend_complete="$(printf '%s' "$penalty_spend_signed" | python3 -c 'import json,sys; print("true" if json.load(sys.stdin).get("complete") else "false")')"
  if [[ "$penalty_spend_complete" != "true" || -z "$penalty_spend_hex" ]]; then
    regtest_log "Failed to sign penalty spend transaction: payload=${penalty_spend_signed}"
    exit 1
  fi

  penalty_spend_txid="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" sendrawtransaction "$penalty_spend_hex")"
  if [[ -z "$penalty_spend_txid" ]]; then
    regtest_log "Failed to broadcast penalty spend transaction"
    exit 1
  fi
  regtest_mine_blocks "$PENALTY_SPEND_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_penalty="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  cat >"$remint_content_file_2" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x4444444444444444444444444444444444444444","prev":["${pass1}"]}
EOF
  pass3="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$remint_content_file_2" "$ord_receive_address_b")"
  regtest_mine_blocks "$REMINT_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  target_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  rollback_height="$height_remint_1"
  if [[ "$height_transfer" -ne $((height_remint_1 - 1)) || "$height_penalty_baseline" -ne $((height_remint_1 + 1)) || "$height_penalty" -ne $((height_remint_1 + 2)) || "$target_height" -ne $((height_remint_1 + 3)) ]]; then
    regtest_log "Unexpected live ord multi-block height layout: transfer=${height_transfer}, remint_1=${height_remint_1}, penalty_baseline=${height_penalty_baseline}, penalty=${height_penalty}, target=${target_height}"
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
  rollback_block_hash="$(regtest_get_bitcoin_block_hash "$height_penalty_baseline")"
  rollback_ancestor_hash="$(regtest_get_bitcoin_block_hash "$rollback_height")"
  old_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${target_height}]")"
  old_bh_commit="$(regtest_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  old_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  old_snapshot_id="$(regtest_json_expr "$old_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
  old_pass_commit_resp="$(regtest_rpc_call_usdb_indexer "get_pass_block_commit" "[{\"block_height\":${target_height}}]")"
  old_pass_anchor="$(regtest_json_expr "$old_pass_commit_resp" "((data.get('result') or {}).get('balance_history_block_commit', ''))")"

  regtest_assert_json_expr "$old_bh_commit_resp" "((data.get('result') or {}).get('btc_block_hash', ''))" "$old_hash"
  regtest_assert_json_expr "$old_snapshot_resp" "(data.get('result') or {}).get('local_synced_block_height')" "$target_height"
  regtest_assert_json_expr "$old_snapshot_resp" "(data.get('result') or {}).get('stable_block_hash')" "$old_hash"
  regtest_assert_json_expr "$old_snapshot_resp" "(data.get('result') or {}).get('latest_block_commit')" "$old_bh_commit"
  assert_old_chain_state "$target_height" "$pass1" "$pass2" "$pass3"
  assert_current_db_state_old_chain "$pass1" "$pass2" "$pass3"

  regtest_log "Triggering live ord multi-block reorg from first penalty block at height=${height_penalty_baseline}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$rollback_block_hash"

  regtest_wait_until_balance_history_synced_eq "$rollback_height"
  regtest_wait_until_rpc_expr_eq \
    "balance-history snapshot stable hash after multi-block rollback" \
    regtest_rpc_call_balance_history \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$rollback_ancestor_hash"
  regtest_wait_until_usdb_synced_eq "$rollback_height"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash after multi-block rollback" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$rollback_ancestor_hash"

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM pass_block_commits WHERE block_height > ${rollback_height};" \
    "0" \
    "future pass_block_commits cleared after multi-block rollback"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height > ${rollback_height};" \
    "0" \
    "future active_balance_snapshots cleared after multi-block rollback"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM state WHERE name = 'upstream_reorg_recovery_pending_height';" \
    "0" \
    "pending recovery marker cleared after multi-block rollback"
  assert_replacement_chain_state "$rollback_height" "$pass1" "$pass2" "$pass3"
  assert_current_db_state_replacement "$pass1" "$pass2" "$pass3"

  replacement_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$replacement_address"
  replacement_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$replacement_address"
  replacement_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$replacement_address"
  replacement_hash="$(regtest_get_bitcoin_block_hash "$target_height")"
  if [[ "$replacement_hash" == "$old_hash" ]]; then
    regtest_log "Replacement hash unexpectedly matches original multi-block tip hash"
    exit 1
  fi

  regtest_wait_until_balance_history_synced_eq "$target_height"
  regtest_wait_until_balance_history_block_commit_hash "$target_height" "$replacement_hash"
  new_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${target_height}]")"
  new_bh_commit="$(regtest_json_expr "$new_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  regtest_wait_until_usdb_synced_eq "$target_height"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash after multi-block replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$replacement_hash"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot latest block commit after multi-block replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('latest_block_commit', ''))" \
    "$new_bh_commit"
  regtest_wait_until_rpc_expr_eq \
    "usdb pass block commit anchor after multi-block replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_pass_block_commit" \
    "[{\"block_height\":${target_height}}]" \
    "((data.get('result') or {}).get('balance_history_block_commit', ''))" \
    "$new_bh_commit"

  new_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_snapshot_info" "[]")"
  new_snapshot_id="$(regtest_json_expr "$new_snapshot_resp" "((data.get('result') or {}).get('snapshot_id', ''))")"
  new_pass_commit_resp="$(regtest_rpc_call_usdb_indexer "get_pass_block_commit" "[{\"block_height\":${target_height}}]")"
  new_pass_anchor="$(regtest_json_expr "$new_pass_commit_resp" "((data.get('result') or {}).get('balance_history_block_commit', ''))")"
  if [[ "$new_snapshot_id" == "$old_snapshot_id" ]]; then
    regtest_log "Snapshot id unexpectedly unchanged after multi-block replacement: snapshot_id=${new_snapshot_id}"
    exit 1
  fi
  if [[ "$new_pass_anchor" == "$old_pass_anchor" ]]; then
    regtest_log "Pass block commit anchor unexpectedly unchanged after multi-block replacement: balance_history_block_commit=${new_pass_anchor}"
    exit 1
  fi

  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM pass_block_commits WHERE block_height = ${target_height};" \
    "1" \
    "exactly one pass_block_commit row at multi-block replacement tip"
  regtest_assert_usdb_db_scalar \
    "SELECT COUNT(*) FROM active_balance_snapshots WHERE block_height = ${target_height};" \
    "1" \
    "exactly one active_balance_snapshot row at multi-block replacement tip"
  assert_replacement_chain_state "$target_height" "$pass1" "$pass2" "$pass3"
  assert_current_db_state_replacement "$pass1" "$pass2" "$pass3"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((target_height + 1))"
  regtest_wait_until_usdb_synced_eq "$((target_height + 1))"

  regtest_log "USDB indexer live ord multi-block reorg test succeeded."
  regtest_log "Rollback ancestor height=${rollback_height}, replacement tip height=${target_height}"
  regtest_log "Logs: ${ORD_SERVER_LOG_FILE}, ${BALANCE_HISTORY_LOG_FILE}, ${USDB_INDEXER_LOG_FILE}"
}

main "$@"
