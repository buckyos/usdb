#!/usr/bin/env bash
# Real-chain V2 attack, collaboration rotation, rollback, crash/reopen, and replay matrix.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK_DIR="${MINER_PASS_V2_MATRIX_WORK_DIR:-${WORK_DIR:-$(mktemp -d /tmp/usdb-v2-security-XXXXXX)}}"
MINER_PASS_USDB_MAIN=0x1111111111111111111111111111111111111111
REGTEST_LOG_PREFIX='[miner-pass-v2-security]'
source "$REPO_ROOT/src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh"
source "$REPO_ROOT/tests/common/miner_pass_v2_regtest.sh"

matrix_mint() {
  local name="$1" wallet="$2" destination="$3" source="$4" prev="$5" extra="${6:-}"
  regtest_fund_address "$source" 0.1
  regtest_mine_blocks 1 "$MINING_ADDRESS"
  regtest_wait_until_ord_server_synced_to_bitcoind
  python3 - "$WORK_DIR/$name.json" "$prev" "$extra" <<'PY'
import json
from pathlib import Path
import sys
payload = dict(p="usdb", op="mint", v=1, prev=json.loads(sys.argv[2]))
payload.update(json.loads(sys.argv[3]) if sys.argv[3] else dict(usdb_main="0x" + "11" * 20))
Path(sys.argv[1]).write_text(json.dumps(payload) + "\n")
PY
  MATRIX_PASS_ID="$(regtest_ord_inscribe_file "$wallet" "$WORK_DIR/$name.json" "$destination" "$source")"
  printf '%s\t%s\n' "$name" "$MATRIX_PASS_ID" >>"$WORK_DIR/cases.tsv"
  regtest_v2_sync "$MINING_ADDRESS"
}

matrix_snapshot() {
  python3 "$REPO_ROOT/tests/common/miner_pass_matrix_snapshot.py" \
    --rpc "http://127.0.0.1:$USDB_INDEXER_RPC_PORT" --cases "$WORK_DIR/cases.tsv" \
    --height "$V2_CONTEXT_HEIGHT" --output "$1"
}

main() {
  trap regtest_cleanup EXIT
  regtest_resolve_bitcoin_binaries
  regtest_assert_ord_server_port_available
  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet
  MINING_ADDRESS="$(regtest_get_new_address)"
  regtest_mine_blocks 130 "$MINING_ADDRESS"
  regtest_start_ord_server
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_prepare_ord_wallets
  local a b d e victim first leader fixed address_collab rotated destination fixed_new address_new
  local response rotation_height rotation_hash rollback_height tip
  a="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  b="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  regtest_create_balance_history_config
  regtest_create_usdb_indexer_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready

  # A nonparticipating P2WPKH holder cannot be enrolled merely by receiving a mint.
  # Keep the victim outside the funding wallet so automatic coin selection cannot spend it.
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" createwallet passive-holder >/dev/null
  victim="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" -rpcwallet=passive-holder getnewaddress '' bech32)"
  regtest_fund_address "$victim" 1.0
  regtest_mine_blocks 1 "$MINING_ADDRESS"
  matrix_mint passive-holder "$ORD_WALLET_NAME" "$victim" "$a" '[]' '{"usdb_main":"0x2222222222222222222222222222222222222222"}'
  regtest_assert_usdb_pass_snapshot_state "$MATRIX_PASS_ID" "$V2_CONTEXT_HEIGHT" invalid
  regtest_v2_assert_audit "$MATRIX_PASS_ID" None "$a"

  d="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  matrix_mint opening "$ORD_WALLET_NAME" "$d" "$a" '[]'
  first="$MATRIX_PASS_ID"
  regtest_v2_assert_audit "$first" first_opening
  matrix_mint forced-replacement "$ORD_WALLET_NAME_B" "$d" "$b" '[]' '{"usdb_main":"0x2222222222222222222222222222222222222222"}'
  regtest_v2_assert_audit "$MATRIX_PASS_ID" None "$b"
  regtest_assert_usdb_pass_snapshot_state "$first" "$V2_CONTEXT_HEIGHT" active
  response="$(regtest_get_pass_economic_profile_response "$first" "$V2_CONTEXT_HEIGHT")"
  regtest_assert_json_expr "$response" "data['result']['pass']['usdb_main']" "$MINER_PASS_USDB_MAIN"
  matrix_mint forced-consumption "$ORD_WALLET_NAME_B" "$d" "$b" "[\"$first\"]"
  regtest_v2_assert_audit "$MATRIX_PASS_ID" None "$b"
  regtest_assert_usdb_pass_snapshot_state "$first" "$V2_CONTEXT_HEIGHT" active

  # Transfer the only output away from D. Historical ownership must survive balance zero.
  regtest_ord_send_inscription "$ORD_WALLET_NAME" "$b" "$first" >/dev/null
  regtest_v2_sync "$MINING_ADDRESS"
  regtest_assert_usdb_pass_snapshot_state "$first" "$V2_CONTEXT_HEIGHT" dormant
  # Invalid gifted inscriptions also carry postage. Send those to B before the zero-balance check.
  local gift
  while read -r gift; do
    regtest_ord_send_inscription "$ORD_WALLET_NAME" "$b" "$gift" >/dev/null
    regtest_mine_blocks 1 "$MINING_ADDRESS"
    regtest_wait_until_ord_server_synced_to_bitcoind
  done < <(awk '$1 == "forced-replacement" || $1 == "forced-consumption" {print $2}' "$WORK_DIR/cases.tsv")
  matrix_mint reused-zero-owner "$ORD_WALLET_NAME_B" "$d" "$b" '[]'
  regtest_v2_assert_audit "$MATRIX_PASS_ID" None "$b"
  response="$(regtest_rpc_call_usdb_indexer get_pass_mint_audit "[{\"inscription_id\":\"$MATRIX_PASS_ID\"}]")"
  regtest_assert_json_expr "$response" "data['result']['audit']['balance_before_tx']" 0
  regtest_assert_json_expr "$response" "data['result']['audit']['ever_valid_owner']" True
  e="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  matrix_mint inherited-dormant "$ORD_WALLET_NAME_B" "$e" "$b" "[\"$first\"]"
  regtest_v2_assert_audit "$MATRIX_PASS_ID" cross_owner "$b"
  regtest_assert_usdb_pass_snapshot_state "$first" "$V2_CONTEXT_HEIGHT" consumed

  local l cf ca
  l="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  cf="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  ca="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  matrix_mint leader "$ORD_WALLET_NAME" "$l" "$a" '[]'
  leader="$MATRIX_PASS_ID"
  matrix_mint fixed-collab "$ORD_WALLET_NAME" "$cf" "$a" '[]' "{\"leader_pass_id\":\"$leader\"}"
  fixed="$MATRIX_PASS_ID"
  matrix_mint address-collab "$ORD_WALLET_NAME" "$ca" "$a" '[]' "{\"leader_btc_addr\":\"$l\"}"
  address_collab="$MATRIX_PASS_ID"
  regtest_fund_address "$l" 1.0
  regtest_fund_address "$cf" 1.0
  regtest_fund_address "$ca" 1.0
  regtest_v2_sync "$MINING_ADDRESS"
  response="$(regtest_get_pass_economic_profile_response "$leader" "$V2_CONTEXT_HEIGHT")"
  regtest_assert_json_expr "$response" "int(data['result']['pass']['collab_contribution']) > 0" True

  destination="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  matrix_mint leader-rotation "$ORD_WALLET_NAME" "$destination" "$l" "[\"$leader\"]"
  rotated="$MATRIX_PASS_ID"
  regtest_v2_assert_audit "$rotated" cross_owner "$l"
  response="$(regtest_get_pass_economic_profile_response "$rotated" "$V2_CONTEXT_HEIGHT")"
  regtest_assert_json_expr "$response" "data['result']['pass']['collab_contribution']" 0
  regtest_assert_usdb_pass_snapshot_state "$fixed" "$V2_CONTEXT_HEIGHT" active
  regtest_assert_usdb_pass_snapshot_state "$address_collab" "$V2_CONTEXT_HEIGHT" active
  response="$(regtest_rpc_call_usdb_indexer get_pass_mint_audit "[{\"inscription_id\":\"$rotated\"}]")"
  rotation_height="$(regtest_json_expr "$response" "data['result']['audit']['block_height']")"
  rotation_hash="$(regtest_json_expr "$response" "data['result']['audit']['block_hash']")"
  matrix_mint fixed-rebind "$ORD_WALLET_NAME" "$cf" "$cf" "[\"$fixed\"]" "{\"leader_pass_id\":\"$rotated\"}"
  fixed_new="$MATRIX_PASS_ID"
  matrix_mint address-rebind "$ORD_WALLET_NAME" "$ca" "$ca" "[\"$address_collab\"]" "{\"leader_btc_addr\":\"$destination\"}"
  address_new="$MATRIX_PASS_ID"
  regtest_fund_address "$cf" 1.0
  regtest_fund_address "$ca" 1.0
  regtest_v2_sync "$MINING_ADDRESS"
  response="$(regtest_get_pass_economic_profile_response "$rotated" "$V2_CONTEXT_HEIGHT")"
  regtest_assert_json_expr "$response" "int(data['result']['pass']['collab_contribution']) > 0" True

  # Remove the cross-owner mint and both rebindings, without re-mining orphaned mempool transactions.
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$rotation_hash"
  rollback_height=$((rotation_height - 1))
  for ((tip=0; tip<BTC_STABLE_LAG_BLOCKS; tip++)); do regtest_mine_empty_block "$MINING_ADDRESS"; done
  regtest_wait_until_balance_history_synced_eq "$rollback_height"
  regtest_wait_until_usdb_synced_eq "$rollback_height"
  regtest_assert_usdb_pass_snapshot_state "$leader" "$rollback_height" active
  regtest_assert_usdb_pass_snapshot_missing "$rotated" "$rollback_height"
  regtest_assert_usdb_pass_snapshot_state "$fixed" "$rollback_height" active
  regtest_assert_usdb_pass_snapshot_state "$address_collab" "$rollback_height" active
  regtest_assert_usdb_pass_snapshot_missing "$fixed_new" "$rollback_height"
  regtest_assert_usdb_pass_snapshot_missing "$address_new" "$rollback_height"
  regtest_finish_ord_reorg
  tip="$(regtest_get_bitcoin_tip_height)"
  V2_CONTEXT_HEIGHT=$((tip - BTC_STABLE_LAG_BLOCKS))
  matrix_snapshot "$WORK_DIR/after-reorg.json"

  regtest_crash_usdb_indexer
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$V2_CONTEXT_HEIGHT"
  regtest_wait_usdb_consensus_ready
  matrix_snapshot "$WORK_DIR/after-crash-reopen.json"
  cmp "$WORK_DIR/after-reorg.json" "$WORK_DIR/after-crash-reopen.json"
  regtest_stop_usdb_indexer
  USDB_INDEXER_ROOT="$WORK_DIR/fresh-replay-indexer"
  USDB_INDEXER_LOG_FILE="$WORK_DIR/fresh-replay-indexer.log"
  regtest_create_usdb_indexer_config
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$V2_CONTEXT_HEIGHT"
  regtest_wait_usdb_consensus_ready
  matrix_snapshot "$WORK_DIR/full-replay.json"
  cmp "$WORK_DIR/after-reorg.json" "$WORK_DIR/full-replay.json"
  regtest_log "V2 security/collaboration/reorg/crash/replay matrix passed: height=${V2_CONTEXT_HEIGHT}, artifacts=${WORK_DIR}"
}
main "$@"
