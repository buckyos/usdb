#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-validator-block-body-three-collab-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
USDB_INDEXER_ROOT="${USDB_INDEXER_ROOT:-$WORK_DIR/usdb-indexer}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30932}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30933}"
BH_RPC_PORT="${BH_RPC_PORT:-30910}"
USDB_INDEXER_RPC_PORT="${USDB_INDEXER_RPC_PORT:-30920}"
ORD_RPC_PORT="${ORD_RPC_PORT:-30930}"
WALLET_NAME="${WALLET_NAME:-usdbvalidatorthreecollab}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-three-collab-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-three-collab-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-2}"
LEADER_TOP_UP_BTC="${LEADER_TOP_UP_BTC:-0.02}"
COLLAB_TOP_UP_BTC="${COLLAB_TOP_UP_BTC:-0.01}"
ENERGY_TOP_UP_CONFIRM_BLOCKS="${ENERGY_TOP_UP_CONFIRM_BLOCKS:-1}"
ENERGY_GROWTH_BLOCKS="${ENERGY_GROWTH_BLOCKS:-2}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-validator-block-body-three-collab]"

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

  local miner_address funding_address_a funding_address_b
  local leader_address leader_mint_file leader_pass_id
  local idx wallet_name collab_address collab_mint_file collab_pass_id
  local query_height head_height profile_resp candidate_view breakdown_view
  local contribution_breakdown_view payload_file
  local continue_address collab_profile_resp
  local -a collab_wallets collab_addresses collab_ids

  miner_address="$(regtest_get_new_address)"
  regtest_mine_blocks "$PREMINE_BLOCKS" "$miner_address"

  regtest_start_ord_server
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_prepare_ord_wallets

  funding_address_a="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  funding_address_b="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  regtest_fund_address "$funding_address_a" "$FUND_ORD_AMOUNT_BTC"
  regtest_fund_address "$funding_address_b" "$FUND_ORD_AMOUNT_BTC"
  regtest_mine_blocks "$FUND_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind

  leader_address="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  leader_mint_file="$WORK_DIR/usdb_validator_three_collab_leader.json"
  cat >"$leader_mint_file" <<'EOF'
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF
  leader_pass_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$leader_mint_file" "$leader_address")"
  regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind

  collab_wallets=("$ORD_WALLET_NAME_B" "$ORD_WALLET_NAME" "$ORD_WALLET_NAME_B")
  collab_addresses=()
  collab_ids=()
  regtest_log "Minting three collab passes bound to leader_pass_id=${leader_pass_id}"
  for idx in 0 1 2; do
    wallet_name="${collab_wallets[$idx]}"
    collab_address="$(regtest_get_ord_wallet_receive_address "$wallet_name")"
    collab_mint_file="$WORK_DIR/usdb_validator_three_collab_$((idx + 1)).json"
    cat >"$collab_mint_file" <<EOF
{"p":"usdb","op":"mint","v":1,"leader_pass_id":"${leader_pass_id}","prev":[]}
EOF
    collab_pass_id="$(regtest_ord_inscribe_file "$wallet_name" "$collab_mint_file" "$collab_address")"
    collab_addresses+=("$collab_address")
    collab_ids+=("$collab_pass_id")
    regtest_mine_blocks "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  done

  regtest_log "Funding leader and collab owners so every contribution is positive"
  regtest_fund_address "$leader_address" "$LEADER_TOP_UP_BTC"
  for collab_address in "${collab_addresses[@]}"; do
    regtest_fund_address "$collab_address" "$COLLAB_TOP_UP_BTC"
  done
  regtest_mine_blocks "$ENERGY_TOP_UP_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  if (( ENERGY_GROWTH_BLOCKS > 0 )); then
    regtest_mine_blocks "$ENERGY_GROWTH_BLOCKS" "$miner_address"
    regtest_wait_until_ord_server_synced_to_bitcoind
  fi
  query_height="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"

  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$query_height"
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$query_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready

  profile_resp="$(regtest_get_pass_economic_profile_response "$leader_pass_id" "$query_height")"
  regtest_assert_json_expr "$profile_resp" "data.get('error') is None" "True"
  candidate_view="$(regtest_collect_candidate_set_view "$query_height" "" "$USDB_ECONOMIC_PAGE_LIMIT")"
  breakdown_view="$(regtest_collect_collab_breakdown \
    "$leader_pass_id" "$query_height" "" "collab_pass_id_asc" "$USDB_ECONOMIC_PAGE_LIMIT")"
  contribution_breakdown_view="$(regtest_collect_collab_breakdown \
    "$leader_pass_id" "$query_height" "" "contribution_desc_pass_id_asc" "$USDB_ECONOMIC_PAGE_LIMIT")"

  python3 - \
    "$profile_resp" \
    "$candidate_view" \
    "$breakdown_view" \
    "$contribution_breakdown_view" \
    "${collab_ids[@]}" <<'PY'
import json
import sys

profile_response = json.loads(sys.argv[1])
candidate_view = json.loads(sys.argv[2])
breakdown = json.loads(sys.argv[3])
contribution_breakdown = json.loads(sys.argv[4])
expected_collab_ids = sys.argv[5:]
profile = profile_response["result"]
leader = profile["pass"]

if leader["state"] != "active" or leader["pass_kind"] != "standard":
    raise SystemExit("leader profile is not active standard")
if int(leader["raw_energy"]) <= 0:
    raise SystemExit("leader raw energy must be positive")
if int(leader["collab_contribution"]) <= 0:
    raise SystemExit("leader collab contribution must be positive")
if int(leader["collab_breakdown_count"]) != 3:
    raise SystemExit("leader profile collab_breakdown_count must be 3")

candidate_items = candidate_view.get("items") or []
if int(candidate_view["total"]) != 1 or len(candidate_items) != 1:
    raise SystemExit("candidate view must contain only the standard leader")
candidate = candidate_items[0]
if candidate["pass_id"] != leader["pass_id"]:
    raise SystemExit("candidate view does not contain the leader")
for field in (
    "raw_energy", "collab_contribution", "effective_energy", "level", "difficulty_factor_bps"
):
    if candidate[field] != leader[field]:
        raise SystemExit(f"candidate/profile mismatch for {field}")

items = breakdown.get("items") or []
if int(breakdown["limit"]) != 2 or int(breakdown["total"]) != 3 or len(items) != 3:
    raise SystemExit("breakdown collector did not combine the expected two cursor pages")
actual_collab_ids = [item["collab_pass_id"] for item in items]
if set(actual_collab_ids) != set(expected_collab_ids):
    raise SystemExit("breakdown collab ids do not match minted collab passes")
for item in items:
    if int(item["collab_raw_energy"]) <= 0 or int(item["collab_contribution"]) <= 0:
        raise SystemExit("every collab breakdown row must have positive energy")
    if int(item["collab_weight_bps"]) != 5000:
        raise SystemExit("unexpected collab weight")
    if item["leader_ref_kind"] != "leader_pass_id" or item["leader_ref_value"] != leader["pass_id"]:
        raise SystemExit("unexpected collab leader reference")

aggregate = sum(int(item["collab_contribution"]) for item in items)
if aggregate != int(breakdown["aggregate_collab_contribution"]):
    raise SystemExit("breakdown rows do not recompute aggregate")
if aggregate != int(leader["collab_contribution"]):
    raise SystemExit("profile collab contribution does not match breakdown aggregate")
if int(leader["effective_energy"]) != int(leader["raw_energy"]) + aggregate:
    raise SystemExit("leader effective energy does not equal raw plus collab aggregate")

contribution_items = contribution_breakdown.get("items") or []
if (
    int(contribution_breakdown["limit"]) != 2
    or int(contribution_breakdown["total"]) != 3
    or len(contribution_items) != 3
):
    raise SystemExit("contribution breakdown did not combine the expected two cursor pages")
if {item["collab_pass_id"] for item in contribution_items} != set(expected_collab_ids):
    raise SystemExit("contribution breakdown collab ids do not match minted collab passes")
expected_contribution_items = sorted(
    contribution_items,
    key=lambda item: (-int(item["collab_contribution"]), item["collab_pass_id"]),
)
if contribution_items != expected_contribution_items:
    raise SystemExit("contribution breakdown ordering is not canonical")
if contribution_breakdown["external_state"] != breakdown["external_state"]:
    raise SystemExit("breakdown sort variants resolved different external states")
if contribution_breakdown["aggregate_collab_contribution"] != breakdown["aggregate_collab_contribution"]:
    raise SystemExit("breakdown sort variants returned different aggregates")
PY

  for collab_pass_id in "${collab_ids[@]}"; do
    collab_profile_resp="$(regtest_get_pass_economic_profile_response "$collab_pass_id" "$query_height")"
    regtest_assert_json_expr "$collab_profile_resp" "data.get('error') is None" "True"
    regtest_assert_json_expr "$collab_profile_resp" "(data.get('result') or {}).get('pass', {}).get('pass_kind')" "collab"
    regtest_assert_json_expr "$collab_profile_resp" "int((data.get('result') or {}).get('pass', {}).get('raw_energy', '0')) > 0" "True"
    regtest_assert_json_expr "$collab_profile_resp" "(data.get('result') or {}).get('pass', {}).get('collab_contribution')" "0"
    regtest_assert_json_expr "$collab_profile_resp" "(data.get('result') or {}).get('pass', {}).get('effective_energy')" "0"
    regtest_assert_json_expr "$collab_profile_resp" "(data.get('result') or {}).get('pass', {}).get('level')" "0"
    regtest_assert_json_expr "$collab_profile_resp" "(data.get('result') or {}).get('pass', {}).get('difficulty_factor_bps')" "10000"
  done

  payload_file="$WORK_DIR/validator_block_body_three_collab_payload.json"
  regtest_write_validator_payload_from_profile_v1 "$payload_file" "$profile_resp"
  regtest_validate_validator_payload_success "$payload_file"

  regtest_log "Advancing head must preserve the historical two-page collab breakdown"
  continue_address="$(regtest_get_new_address)"
  regtest_mine_empty_block "$continue_address"
  head_height="$((query_height + 1))"
  regtest_wait_until_balance_history_synced_eq "$head_height"
  regtest_wait_until_usdb_synced_eq "$head_height"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_usdb_consensus_ready
  regtest_validate_validator_payload_success "$payload_file"

  regtest_log "USDB validator block-body three-collab breakdown test succeeded."
  regtest_log "leader=${leader_pass_id}, collabs=${collab_ids[*]}, query_height=${query_height}, head_height=${head_height}"
}

main "$@"
