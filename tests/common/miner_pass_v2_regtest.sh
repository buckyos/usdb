#!/usr/bin/env bash
# V2 transition checks shared by the real-service profile runner.

regtest_v2_sync() {
  regtest_mine_blocks "$((BTC_STABLE_LAG_BLOCKS + 2))" "$1"
  regtest_wait_until_ord_server_synced_to_bitcoind
  V2_CONTEXT_HEIGHT=$(( $("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount) - BTC_STABLE_LAG_BLOCKS ))
  regtest_wait_until_balance_history_synced_eq "$V2_CONTEXT_HEIGHT"
  regtest_wait_balance_history_consensus_ready
  regtest_wait_until_usdb_synced_eq "$V2_CONTEXT_HEIGHT"
  regtest_wait_usdb_consensus_ready
}

regtest_v2_assert_audit() {
  local pass_id="$1" path="$2" source_address="${3:-}" response source_outpoint source_txid source_vout tx
  response="$(regtest_rpc_call_usdb_indexer get_pass_mint_audit "[{\"inscription_id\":\"$pass_id\"}]")"
  printf '%s\n' "$response" >"$WORK_DIR/v2-audit-${pass_id}.json"
  regtest_assert_json_expr "$response" "data.get('error') is None" True
  regtest_assert_json_expr "$response" "data['result']['audit']['operation_path']" "$path"
  regtest_assert_json_expr "$response" "data['result']['activation_registry_id']" d53e9907cfc5abf5d8294e98bbaa838630ee070118eb0845ad3e770959279f08
  regtest_assert_json_expr "$response" "data['result']['active_version_set_id']" 58a5bf5a3cfdba184c57a7ab13d6d7d6c19a359da467625ebc0392459a0f2a18
  if [[ "$path" == first_opening || "$path" == cross_owner ]]; then
    regtest_assert_json_expr "$response" "data['result']['audit']['balance_before_tx']" 0
    regtest_assert_json_expr "$response" "data['result']['audit']['ever_valid_owner']" False
  fi
  if [[ "$path" == None ]]; then
    regtest_assert_json_expr "$response" "data['result']['audit']['error_code']" INELIGIBLE_RECIPIENT
  else
    regtest_assert_json_expr "$response" "data['result']['audit']['error_code']" None
  fi
  if [[ -n "$source_address" ]]; then
    regtest_assert_json_expr "$response" "data['result']['audit']['source']['authorization']" taproot_key_path
    source_outpoint="$(regtest_json_expr "$response" "data['result']['audit']['source']['source_outpoint']")"
    source_txid="${source_outpoint%:*}"
    source_vout="${source_outpoint##*:}"
    tx="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getrawtransaction "$source_txid" true)"
    regtest_assert_json_expr "$tx" "data['vout'][$source_vout]['scriptPubKey']['address']" "$source_address"
  fi
}

regtest_v2_write_mint() {
  python3 - "$1" "$MINER_PASS_USDB_MAIN" "$2" <<'PY'
import json
from pathlib import Path
import sys
Path(sys.argv[1]).write_text(json.dumps({
    "p": "usdb", "op": "mint", "v": 1, "usdb_main": sys.argv[2], "prev": [sys.argv[3]],
}) + "\n")
PY
}

# The caller has already opened and funded D. Return the final active pass and E via globals.
regtest_run_miner_pass_v2_transitions() {
  local miner_address="$1" first_pass="$2" owner="$3"
  local remint gift cross attacker destination protected_before
  regtest_v2_assert_audit "$first_pass" first_opening
  regtest_v2_write_mint "$WORK_DIR/v2-same-owner.json" "$first_pass"
  protected_before="$(regtest_run_ord_wallet_named "$ORD_WALLET_NAME" inscriptions)"
  remint="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$WORK_DIR/v2-same-owner.json" "$owner" "$owner")"
  regtest_v2_sync "$miner_address"
  regtest_assert_usdb_pass_snapshot_state "$first_pass" "$V2_CONTEXT_HEIGHT" consumed
  regtest_assert_usdb_pass_snapshot_state "$remint" "$V2_CONTEXT_HEIGHT" active
  regtest_v2_assert_audit "$remint" same_owner "$owner"

  # A third party must not consume the incumbent pass by gifting a forged prev mint.
  attacker="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  regtest_fund_address "$attacker" 1.0
  regtest_mine_blocks 1 "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_v2_write_mint "$WORK_DIR/v2-forced-gift.json" "$remint"
  gift="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$WORK_DIR/v2-forced-gift.json" "$owner" "$attacker")"
  regtest_v2_sync "$miner_address"
  regtest_assert_usdb_pass_snapshot_state "$gift" "$V2_CONTEXT_HEIGHT" invalid
  regtest_assert_usdb_pass_snapshot_state "$remint" "$V2_CONTEXT_HEIGHT" active
  regtest_v2_assert_audit "$gift" None "$attacker"

  # Refill D with a cardinal coin; never use either previous pass UTXO for the commit.
  regtest_fund_address "$owner" 1.0
  regtest_mine_blocks 1 "$miner_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
  destination="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  regtest_v2_write_mint "$WORK_DIR/v2-cross-owner.json" "$remint"
  regtest_run_ord_wallet_named "$ORD_WALLET_NAME" inscriptions >"$WORK_DIR/v2-protected-before.json"
  cross="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$WORK_DIR/v2-cross-owner.json" "$destination" "$owner")"
  regtest_v2_sync "$miner_address"
  regtest_assert_usdb_pass_snapshot_state "$remint" "$V2_CONTEXT_HEIGHT" consumed
  regtest_assert_usdb_pass_snapshot_state "$cross" "$V2_CONTEXT_HEIGHT" active
  regtest_v2_assert_audit "$cross" cross_owner "$owner"
  regtest_run_ord_wallet_named "$ORD_WALLET_NAME" inscriptions >"$WORK_DIR/v2-protected-after.json"
  python3 - "$WORK_DIR" "$protected_before" <<'PY'
import json
from pathlib import Path
import sys
root = Path(sys.argv[1])
initial = json.loads(sys.argv[2])
before = json.loads((root / "v2-protected-before.json").read_text())
after = json.loads((root / "v2-protected-after.json").read_text())
positions = {item["inscription"]: item["location"] for item in after}
for item in initial + before:
    if positions.get(item["inscription"]) != item["location"]:
        raise SystemExit(f"Protected inscription moved or spent: {item}")
PY
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$cross"
  regtest_fund_address "$destination" "$ENERGY_TOPUP_AMOUNT_BTC"
  regtest_v2_sync "$miner_address"
  # Results are consumed by the sourcing Geth profile runner.
  # shellcheck disable=SC2034
  V2_FINAL_PASS_ID="$cross"
  # shellcheck disable=SC2034
  V2_FINAL_OWNER="$destination"
  python3 - "$WORK_DIR/v2-transitions.json" "$first_pass" "$remint" "$gift" "$cross" "$V2_CONTEXT_HEIGHT" <<'PY'
import json
from pathlib import Path
import sys
Path(sys.argv[1]).write_text(json.dumps({
    "first_opening": sys.argv[2], "same_owner": sys.argv[3],
    "rejected_gift": sys.argv[4], "cross_owner": sys.argv[5],
    "context_height": int(sys.argv[6]), "protected_utxos_unchanged": True,
}, indent=2) + "\n")
PY
  regtest_log "MinerPass V2 transitions verified: first=${first_pass}, same=${remint}, rejected=${gift}, cross=${cross}"
}
