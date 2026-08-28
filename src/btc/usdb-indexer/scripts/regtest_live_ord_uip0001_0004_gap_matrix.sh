#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-live-ord-uip0001-0004-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
ORD_DATA_DIR="${ORD_DATA_DIR:-$WORK_DIR/ord}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BITCOIND_INDEXER_ROOT="${BITCOIND_INDEXER_ROOT:-$WORK_DIR/usdb-indexer-bitcoind}"
ORD_INDEXER_ROOT="${ORD_INDEXER_ROOT:-$WORK_DIR/usdb-indexer-ord}"
USDB_INDEXER_ROOT="$BITCOIND_INDEXER_ROOT"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"
BTC_RPC_PORT="${BTC_RPC_PORT:-31432}"
BTC_P2P_PORT="${BTC_P2P_PORT:-31433}"
BH_RPC_PORT="${BH_RPC_PORT:-31410}"
USDB_INDEXER_RPC_PORT="${USDB_INDEXER_RPC_PORT:-31420}"
ORD_SOURCE_INDEXER_RPC_PORT="${ORD_SOURCE_INDEXER_RPC_PORT:-31421}"
ORD_RPC_PORT="${ORD_RPC_PORT:-31430}"
WALLET_NAME="${WALLET_NAME:-usdbuipgapmatrix}"
ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-uip-gap-a}"
ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-uip-gap-b}"
PREMINE_BLOCKS="${PREMINE_BLOCKS:-130}"
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
INSCRIBE_CONFIRM_BLOCKS="${INSCRIBE_CONFIRM_BLOCKS:-1}"
TRANSFER_CONFIRM_BLOCKS="${TRANSFER_CONFIRM_BLOCKS:-1}"
BURN_CONFIRM_BLOCKS="${BURN_CONFIRM_BLOCKS:-1}"
BTC_STABLE_LAG_BLOCKS="${BTC_STABLE_LAG_BLOCKS:-10}"
ORD_FUNDING_UTXO_AMOUNT_BTC="${ORD_FUNDING_UTXO_AMOUNT_BTC:-0.25}"
ORD_WALLET_A_FUNDING_UTXOS="${ORD_WALLET_A_FUNDING_UTXOS:-12}"
ORD_WALLET_B_FUNDING_UTXOS="${ORD_WALLET_B_FUNDING_UTXOS:-4}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-300}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
USDB_INDEXER_LOG_FILE="${USDB_INDEXER_LOG_FILE:-$WORK_DIR/usdb-indexer-bitcoind.log}"
ORD_SERVER_LOG_FILE="${ORD_SERVER_LOG_FILE:-$WORK_DIR/ord-server.log}"
REGTEST_LOG_PREFIX="[usdb-uip0001-0004-live-gap]"
export REGTEST_LOG_PREFIX

# shellcheck source=src/btc/usdb-indexer/scripts/regtest_reorg_lib.sh
source "${SCRIPT_DIR}/regtest_reorg_lib.sh"

current_btc_height() {
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount
}

mine_and_sync_ord() {
  local block_count="$1"
  local mining_address="$2"

  regtest_mine_blocks "$block_count" "$mining_address"
  regtest_wait_until_ord_server_synced_to_bitcoind
}

assert_ord_content_types() {
  local json_pass_id="$1"
  local text_pass_id="$2"

  python3 - "http://127.0.0.1:${ORD_RPC_PORT}" "$json_pass_id" "$text_pass_id" <<'PY'
import json
import sys
import urllib.request

ord_url, json_pass_id, text_pass_id = sys.argv[1:]
request = urllib.request.Request(
    ord_url + "/inscriptions",
    data=json.dumps([json_pass_id, text_pass_id]).encode(),
    headers={
        "accept": "application/json",
        "content-type": "application/json",
    },
    method="POST",
)
with urllib.request.urlopen(request, timeout=30) as response:
    items = json.load(response)

by_id = {item["id"]: item for item in items}
if set(by_id) != {json_pass_id, text_pass_id}:
    raise SystemExit(f"ord content-type response ids mismatch: {sorted(by_id)}")

json_type = (
    by_id[json_pass_id].get("content_type")
    or by_id[json_pass_id].get("effective_content_type")
    or ""
).lower()
text_type = (
    by_id[text_pass_id].get("content_type")
    or by_id[text_pass_id].get("effective_content_type")
    or ""
).lower()
if not json_type.startswith("application/json"):
    raise SystemExit(f"expected application/json inscription, got {json_type!r}")
if not text_type.startswith("text/plain"):
    raise SystemExit(f"expected text/plain inscription, got {text_type!r}")
print(
    "ord content types verified: "
    f"{json_pass_id}={json_type}, {text_pass_id}={text_type}"
)
PY
}

run_source_compare() {
  local config_root="$1"
  local start_height="$2"
  local end_height="$3"
  local compare_target

  for compare_target in raw usdb; do
    regtest_log "Comparing ord and bitcoind sources: target=${compare_target}, range=${start_height}..${end_height}"
    (
      cd "$REPO_ROOT"
      USDB_COMPARE_CONFIG_ROOT="$config_root" \
        USDB_COMPARE_START_HEIGHT="$start_height" \
        USDB_COMPARE_END_HEIGHT="$end_height" \
        USDB_COMPARE_PROGRESS_EVERY=1 \
        USDB_COMPARE_FAIL_FAST=1 \
        USDB_COMPARE_TARGET="$compare_target" \
        cargo test --manifest-path src/btc/Cargo.toml -p usdb-indexer \
          inscription::test::source_compare_live::test_compare_ord_and_bitcoind_on_height_range \
          -- --ignored --exact --nocapture
    )
  done
}

verify_and_capture_indexer() {
  local output_path="$1"
  local stable_height="$2"
  local json_pass_id="$3"
  local text_pass_id="$4"
  local invalid_missing_id="$5"
  local invalid_state_id="$6"
  local invalid_owner_id="$7"
  local invalid_duplicate_id="$8"
  local successor_id="$9"
  local invalid_consumed_id="${10}"
  local invalid_burned_id="${11}"
  local multi_prev_1_id="${12}"
  local multi_prev_2_id="${13}"
  local multi_child_id="${14}"
  local height_missing="${15}"
  local height_owner="${16}"
  local height_duplicate="${17}"
  local height_consume="${18}"
  local height_consumed_invalid="${19}"
  local height_successor_burn="${20}"
  local height_text_transfer="${21}"
  local height_text_burn="${22}"
  local height_consumed_burn="${23}"
  local height_multi_1_transfer="${24}"
  local height_multi_2_transfer="${25}"

  python3 - \
    "http://127.0.0.1:${USDB_INDEXER_RPC_PORT}" \
    "$output_path" \
    "$stable_height" \
    "$BTC_STABLE_LAG_BLOCKS" \
    "$json_pass_id" \
    "$text_pass_id" \
    "$invalid_missing_id" \
    "$invalid_state_id" \
    "$invalid_owner_id" \
    "$invalid_duplicate_id" \
    "$successor_id" \
    "$invalid_consumed_id" \
    "$invalid_burned_id" \
    "$multi_prev_1_id" \
    "$multi_prev_2_id" \
    "$multi_child_id" \
    "$height_missing" \
    "$height_owner" \
    "$height_duplicate" \
    "$height_consume" \
    "$height_consumed_invalid" \
    "$height_successor_burn" \
    "$height_text_transfer" \
    "$height_text_burn" \
    "$height_consumed_burn" \
    "$height_multi_1_transfer" \
    "$height_multi_2_transfer" <<'PY'
import json
import pathlib
import sys
import urllib.request

(
    rpc_url,
    output_path,
    stable_height_text,
    stable_lag_text,
    json_pass_id,
    text_pass_id,
    invalid_missing_id,
    invalid_state_id,
    invalid_owner_id,
    invalid_duplicate_id,
    successor_id,
    invalid_consumed_id,
    invalid_burned_id,
    multi_prev_1_id,
    multi_prev_2_id,
    multi_child_id,
    height_missing_text,
    height_owner_text,
    height_duplicate_text,
    height_consume_text,
    height_consumed_invalid_text,
    height_successor_burn_text,
    height_text_transfer_text,
    height_text_burn_text,
    height_consumed_burn_text,
    height_multi_1_transfer_text,
    height_multi_2_transfer_text,
) = sys.argv[1:]

stable_height = int(stable_height_text)
stable_lag = int(stable_lag_text)
height_missing = int(height_missing_text)
height_owner = int(height_owner_text)
height_duplicate = int(height_duplicate_text)
height_consume = int(height_consume_text)
height_consumed_invalid = int(height_consumed_invalid_text)
height_successor_burn = int(height_successor_burn_text)
height_text_transfer = int(height_text_transfer_text)
height_text_burn = int(height_text_burn_text)
height_consumed_burn = int(height_consumed_burn_text)
height_multi_1_transfer = int(height_multi_1_transfer_text)
height_multi_2_transfer = int(height_multi_2_transfer_text)


def rpc(method, params=None):
    payload = json.dumps(
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": method,
            "params": [] if params is None else params,
        }
    ).encode()
    request = urllib.request.Request(
        rpc_url,
        data=payload,
        headers={"content-type": "application/json"},
        method="POST",
    )
    with urllib.request.urlopen(request, timeout=30) as response:
        envelope = json.load(response)
    if envelope.get("error") is not None:
        raise SystemExit(
            f"{method} failed: {json.dumps(envelope['error'], sort_keys=True)}"
        )
    return envelope.get("result")


def pass_snapshot(pass_id, height):
    result = rpc(
        "get_pass_snapshot",
        [{"inscription_id": pass_id, "at_height": height}],
    )
    if result is None:
        raise SystemExit(f"missing pass snapshot: pass_id={pass_id}, height={height}")
    return result


def energy(pass_id, height):
    result = rpc(
        "get_pass_energy",
        [
            {
                "inscription_id": pass_id,
                "block_height": height,
                "mode": "at_or_before",
            }
        ],
    )
    if result is None:
        raise SystemExit(f"missing pass energy: pass_id={pass_id}, height={height}")
    return result


def require_state(pass_id, height, expected):
    actual = pass_snapshot(pass_id, height)["state"]
    if actual != expected:
        raise SystemExit(
            f"pass state mismatch: pass_id={pass_id}, height={height}, "
            f"have={actual}, want={expected}"
        )


for height in (height_missing, height_owner, height_duplicate):
    require_state(json_pass_id, height, "active")
require_state(json_pass_id, height_consume, "consumed")
require_state(successor_id, height_consume, "active")
require_state(successor_id, height_consumed_invalid, "active")
require_state(successor_id, height_successor_burn, "burned")
require_state(text_pass_id, height_text_transfer, "dormant")
require_state(text_pass_id, height_text_burn, "burned")
require_state(json_pass_id, height_consumed_burn, "consumed")

terminal_expectations = {
    json_pass_id: "consumed",
    text_pass_id: "burned",
    successor_id: "burned",
    multi_prev_1_id: "consumed",
    multi_prev_2_id: "consumed",
}
for pass_id, expected_state in terminal_expectations.items():
    snapshot = pass_snapshot(pass_id, stable_height)
    pass_energy = energy(pass_id, stable_height)
    if snapshot["state"] != expected_state or pass_energy["state"] != expected_state:
        raise SystemExit(
            f"terminal state mismatch for {pass_id}: "
            f"snapshot={snapshot['state']}, energy={pass_energy['state']}, "
            f"want={expected_state}"
        )
    if int(pass_energy["raw_energy"]) != 0:
        raise SystemExit(f"terminal pass retained raw energy: {pass_id}")

multi_1_frozen = energy(multi_prev_1_id, height_multi_1_transfer)
multi_2_frozen = energy(multi_prev_2_id, height_multi_2_transfer)
if multi_1_frozen["state"] != "dormant" or multi_2_frozen["state"] != "dormant":
    raise SystemExit("multi-prev sources must be dormant before inheritance")
multi_child = energy(multi_child_id, stable_height)
if multi_child["state"] != "active":
    raise SystemExit("multi-prev child is not active")
expected_inherited = (
    int(multi_1_frozen["raw_energy"]) * 9500 // 10000
    + int(multi_2_frozen["raw_energy"]) * 9500 // 10000
)
if int(multi_child["raw_energy"]) != expected_inherited:
    raise SystemExit(
        "multi-prev inheritance mismatch: "
        f"have={multi_child['raw_energy']}, want={expected_inherited}"
    )

invalid_ids = {
    invalid_missing_id: "not found",
    invalid_state_id: "state invalid",
    invalid_owner_id: "does not match",
    invalid_duplicate_id: "duplicate prev",
    invalid_consumed_id: "state consumed",
    invalid_burned_id: "state burned",
}
invalid_page = rpc(
    "get_invalid_passes",
    [
        {
            "from_height": 1,
            "to_height": stable_height,
            "page": 0,
            "page_size": 50,
        }
    ],
)
invalid_items = invalid_page.get("items") or []
if int(invalid_page.get("total", len(invalid_items))) != len(invalid_ids):
    raise SystemExit(
        f"invalid pass total mismatch: page={json.dumps(invalid_page, sort_keys=True)}"
    )
by_id = {item["inscription_id"]: item for item in invalid_items}
if set(by_id) != set(invalid_ids):
    raise SystemExit(
        f"invalid pass ids mismatch: have={sorted(by_id)}, want={sorted(invalid_ids)}"
    )
for pass_id, reason_fragment in invalid_ids.items():
    item = by_id[pass_id]
    if item["state"] != "invalid" or item["invalid_code"] != "INVALID_PREV_ID":
        raise SystemExit(f"invalid prev classification mismatch: {item}")
    if reason_fragment not in (item.get("invalid_reason") or "").lower():
        raise SystemExit(
            f"invalid prev reason mismatch: pass_id={pass_id}, item={item}"
        )

candidate = rpc(
    "get_candidate_set_view",
    [
        {
            "view_version": "uip-0006-usdb-economic-state-view:v1",
            "selection_rule": "uip-0006:effective-energy-desc-pass-id-asc:v1",
            "block_height": stable_height,
            "context": None,
            "cursor": None,
            "limit": 20,
        }
    ],
)
candidate_items = candidate.get("items") or []
if int(candidate.get("total", len(candidate_items))) != 1 or len(candidate_items) != 1:
    raise SystemExit(f"candidate set must contain only multi-prev child: {candidate}")
if candidate_items[0]["pass_id"] != multi_child_id:
    raise SystemExit(f"unexpected final candidate: {candidate_items[0]}")

all_ids = [
    json_pass_id,
    text_pass_id,
    invalid_missing_id,
    invalid_state_id,
    invalid_owner_id,
    invalid_duplicate_id,
    successor_id,
    invalid_consumed_id,
    invalid_burned_id,
    multi_prev_1_id,
    multi_prev_2_id,
    multi_child_id,
]
final_passes = {
    pass_id: pass_snapshot(pass_id, stable_height) for pass_id in all_ids
}
final_energies = {
    pass_id: energy(pass_id, stable_height)
    for pass_id in (
        json_pass_id,
        text_pass_id,
        successor_id,
        multi_prev_1_id,
        multi_prev_2_id,
        multi_child_id,
    )
}
snapshot_info = rpc("get_snapshot_info")
if snapshot_info["consensus_identity"]["stable_lag"] != stable_lag:
    raise SystemExit(
        "stable lag mismatch: "
        f"have={snapshot_info['consensus_identity']['stable_lag']}, want={stable_lag}"
    )
if snapshot_info["balance_history_stable_height"] != stable_height:
    raise SystemExit(
        "stable height mismatch: "
        f"have={snapshot_info['balance_history_stable_height']}, want={stable_height}"
    )

captured = {
    "stable_height": stable_height,
    "snapshot_info": snapshot_info,
    "pass_block_commit": rpc(
        "get_pass_block_commit", [{"block_height": stable_height}]
    ),
    "local_state_commit_info": rpc("get_local_state_commit_info"),
    "system_state_info": rpc("get_system_state_info"),
    "passes": final_passes,
    "energies": final_energies,
    "invalid_page": invalid_page,
    "candidate_set": candidate,
    "multi_prev_frozen_energy": {
        multi_prev_1_id: multi_1_frozen,
        multi_prev_2_id: multi_2_frozen,
    },
}
pathlib.Path(output_path).write_text(
    json.dumps(captured, indent=2, sort_keys=True) + "\n",
    encoding="utf-8",
)
print(
    "indexer view verified: "
    f"url={rpc_url}, stable_height={stable_height}, "
    f"system_state_id={captured['system_state_info']['system_state_id']}"
)
PY
}

run_indexer_source() {
  local source_name="$1"
  local root_dir="$2"
  local rpc_port="$3"
  local log_file="$4"
  local output_file="$5"
  shift 5

  INSCRIPTION_SOURCE="$source_name"
  USDB_INDEXER_ROOT="$root_dir"
  USDB_INDEXER_RPC_PORT="$rpc_port"
  USDB_INDEXER_LOG_FILE="$log_file"
  export INSCRIPTION_SOURCE USDB_INDEXER_ROOT USDB_INDEXER_RPC_PORT USDB_INDEXER_LOG_FILE
  regtest_create_usdb_indexer_config
  regtest_start_usdb_indexer
  regtest_wait_usdb_rpc_ready
  regtest_wait_until_usdb_synced_eq "$1"
  regtest_wait_usdb_consensus_ready
  verify_and_capture_indexer "$output_file" "$@"
  regtest_stop_usdb_indexer
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  if [[ ! -x "$ORD_BIN" ]]; then
    echo "Missing required ORD_BIN executable: $ORD_BIN" >&2
    exit 1
  fi
  regtest_require_cmd cargo
  regtest_require_cmd cmp
  regtest_require_cmd curl
  regtest_require_cmd diff
  regtest_require_cmd python3
  regtest_assert_ord_server_port_available
  if [[ ! "$BTC_STABLE_LAG_BLOCKS" =~ ^[0-9]+$ ]]; then
    echo "BTC_STABLE_LAG_BLOCKS must be a non-negative integer" >&2
    exit 1
  fi
  if [[ ! "$ORD_WALLET_A_FUNDING_UTXOS" =~ ^[1-9][0-9]*$ ]] ||
    [[ ! "$ORD_WALLET_B_FUNDING_UTXOS" =~ ^[1-9][0-9]*$ ]]; then
    echo "ORD wallet funding UTXO counts must be positive integers" >&2
    exit 1
  fi

  regtest_ensure_workspace_dirs
  mkdir -p "$BITCOIND_INDEXER_ROOT" "$ORD_INDEXER_ROOT"
  regtest_start_bitcoind
  regtest_ensure_wallet

  local miner_address address_a address_b
  local json_mint_file text_mint_file
  local json_pass_id text_pass_id
  local invalid_missing_id invalid_state_id invalid_owner_id invalid_duplicate_id
  local successor_id invalid_consumed_id invalid_burned_id
  local multi_prev_1_id multi_prev_2_id multi_child_id
  local height_json_mint height_missing height_owner height_duplicate
  local height_consume height_consumed_invalid height_successor_burn
  local height_text_transfer height_text_burn height_consumed_burn
  local height_multi_1_transfer height_multi_2_transfer stable_height tip_height
  local missing_prev_id burn_txid transfer_txid
  local bitcoind_view_file ord_view_file

  miner_address="$(regtest_get_new_address)"
  regtest_mine_blocks "$PREMINE_BLOCKS" "$miner_address"

  regtest_start_ord_server
  regtest_wait_until_ord_server_synced_to_bitcoind
  regtest_prepare_ord_wallets
  address_a="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME")"
  address_b="$(regtest_get_ord_wallet_receive_address "$ORD_WALLET_NAME_B")"
  local funding_index
  for ((funding_index = 0; funding_index < ORD_WALLET_A_FUNDING_UTXOS; funding_index++)); do
    regtest_fund_address "$address_a" "$ORD_FUNDING_UTXO_AMOUNT_BTC"
  done
  for ((funding_index = 0; funding_index < ORD_WALLET_B_FUNDING_UTXOS; funding_index++)); do
    regtest_fund_address "$address_b" "$ORD_FUNDING_UTXO_AMOUNT_BTC"
  done
  mine_and_sync_ord "$FUND_CONFIRM_BLOCKS" "$miner_address"

  json_mint_file="$WORK_DIR/standard-json.json"
  cat >"$json_mint_file" <<'EOF'
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x1111111111111111111111111111111111111111","prev":[]}
EOF
  json_pass_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$json_mint_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  height_json_mint="$(current_btc_height)"

  text_mint_file="$WORK_DIR/standard-text.txt"
  cat >"$text_mint_file" <<'EOF'
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x2222222222222222222222222222222222222222","prev":[]}
EOF
  text_pass_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$text_mint_file" "$address_b")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  assert_ord_content_types "$json_pass_id" "$text_pass_id"

  missing_prev_id="$(printf '0%.0s' {1..64})i0"
  local missing_file="$WORK_DIR/invalid-prev-missing.json"
  cat >"$missing_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x3333333333333333333333333333333333333333","prev":["${missing_prev_id}"]}
EOF
  invalid_missing_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$missing_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  height_missing="$(current_btc_height)"

  local invalid_state_file="$WORK_DIR/invalid-prev-state-invalid.json"
  cat >"$invalid_state_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x3434343434343434343434343434343434343434","prev":["${invalid_missing_id}"]}
EOF
  invalid_state_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$invalid_state_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"

  local owner_file="$WORK_DIR/invalid-prev-owner.json"
  cat >"$owner_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x4444444444444444444444444444444444444444","prev":["${text_pass_id}"]}
EOF
  invalid_owner_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$owner_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  height_owner="$(current_btc_height)"

  local duplicate_file="$WORK_DIR/invalid-prev-duplicate.json"
  cat >"$duplicate_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x5555555555555555555555555555555555555555","prev":["${json_pass_id}","${json_pass_id}"]}
EOF
  invalid_duplicate_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$duplicate_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  height_duplicate="$(current_btc_height)"

  local successor_file="$WORK_DIR/valid-prev-successor.json"
  cat >"$successor_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x6666666666666666666666666666666666666666","prev":["${json_pass_id}"]}
EOF
  successor_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$successor_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  height_consume="$(current_btc_height)"

  local consumed_file="$WORK_DIR/invalid-prev-consumed.json"
  cat >"$consumed_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x7777777777777777777777777777777777777777","prev":["${json_pass_id}"]}
EOF
  invalid_consumed_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$consumed_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  height_consumed_invalid="$(current_btc_height)"

  burn_txid="$(regtest_ord_burn_inscription "$ORD_WALLET_NAME" "$successor_id")"
  regtest_log "Active successor burn txid=${burn_txid}"
  mine_and_sync_ord "$BURN_CONFIRM_BLOCKS" "$miner_address"
  height_successor_burn="$(current_btc_height)"

  local burned_file="$WORK_DIR/invalid-prev-burned.json"
  cat >"$burned_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x8888888888888888888888888888888888888888","prev":["${successor_id}"]}
EOF
  invalid_burned_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$burned_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"

  transfer_txid="$(regtest_ord_send_inscription "$ORD_WALLET_NAME_B" "$address_a" "$text_pass_id")"
  regtest_log "Dormant-pass transfer txid=${transfer_txid}"
  mine_and_sync_ord "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME" "$text_pass_id"
  height_text_transfer="$(current_btc_height)"

  burn_txid="$(regtest_ord_burn_inscription "$ORD_WALLET_NAME" "$text_pass_id")"
  regtest_log "Dormant pass burn txid=${burn_txid}"
  mine_and_sync_ord "$BURN_CONFIRM_BLOCKS" "$miner_address"
  height_text_burn="$(current_btc_height)"

  burn_txid="$(regtest_ord_burn_inscription "$ORD_WALLET_NAME" "$json_pass_id")"
  regtest_log "Consumed pass physical burn txid=${burn_txid}"
  mine_and_sync_ord "$BURN_CONFIRM_BLOCKS" "$miner_address"
  height_consumed_burn="$(current_btc_height)"

  local multi_1_file="$WORK_DIR/multi-prev-1.json"
  cat >"$multi_1_file" <<'EOF'
{"p":"usdb","op":"mint","v":1,"usdb_main":"0x9999999999999999999999999999999999999999","prev":[]}
EOF
  multi_prev_1_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$multi_1_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  transfer_txid="$(regtest_ord_send_inscription "$ORD_WALLET_NAME" "$address_b" "$multi_prev_1_id")"
  regtest_log "First multi-prev source transfer txid=${transfer_txid}"
  mine_and_sync_ord "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$multi_prev_1_id"
  height_multi_1_transfer="$(current_btc_height)"

  local multi_2_file="$WORK_DIR/multi-prev-2.json"
  cat >"$multi_2_file" <<'EOF'
{"p":"usdb","op":"mint","v":1,"usdb_main":"0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","prev":[]}
EOF
  multi_prev_2_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME" "$multi_2_file" "$address_a")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  transfer_txid="$(regtest_ord_send_inscription "$ORD_WALLET_NAME" "$address_b" "$multi_prev_2_id")"
  regtest_log "Second multi-prev source transfer txid=${transfer_txid}"
  mine_and_sync_ord "$TRANSFER_CONFIRM_BLOCKS" "$miner_address"
  regtest_wait_until_ord_wallet_has_inscription "$ORD_WALLET_NAME_B" "$multi_prev_2_id"
  height_multi_2_transfer="$(current_btc_height)"

  local multi_child_file="$WORK_DIR/multi-prev-child.json"
  cat >"$multi_child_file" <<EOF
{"p":"usdb","op":"mint","v":1,"usdb_main":"0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","prev":["${multi_prev_1_id}","${multi_prev_2_id}"]}
EOF
  multi_child_id="$(regtest_ord_inscribe_file "$ORD_WALLET_NAME_B" "$multi_child_file" "$address_b")"
  mine_and_sync_ord "$INSCRIBE_CONFIRM_BLOCKS" "$miner_address"
  stable_height="$(current_btc_height)"

  regtest_log "Mining ${BTC_STABLE_LAG_BLOCKS} blocks so event height ${stable_height} reaches the stable frontier"
  mine_and_sync_ord "$BTC_STABLE_LAG_BLOCKS" "$miner_address"
  tip_height="$(current_btc_height)"
  if (( tip_height - stable_height != BTC_STABLE_LAG_BLOCKS )); then
    regtest_log "Unexpected stable-lag layout: tip=${tip_height}, event_height=${stable_height}, lag=${BTC_STABLE_LAG_BLOCKS}"
    exit 1
  fi

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_balance_history_synced_eq "$stable_height"
  regtest_wait_balance_history_consensus_ready

  INSCRIPTION_SOURCE="bitcoind"
  USDB_INDEXER_ROOT="$BITCOIND_INDEXER_ROOT"
  USDB_INDEXER_LOG_FILE="$WORK_DIR/usdb-indexer-bitcoind.log"
  export INSCRIPTION_SOURCE USDB_INDEXER_ROOT USDB_INDEXER_LOG_FILE
  regtest_create_usdb_indexer_config
  run_source_compare "$BITCOIND_INDEXER_ROOT" "$height_json_mint" "$stable_height"

  bitcoind_view_file="$WORK_DIR/bitcoind-indexer-view.json"
  run_indexer_source \
    bitcoind \
    "$BITCOIND_INDEXER_ROOT" \
    "$USDB_INDEXER_RPC_PORT" \
    "$WORK_DIR/usdb-indexer-bitcoind.log" \
    "$bitcoind_view_file" \
    "$stable_height" \
    "$json_pass_id" \
    "$text_pass_id" \
    "$invalid_missing_id" \
    "$invalid_state_id" \
    "$invalid_owner_id" \
    "$invalid_duplicate_id" \
    "$successor_id" \
    "$invalid_consumed_id" \
    "$invalid_burned_id" \
    "$multi_prev_1_id" \
    "$multi_prev_2_id" \
    "$multi_child_id" \
    "$height_missing" \
    "$height_owner" \
    "$height_duplicate" \
    "$height_consume" \
    "$height_consumed_invalid" \
    "$height_successor_burn" \
    "$height_text_transfer" \
    "$height_text_burn" \
    "$height_consumed_burn" \
    "$height_multi_1_transfer" \
    "$height_multi_2_transfer"

  ord_view_file="$WORK_DIR/ord-indexer-view.json"
  run_indexer_source \
    ord \
    "$ORD_INDEXER_ROOT" \
    "$ORD_SOURCE_INDEXER_RPC_PORT" \
    "$WORK_DIR/usdb-indexer-ord.log" \
    "$ord_view_file" \
    "$stable_height" \
    "$json_pass_id" \
    "$text_pass_id" \
    "$invalid_missing_id" \
    "$invalid_state_id" \
    "$invalid_owner_id" \
    "$invalid_duplicate_id" \
    "$successor_id" \
    "$invalid_consumed_id" \
    "$invalid_burned_id" \
    "$multi_prev_1_id" \
    "$multi_prev_2_id" \
    "$multi_child_id" \
    "$height_missing" \
    "$height_owner" \
    "$height_duplicate" \
    "$height_consume" \
    "$height_consumed_invalid" \
    "$height_successor_burn" \
    "$height_text_transfer" \
    "$height_text_burn" \
    "$height_consumed_burn" \
    "$height_multi_1_transfer" \
    "$height_multi_2_transfer"

  if ! cmp -s "$bitcoind_view_file" "$ord_view_file"; then
    regtest_log "bitcoind and ord indexers produced different canonical views"
    diff -u "$bitcoind_view_file" "$ord_view_file" || true
    exit 1
  fi

  regtest_log "UIP0001-0004 live gap matrix succeeded."
  regtest_log "stable_height=${stable_height}, tip_height=${tip_height}, json_pass=${json_pass_id}, text_pass=${text_pass_id}, multi_child=${multi_child_id}"
  regtest_log "Canonical source views are identical: ${bitcoind_view_file} == ${ord_view_file}"
}

main "$@"
