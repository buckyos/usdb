#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-payload-version-upgrade-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30872}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30873}"
BH_RPC_PORT="${BH_RPC_PORT:-30810}"
USDB_RPC_PORT="${USDB_RPC_PORT:-30820}"
ORD_RPC_PORT="${ORD_RPC_PORT:-30830}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorpayloadupgradereorg}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-payload-upgrade-reorg-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-payload-upgrade-reorg-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-payload-version-upgrade-reorg]"

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
  local mint_a_file mint_b_file pass1 pass2
  local height_v1 height_v11 replacement_hash original_v11_hash
  local new_bh_commit_resp new_bh_commit
  local state_ref_resp pass_snapshot_resp pass_energy_resp
  local payload_v1 payload_v11 candidate_entries_json winner_id replacement_address

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

  mint_a_file="$WORK_DIR/usdb_validator_payload_version_upgrade_reorg_mint_a.json"
  mint_b_file="$WORK_DIR/usdb_validator_payload_version_upgrade_reorg_mint_b.json"
  cat >"$mint_a_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","prev":[]}
EOF
  cat >"$mint_b_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","prev":[]}
EOF

  pass1="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_a_file" "$ord_receive_address_a")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_v1="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$height_v1"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$height_v1"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready
  regtest_wait_usdb_state_ref_available "$height_v1"

  state_ref_resp="$(regtest_get_usdb_state_ref_response "$height_v1")"
  regtest_assert_json_expr "$state_ref_resp" "data.get('error') is None" "True"
  pass_snapshot_resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "[{\"inscription_id\":\"${pass1}\",\"at_height\":${height_v1}}]")"
  regtest_assert_json_expr "$pass_snapshot_resp" "data.get('error') is None" "True"
  pass_energy_resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${pass1}\",\"block_height\":${height_v1},\"mode\":\"at_or_before\"}]")"
  regtest_assert_json_expr "$pass_energy_resp" "data.get('error') is None" "True"

  payload_v1="$WORK_DIR/validator_block_body_payload_v1.json"
  regtest_write_validator_payload_v1 "$payload_v1" "$state_ref_resp" "$pass_snapshot_resp" "$pass_energy_resp"

  pass2="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$mint_b_file" "$ord_receive_address_b")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  height_v11="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  original_v11_hash="$(regtest_get_bitcoin_block_hash "$height_v11")"
  regtest_wait_until_balance_history_synced_eq "$height_v11"
  regtest_wait_until_usdb_synced_eq "$height_v11"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  candidate_entries_json="$(regtest_build_validator_candidate_entries_for_passes_at_height "$height_v11" "$pass1" "$pass2")"
  winner_id="$(regtest_choose_validator_candidate_set_winner_id "$candidate_entries_json")"
  payload_v11="$WORK_DIR/validator_block_body_payload_v11.json"
  regtest_write_validator_candidate_set_payload_for_passes_at_height "$payload_v11" "$height_v11" "$winner_id" "$pass1" "$pass2"

  regtest_validate_validator_payload_versioned_success "$payload_v1"
  regtest_validate_validator_payload_versioned_success "$payload_v11"

  regtest_log "Triggering same-height replacement at the v1.1 payload height=${height_v11}"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_v11_hash"
  replacement_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$replacement_address"
  replacement_hash="$(regtest_get_bitcoin_block_hash "$height_v11")"
  if [[ "$replacement_hash" == "$original_v11_hash" ]]; then
    regtest_log "Replacement hash unexpectedly matches original v1.1 block hash"
    exit 1
  fi

  regtest_wait_until_balance_history_synced_eq "$height_v11"
  regtest_wait_until_balance_history_block_commit_hash "$height_v11" "$replacement_hash"
  new_bh_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${height_v11}]")"
  new_bh_commit="$(regtest_json_expr "$new_bh_commit_resp" "((data.get('result') or {}).get('block_commit', ''))")"
  regtest_wait_until_usdb_synced_eq "$height_v11"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot stable hash after payload-version same-height replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('stable_block_hash', ''))" \
    "$replacement_hash"
  regtest_wait_until_rpc_expr_eq \
    "usdb snapshot latest block commit after payload-version same-height replacement" \
    regtest_rpc_call_usdb_indexer \
    "get_snapshot_info" \
    "[]" \
    "((data.get('result') or {}).get('latest_block_commit', ''))" \
    "$new_bh_commit"

  regtest_log "The older v1.0 payload must remain valid because its height stayed on the canonical chain"
  regtest_validate_validator_payload_versioned_success "$payload_v1"

  regtest_log "The newer v1.1 payload must mismatch after its historical height is replaced"
  regtest_validate_validator_payload_versioned_consensus_error "$payload_v11" "-32042" "SNAPSHOT_ID_MISMATCH"

  regtest_log "USDB validator block-body payload-version upgrade reorg test succeeded."
  regtest_log "pass1=${pass1}, pass2=${pass2}, height_v1=${height_v1}, height_v11=${height_v11}, winner_id=${winner_id}"
}

main "$@"
