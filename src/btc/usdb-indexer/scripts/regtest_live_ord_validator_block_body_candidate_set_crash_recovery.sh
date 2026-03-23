#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-candidate-set-crash-recovery-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-31332}"
BTC_P2P_PORT="${BTC_P2P_PORT:-31333}"
BH_RPC_PORT="${BH_RPC_PORT:-31310}"
USDB_RPC_PORT="${USDB_RPC_PORT:-31320}"
ORD_RPC_PORT="${ORD_RPC_PORT:-31330}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorcandidatesetcrash}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-candidate-set-crash-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-candidate-set-crash-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
AGE_BLOCKS_BEFORE_SECOND_PASS="${AGE_BLOCKS_BEFORE_SECOND_PASS:-2}"
CRASH_GAP_BLOCKS="${CRASH_GAP_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-candidate-set-crash-recovery]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  [[ -x "$ORD_BIN" ]] || { echo "Missing required ORD_BIN executable: $ORD_BIN" >&2; exit 1; }
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3
  regtest_assert_ord_server_port_available

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local miner_address ord_receive_address_a ord_receive_address_b
  local mint_a_file mint_b_file pass1 pass2
  local current_tip_height payload_height candidate_entries_json winner_id payload_file continue_address

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

  mint_a_file="$WORK_DIR/usdb_validator_candidate_set_crash_recovery_mint_a.json"
  mint_b_file="$WORK_DIR/usdb_validator_candidate_set_crash_recovery_mint_b.json"
  cat >"$mint_a_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a1a","prev":[]}
EOF
  cat >"$mint_b_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0x2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b2b","prev":[]}
EOF

  pass1="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_a_file" "$ord_receive_address_a")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind

  if (( AGE_BLOCKS_BEFORE_SECOND_PASS > 0 )); then
    regtest_mine_blocks "$AGE_BLOCKS_BEFORE_SECOND_PASS" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  fi

  pass2="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$mint_b_file" "$ord_receive_address_b")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  current_tip_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  payload_height="$((current_tip_height - 1))"

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
  regtest_wait_usdb_state_ref_available "$payload_height"

  candidate_entries_json="$(regtest_build_validator_candidate_entries_for_passes_at_height "$payload_height" "$pass1" "$pass2")"
  winner_id="$(regtest_choose_validator_candidate_set_winner_id "$candidate_entries_json")"
  payload_file="$WORK_DIR/validator_block_body_candidate_set_crash_recovery_payload.json"
  regtest_write_validator_candidate_set_payload_for_passes_at_height "$payload_file" "$payload_height" "$winner_id" "$pass1" "$pass2"
  regtest_validate_validator_payload_versioned_success "$payload_file"

  regtest_log "Crashing both services after candidate-set payload capture"
  regtest_crash_usdb_indexer
  regtest_crash_balance_history
  continue_address="$(regtest_get_new_address)"
  regtest_mine_blocks "$CRASH_GAP_BLOCKS" "$continue_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  current_tip_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$current_tip_height"
  regtest_wait_until_usdb_synced_eq "$current_tip_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready
  regtest_wait_usdb_state_ref_available "$payload_height"

  regtest_validate_validator_payload_versioned_success "$payload_file"
  regtest_log "USDB validator block-body candidate-set crash recovery test succeeded."
}

main "$@"
