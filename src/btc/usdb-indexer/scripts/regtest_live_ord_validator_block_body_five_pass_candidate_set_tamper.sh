#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-five-pass-tamper-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30112}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30113}"
BH_RPC_PORT="${BH_RPC_PORT:-30110}"
USDB_RPC_PORT="${USDB_RPC_PORT:-30120}"
ORD_RPC_PORT="${ORD_RPC_PORT:-30130}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorfivepasstamper}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-five-pass-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-five-pass-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
AGE_BLOCKS_BETWEEN_PASSES="${AGE_BLOCKS_BETWEEN_PASSES:-1}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-five-pass-tamper]"

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
  local wallet_names candidate_ids
  local idx wallet_name receive_address mint_file pass_id height_competition
  local candidate_entries_json winner_id loser_id valid_payload tampered_payload

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

  wallet_names=("$ORD_WALLET_NAME" "$ORD_WALLET_NAME_B" "$ORD_WALLET_NAME" "$ORD_WALLET_NAME_B" "$ORD_WALLET_NAME")
  candidate_ids=()

  for idx in 0 1 2 3 4; do
    wallet_name="${wallet_names[$idx]}"
    receive_address="$(regtest_get_ord_wallet_receive_address "$wallet_name")"
    mint_file="$WORK_DIR/usdb_validator_five_pass_mint_$((idx + 1)).json"
    cat >"$mint_file" <<EOF
{"p":"usdb","op":"mint","eth_main":"0x$(printf '%040x' $((idx + 1)))","prev":[]}
EOF
    pass_id="$(regtest_ord_inscribe_file "$wallet_name" "$mint_file" "$receive_address")"
    candidate_ids+=("$pass_id")
    regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
    if (( idx < 4 && AGE_BLOCKS_BETWEEN_PASSES > 0 )); then
      regtest_mine_blocks "$AGE_BLOCKS_BETWEEN_PASSES" "$miner_address"
      regtest_wait_until_ord_server_synced_to_bitcoind
    fi
  done

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

  candidate_entries_json="$(regtest_build_validator_candidate_entries_for_passes_at_height "$height_competition" "${candidate_ids[@]}")"
  winner_id="$(regtest_choose_validator_candidate_set_winner_id "$candidate_entries_json")"
  valid_payload="$WORK_DIR/validator_block_body_five_pass_candidate_set_valid_payload.json"
  regtest_write_validator_candidate_set_payload_for_passes_at_height "$valid_payload" "$height_competition" "$winner_id" "${candidate_ids[@]}"
  regtest_validate_validator_candidate_set_payload_success "$valid_payload"
  regtest_assert_json_expr "$(cat "$valid_payload")" "((data.get('candidate_passes') or []).__len__())" "5"
  regtest_assert_json_expr "$(cat "$valid_payload")" "all(item.get('state') == 'active' for item in (data.get('candidate_passes') or []))" "True"

  loser_id="$(python3 - "$valid_payload" "$winner_id" <<'PY'
import json
import pathlib
import sys

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
winner_id = sys.argv[2]
for item in payload.get("candidate_passes") or []:
    if item["inscription_id"] != winner_id:
        print(item["inscription_id"])
        break
PY
)"

  tampered_payload="$WORK_DIR/validator_block_body_five_pass_candidate_set_tampered_payload.json"
  regtest_write_validator_competition_payload_tampered_winner "$valid_payload" "$tampered_payload" "$loser_id"
  regtest_validate_validator_candidate_set_payload_tampered_selection "$tampered_payload"

  regtest_log "USDB validator block-body five-pass candidate-set tamper test succeeded."
  regtest_log "winner=${winner_id}, tampered_winner=${loser_id}, candidate_count=5, competition_height=${height_competition}"
}

main "$@"
