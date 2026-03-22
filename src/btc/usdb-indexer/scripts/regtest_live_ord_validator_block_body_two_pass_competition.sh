#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-two-pass-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29892}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29893}"
BH_RPC_PORT="${BH_RPC_PORT:-29890}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29900}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29910}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatortwopass}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-two-pass-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-two-pass-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
AGE_BLOCKS_BEFORE_SECOND_PASS="${AGE_BLOCKS_BEFORE_SECOND_PASS:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-two-pass]"

source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

pass_energy_at_height() {
  local inscription_id="$1"
  local block_height="$2"
  local resp

  resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "[{\"inscription_id\":\"${inscription_id}\",\"block_height\":${block_height},\"mode\":\"at_or_before\"}]")"
  if [[ "$(regtest_json_expr "$resp" "data.get('error') is None")" != "True" ]]; then
    regtest_log "Failed to fetch pass energy at height=${block_height}, inscription_id=${inscription_id}, response=${resp}"
    exit 1
  fi
  regtest_json_expr "$resp" "(data.get('result') or {}).get('energy', 0)"
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
  local mint_a_file mint_b_file
  local pass1 pass2 height_competition height_after_transfer
  local payload_competition
  local pass1_energy pass2_energy continue_address
  local winner_id winner_wallet winner_destination loser_id

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

  mint_a_file="$WORK_DIR/usdb_validator_two_pass_mint_a.json"
  mint_b_file="$WORK_DIR/usdb_validator_two_pass_mint_b.json"
  cat >"$mint_a_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","prev":[]}
EOF
  cat >"$mint_b_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","prev":[]}
EOF

  regtest_log "Minting first candidate pass and aging it before second candidate appears"
  pass1="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_a_file" "$ord_receive_address_a")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  if (( AGE_BLOCKS_BEFORE_SECOND_PASS > 0 )); then
    regtest_mine_blocks "$AGE_BLOCKS_BEFORE_SECOND_PASS" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  fi

  regtest_log "Minting second candidate pass at the competition height"
  pass2="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$mint_b_file" "$ord_receive_address_b")"
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

  regtest_assert_usdb_pass_snapshot_state "$pass1" "$height_competition" "active"
  regtest_assert_usdb_pass_snapshot_state "$pass2" "$height_competition" "active"

  pass1_energy="$(pass_energy_at_height "$pass1" "$height_competition")"
  pass2_energy="$(pass_energy_at_height "$pass2" "$height_competition")"
  winner_id="$(python3 - "$pass1" "$pass1_energy" "$pass2" "$pass2_energy" <<'PY'
import sys

pass1, energy1, pass2, energy2 = sys.argv[1], int(sys.argv[2]), sys.argv[3], int(sys.argv[4])
candidates = [
    {"inscription_id": pass1, "energy": energy1},
    {"inscription_id": pass2, "energy": energy2},
]
winner = min(candidates, key=lambda item: (-item["energy"], item["inscription_id"]))
print(winner["inscription_id"])
PY
)"
  if [[ "$winner_id" == "$pass1" ]]; then
    winner_wallet="$ORD_WALLET_NAME"
    winner_destination="$ord_receive_address_b"
    loser_id="$pass2"
  else
    winner_wallet="$ORD_WALLET_NAME_B"
    winner_destination="$ord_receive_address_a"
    loser_id="$pass1"
  fi
  regtest_log "Two-pass competition energies at H=${height_competition}: pass1=${pass1_energy}, pass2=${pass2_energy}, winner=${winner_id}"

  payload_competition="$WORK_DIR/validator_block_body_two_pass_competition_payload.json"
  write_validator_competition_payload_for_passes_at_height "$payload_competition" "$height_competition" "$winner_id" "$pass1" "$pass2"
  regtest_log "Wrote two-pass competition payload at height=${height_competition}: ${payload_competition}"

  regtest_log "Competition payload must validate both candidates and winner relation at the original historical state"
  regtest_validate_validator_competition_payload_success "$payload_competition"
  regtest_assert_json_expr "$(cat "$payload_competition")" "((data.get('candidate_passes') or []).__len__())" "2"
  regtest_assert_json_expr "$(cat "$payload_competition")" "data.get('miner_selection', {}).get('inscription_id')" "$winner_id"

  regtest_log "Advancing winner pass state at H+1 must not break historical competition payload"
  regtest_ord_send_inscription "$winner_wallet" "$winner_destination" "$winner_id" >/dev/null
  regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  if [[ "$winner_wallet" == "$ORD_WALLET_NAME" ]]; then
    regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$winner_id"
  else
    regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME" "$winner_id"
  fi
  height_after_transfer="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_balance_history_synced_eq "$height_after_transfer"
  regtest_wait_until_usdb_synced_eq "$height_after_transfer"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_assert_usdb_pass_snapshot_state "$winner_id" "$height_after_transfer" "dormant"
  regtest_assert_usdb_pass_snapshot_state "$loser_id" "$height_after_transfer" "active"
  regtest_validate_validator_competition_payload_success "$payload_competition"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((height_after_transfer + 1))"
  regtest_wait_until_usdb_synced_eq "$((height_after_transfer + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_log "Historical two-pass competition payload must still validate after another head advance"
  regtest_validate_validator_competition_payload_success "$payload_competition"

  regtest_log "USDB validator block-body two-pass competition test succeeded."
  regtest_log "pass1=${pass1}, pass2=${pass2}, winner=${winner_id}, loser=${loser_id}, competition_height=${height_competition}, transfer_height=${height_after_transfer}"
}

main "$@"
