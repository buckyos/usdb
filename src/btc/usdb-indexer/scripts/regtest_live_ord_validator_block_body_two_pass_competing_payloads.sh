#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-two-pass-competing-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-29912}"
BTC_P2P_PORT="${BTC_P2P_PORT:-29913}"
BH_RPC_PORT="${BH_RPC_PORT:-29910}"
USDB_RPC_PORT="${USDB_RPC_PORT:-29920}"
ORD_RPC_PORT="${ORD_RPC_PORT:-29930}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatortwopasscompeting}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-two-pass-competing-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-two-pass-competing-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
AGE_BLOCKS_BEFORE_SECOND_PASS="${AGE_BLOCKS_BEFORE_SECOND_PASS:-3}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-two-pass-competing]"

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

build_payload_context_with_requested_height() {
  local payload_file="$1"
  local requested_height="$2"
  local snapshot_id stable_block_hash local_state_commit system_state_id protocol_version

  snapshot_id="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['snapshot_id']")"
  stable_block_hash="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['stable_block_hash']")"
  local_state_commit="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['local_state_commit']")"
  system_state_id="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['system_state_id']")"
  protocol_version="$(regtest_validator_payload_expr "$payload_file" "data['external_state']['usdb_index_protocol_version']")"

  python3 - "$requested_height" "$snapshot_id" "$stable_block_hash" "$local_state_commit" "$system_state_id" "$protocol_version" <<'PY'
import json
import sys

print(json.dumps({
    "requested_height": int(sys.argv[1]),
    "expected_state": {
        "snapshot_id": sys.argv[2],
        "stable_block_hash": sys.argv[3],
        "local_state_commit": sys.argv[4],
        "system_state_id": sys.argv[5],
        "usdb_index_protocol_version": sys.argv[6],
    },
}))
PY
}

assert_competition_payload_context_mismatch() {
  local payload_file="$1"
  local target_height="$2"
  local expected_code="$3"
  local expected_message="$4"
  local context_json state_ref_params resp candidate_count idx candidate_id
  local snapshot_params energy_params

  context_json="$(build_payload_context_with_requested_height "$payload_file" "$target_height")"
  state_ref_params="$(python3 - "$target_height" "$context_json" <<'PY'
import json
import sys
print(json.dumps([{
    "block_height": int(sys.argv[1]),
    "context": json.loads(sys.argv[2]),
}]))
PY
)"

  resp="$(regtest_rpc_call_usdb_indexer "get_state_ref_at_height" "$state_ref_params")"
  regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"

  candidate_count="$(regtest_validator_payload_expr "$payload_file" "((data.get('candidate_passes') or []).__len__())")"
  for idx in $(seq 0 $((candidate_count - 1))); do
    candidate_id="$(regtest_validator_payload_expr "$payload_file" "data['candidate_passes'][$idx]['inscription_id']")"
    snapshot_params="$(python3 - "$candidate_id" "$target_height" "$context_json" <<'PY'
import json
import sys
print(json.dumps([{
    "inscription_id": sys.argv[1],
    "at_height": int(sys.argv[2]),
    "context": json.loads(sys.argv[3]),
}]))
PY
)"
    resp="$(regtest_rpc_call_usdb_indexer "get_pass_snapshot" "$snapshot_params")"
    regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"

    energy_params="$(python3 - "$candidate_id" "$target_height" "$context_json" <<'PY'
import json
import sys
print(json.dumps([{
    "inscription_id": sys.argv[1],
    "block_height": int(sys.argv[2]),
    "mode": "at_or_before",
    "context": json.loads(sys.argv[3]),
}]))
PY
)"
    resp="$(regtest_rpc_call_usdb_indexer "get_pass_energy" "$energy_params")"
    regtest_assert_usdb_consensus_error "$resp" "$expected_code" "$expected_message"
  done
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
  local mint_a_file mint_b_file pass1 pass2
  local height_h height_h1 winner_h loser_h
  local pass1_energy_h pass2_energy_h pass1_energy_h1 pass2_energy_h1
  local payload_h payload_h1 continue_address

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

  mint_a_file="$WORK_DIR/usdb_validator_two_pass_competing_mint_a.json"
  mint_b_file="$WORK_DIR/usdb_validator_two_pass_competing_mint_b.json"
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
  height_h="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$height_h"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$height_h"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  pass1_energy_h="$(pass_energy_at_height "$pass1" "$height_h")"
  pass2_energy_h="$(pass_energy_at_height "$pass2" "$height_h")"
  winner_h="$(choose_winner_for_candidates "$pass1" "$pass1_energy_h" "$pass2" "$pass2_energy_h")"
  if [[ "$winner_h" == "$pass1" ]]; then
    loser_h="$pass2"
    regtest_ord_send_inscription "$ORD_WALLET_NAME" "$ord_receive_address_b" "$winner_h" >/dev/null
    regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
    regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$winner_h"
  else
    loser_h="$pass1"
    regtest_ord_send_inscription "$ORD_WALLET_NAME_B" "$ord_receive_address_a" "$winner_h" >/dev/null
    regtest_mine_blocks "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
    regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME" "$winner_h"
  fi

  height_h1="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  regtest_wait_until_balance_history_synced_eq "$height_h1"
  regtest_wait_until_usdb_synced_eq "$height_h1"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  payload_h="$WORK_DIR/validator_block_body_two_pass_payload_h.json"
  regtest_write_validator_competition_payload_for_passes_at_height "$payload_h" "$height_h" "$winner_h" "$pass1" "$pass2"
  payload_h1="$WORK_DIR/validator_block_body_two_pass_payload_h1.json"
  regtest_write_validator_competition_payload_for_passes_at_height "$payload_h1" "$height_h1" "$loser_h" "$loser_h"

  regtest_log "Both multi-pass payloads must validate at their own historical heights"
  regtest_validate_validator_competition_payload_success "$payload_h"
  regtest_validate_validator_competition_payload_success "$payload_h1"

  regtest_log "Historical two-pass payloads must describe different competition states"
  regtest_assert_json_expr \
    "{\"result\":{\"left\":\"$(regtest_validator_payload_expr "$payload_h" "data['external_state']['snapshot_id']")\",\"right\":\"$(regtest_validator_payload_expr "$payload_h1" "data['external_state']['snapshot_id']")\"}}" \
    "(data.get('result') or {}).get('left') != (data.get('result') or {}).get('right')" \
    "True"
  regtest_assert_json_expr \
    "{\"result\":{\"left\":\"$(regtest_validator_payload_expr "$payload_h" "((data.get('candidate_passes') or []).__len__())")\",\"right\":\"$(regtest_validator_payload_expr "$payload_h1" "((data.get('candidate_passes') or []).__len__())")\"}}" \
    "(data.get('result') or {}).get('left') != (data.get('result') or {}).get('right')" \
    "True"
  regtest_assert_json_expr \
    "{\"result\":{\"left\":\"$(regtest_validator_payload_expr "$payload_h" "data['miner_selection']['inscription_id']")\",\"right\":\"$(regtest_validator_payload_expr "$payload_h1" "data['miner_selection']['inscription_id']")\"}}" \
    "(data.get('result') or {}).get('left') != (data.get('result') or {}).get('right')" \
    "True"

  regtest_log "Older multi-pass payload cannot be reused at H+1"
  assert_competition_payload_context_mismatch "$payload_h" "$height_h1" "-32042" "SNAPSHOT_ID_MISMATCH"
  regtest_log "Newer multi-pass payload cannot be reused at H"
  assert_competition_payload_context_mismatch "$payload_h1" "$height_h" "-32042" "SNAPSHOT_ID_MISMATCH"

  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  regtest_wait_until_balance_history_synced_eq "$((height_h1 + 1))"
  regtest_wait_until_usdb_synced_eq "$((height_h1 + 1))"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  regtest_validate_validator_competition_payload_success "$payload_h"
  regtest_validate_validator_competition_payload_success "$payload_h1"

  regtest_log "USDB validator block-body two-pass competing-payloads test succeeded."
  regtest_log "pass1=${pass1}, pass2=${pass2}, winner_h=${winner_h}, winner_h1=${loser_h}, h=${height_h}, h1=${height_h1}"
}

main "$@"
