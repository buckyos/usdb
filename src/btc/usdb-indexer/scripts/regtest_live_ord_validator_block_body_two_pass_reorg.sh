#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-two-pass-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29942}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29943}"
BH_RPC_PORT="${BH_RPC_PORT:-29940}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29950}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29960}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatortwopassreorg}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-two-pass-reorg-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-two-pass-reorg-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
AGE_BLOCKS_BEFORE_SECOND_PASS="${AGE_BLOCKS_BEFORE_SECOND_PASS:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-two-pass-reorg]"

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

choose_winner_for_candidates() {
  local first_id="$1"
  local first_energy="$2"
  local second_id="$3"
  local second_energy="$4"
  python3 - "$first_id" "$first_energy" "$second_id" "$second_energy" <<'PY'
import sys
first_id, first_energy, second_id, second_energy = sys.argv[1], int(sys.argv[2]), sys.argv[3], int(sys.argv[4])
candidates = [
    {"inscription_id": first_id, "energy": first_energy},
    {"inscription_id": second_id, "energy": second_energy},
]
winner = min(candidates, key=lambda item: (-item["energy"], item["inscription_id"]))
print(winner["inscription_id"])
PY
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
  local mint_a_file mint_b_file pass1 pass2 historical_height historical_hash current_tip_height
  local pass1_energy pass2_energy winner_id payload_file replacement_address target_post_reorg_height

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

  mint_a_file="$WORK_DIR/usdb_validator_two_pass_reorg_mint_a.json"
  mint_b_file="$WORK_DIR/usdb_validator_two_pass_reorg_mint_b.json"
  cat >"$mint_a_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","prev":[]}
EOF
  cat >"$mint_b_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","prev":[]}
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
  historical_height="$current_tip_height"
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

  pass1_energy="$(pass_energy_at_height "$pass1" "$historical_height")"
  pass2_energy="$(pass_energy_at_height "$pass2" "$historical_height")"
  winner_id="$(choose_winner_for_candidates "$pass1" "$pass1_energy" "$pass2" "$pass2_energy")"

  payload_file="$WORK_DIR/validator_block_body_two_pass_reorg_payload.json"
  regtest_write_validator_competition_payload_for_passes_at_height "$payload_file" "$historical_height" "$winner_id" "$pass1" "$pass2"
  regtest_validate_validator_competition_payload_success "$payload_file"

  regtest_log "Triggering same-height reorg over two-pass competition payload height=${historical_height}"
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

  regtest_validate_validator_competition_payload_consensus_error "$payload_file" "-32042" "SNAPSHOT_ID_MISMATCH"
  regtest_log "USDB validator block-body two-pass reorg test succeeded."
}

main "$@"
