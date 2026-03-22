#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-two-pass-energy-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-39032}"
BTC_P2P_PORT="${BTC_P2P_PORT:-39033}"
BH_RPC_PORT="${BH_RPC_PORT:-39010}"
USDB_RPC_PORT="${USDB_RPC_PORT:-39020}"
ORD_RPC_PORT="${ORD_RPC_PORT:-39030}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatortwopassenergy}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-two-pass-energy-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-two-pass-energy-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
ENERGY_TOP_UP_A_BTC="${ENERGY_TOP_UP_A_BTC:-1.0}"
ENERGY_TOP_UP_B_BTC="${ENERGY_TOP_UP_B_BTC:-6.0}"
ENERGY_TOP_UP_CONFIRM_BLOCKS="${ENERGY_TOP_UP_CONFIRM_BLOCKS:-1}"
ENERGY_GROWTH_BLOCKS_A="${ENERGY_GROWTH_BLOCKS_A:-2}"
ENERGY_GROWTH_BLOCKS_B="${ENERGY_GROWTH_BLOCKS_B:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-two-pass-energy]"

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
  local pass1 pass2 competition_height flipped_height
  local pass1_energy_h pass2_energy_h
  local pass1_energy_flipped pass2_energy_flipped
  local payload_h payload_flipped

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

  mint_a_file="$WORK_DIR/usdb_validator_two_pass_energy_mint_a.json"
  mint_b_file="$WORK_DIR/usdb_validator_two_pass_energy_mint_b.json"
  cat >"$mint_a_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","prev":[]}
EOF
  cat >"$mint_b_file" <<'EOF'
{"p":"usdb","op":"mint","eth_main":"0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","prev":[]}
EOF

  regtest_log "Minting pass1 and topping up its owner address to create real energy growth"
  pass1="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$mint_a_file" "$ord_receive_address_a")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_fund_address "$ord_receive_address_a" "$ENERGY_TOP_UP_A_BTC"
  regtest_mine_blocks "$ENERGY_TOP_UP_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  if (( ENERGY_GROWTH_BLOCKS_A > 0 )); then
    regtest_mine_blocks "$ENERGY_GROWTH_BLOCKS_A" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  fi

  regtest_log "Minting pass2 later so competition height sees a real energy difference"
  pass2="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$mint_b_file" "$ord_receive_address_b")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  competition_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$competition_height"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$competition_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  pass1_energy_h="$(pass_energy_at_height "$pass1" "$competition_height")"
  pass2_energy_h="$(pass_energy_at_height "$pass2" "$competition_height")"
  regtest_log "Real energy competition at H=${competition_height}: pass1=${pass1_energy_h}, pass2=${pass2_energy_h}"
  if (( pass1_energy_h <= 0 )); then
    regtest_log "Expected pass1 energy to be positive at competition height, got ${pass1_energy_h}"
    exit 1
  fi
  if (( pass1_energy_h <= pass2_energy_h )); then
    regtest_log "Expected pass1 energy to exceed pass2 at competition height, got pass1=${pass1_energy_h}, pass2=${pass2_energy_h}"
    exit 1
  fi

  payload_h="$WORK_DIR/validator_block_body_two_pass_energy_advantage_h.json"
  regtest_write_validator_competition_payload_for_passes_at_height "$payload_h" "$competition_height" "$pass1" "$pass1" "$pass2"
  regtest_validate_validator_competition_payload_success "$payload_h"
  regtest_assert_json_expr "$(cat "$payload_h")" "data.get('miner_selection', {}).get('inscription_id')" "$pass1"

  regtest_log "Topping up pass2 owner later to flip current energy ordering without changing historical winner at H"
  regtest_fund_address "$ord_receive_address_b" "$ENERGY_TOP_UP_B_BTC"
  regtest_mine_blocks "$ENERGY_TOP_UP_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  if (( ENERGY_GROWTH_BLOCKS_B > 0 )); then
    regtest_mine_blocks "$ENERGY_GROWTH_BLOCKS_B" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  fi
  flipped_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_balance_history_synced_eq "$flipped_height"
  regtest_wait_until_usdb_synced_eq "$flipped_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  pass1_energy_flipped="$(pass_energy_at_height "$pass1" "$flipped_height")"
  pass2_energy_flipped="$(pass_energy_at_height "$pass2" "$flipped_height")"
  regtest_log "Current energies after pass2 top-up at head=${flipped_height}: pass1=${pass1_energy_flipped}, pass2=${pass2_energy_flipped}"
  if (( pass2_energy_flipped <= pass1_energy_flipped )); then
    regtest_log "Expected pass2 energy to overtake pass1 after later top-up, got pass1=${pass1_energy_flipped}, pass2=${pass2_energy_flipped}"
    exit 1
  fi

  regtest_log "Historical payload at H must still validate even after current energy ordering flips"
  regtest_validate_validator_competition_payload_success "$payload_h"

  payload_flipped="$WORK_DIR/validator_block_body_two_pass_energy_advantage_flipped.json"
  regtest_write_validator_competition_payload_for_passes_at_height "$payload_flipped" "$flipped_height" "$pass2" "$pass1" "$pass2"
  regtest_validate_validator_competition_payload_success "$payload_flipped"
  regtest_assert_json_expr "$(cat "$payload_flipped")" "data.get('miner_selection', {}).get('inscription_id')" "$pass2"

  regtest_log "Historical and current energy-based payloads must describe different winners"
  regtest_assert_json_expr "$(python3 - "$payload_h" "$payload_flipped" <<'PY'
import json
import pathlib
import sys
h = json.loads(pathlib.Path(sys.argv[1]).read_text())
f = json.loads(pathlib.Path(sys.argv[2]).read_text())
print(json.dumps({"left": h["miner_selection"]["inscription_id"], "right": f["miner_selection"]["inscription_id"]}))
PY
)" "(data.get('result') or data).get('left') != (data.get('result') or data).get('right')" "True"

  regtest_log "USDB validator block-body two-pass real-energy-advantage test succeeded."
  regtest_log "pass1=${pass1}, pass2=${pass2}, h=${competition_height}, pass1_energy_h=${pass1_energy_h}, pass2_energy_h=${pass2_energy_h}, flipped_height=${flipped_height}, pass1_energy_flipped=${pass1_energy_flipped}, pass2_energy_flipped=${pass2_energy_flipped}"
}

main "$@"
