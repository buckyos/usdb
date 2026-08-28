#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-core-utxo-audit-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-31032}"
BTC_P2P_PORT="${BTC_P2P_PORT:-31033}"
BH_RPC_PORT="${BH_RPC_PORT:-31010}"
WALLET_NAME="${WALLET_NAME:-bhcoreutxoaudit}"
SAMPLE_ADDRESS_COUNT="${SAMPLE_ADDRESS_COUNT:-12}"
SAMPLE_SIZE="${SAMPLE_SIZE:-8}"
EXPECTED_STABLE_LAG="${EXPECTED_STABLE_LAG:-10}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
AUDIT_REPORT="${AUDIT_REPORT:-$WORK_DIR/bitcoin-core-utxo-audit.json}"
AUDIT_TOOL="${SCRIPT_DIR}/audit_bitcoin_core_utxo_sample.py"
REGTEST_LOG_PREFIX="[bitcoin-core-utxo-audit]"
export REPO_ROOT REGTEST_LOG_PREFIX

# shellcheck source=regtest_lib.sh
source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  if (( SAMPLE_ADDRESS_COUNT < SAMPLE_SIZE )); then
    regtest_log "SAMPLE_ADDRESS_COUNT must be at least SAMPLE_SIZE"
    exit 1
  fi

  regtest_ensure_workspace_dirs
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address recipients_json txid event_height stable_lag expected_stable_height
  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"

  recipients_json="$(
    for _ in $(seq 1 "$SAMPLE_ADDRESS_COUNT"); do
      printf '%s\n' "$(regtest_get_new_address)"
    done | python3 -c 'import json,sys; print(json.dumps({line.strip(): 0.1 for line in sys.stdin if line.strip()}))'
  )"
  txid="$(
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -rpcwallet="$WALLET_NAME" sendmany "" "$recipients_json"
  )"
  regtest_log "Created sample transaction txid=${txid} with ${SAMPLE_ADDRESS_COUNT} outputs"
  regtest_mine_blocks 1 "$mining_address"
  event_height="$(
    "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount
  )"

  # Mining exactly the registry lag makes event_height the service's stable frontier and
  # leaves the repeated mining script visibly touched in the excluded lag window.
  regtest_mine_blocks "$EXPECTED_STABLE_LAG" "$mining_address"

  regtest_create_balance_history_config
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$event_height"
  regtest_wait_balance_history_consensus_ready

  stable_lag="$(regtest_get_snapshot_stable_lag)"
  expected_stable_height="$(regtest_get_snapshot_stable_height)"
  if [[ "$stable_lag" != "$EXPECTED_STABLE_LAG" || "$expected_stable_height" != "$event_height" ]]; then
    regtest_log "Unexpected stable frontier: event=${event_height}, stable=${expected_stable_height}, lag=${stable_lag}"
    exit 1
  fi

  PYTHONDONTWRITEBYTECODE=1 python3 "$AUDIT_TOOL" \
    --bitcoin-rpc-url "http://127.0.0.1:${BTC_RPC_PORT}" \
    --bitcoin-cookie-file "${BITCOIN_DIR}/regtest/.cookie" \
    --balance-history-url "http://127.0.0.1:${BH_RPC_PORT}" \
    --expected-network regtest \
    --sample-size "$SAMPLE_SIZE" \
    --oversample-factor 4 \
    --source-lookback-blocks 16 \
    --source-block-count 16 \
    --max-gettxout-checks 32 \
    --seed 20260827 \
    --output "$AUDIT_REPORT"

  python3 - "$AUDIT_REPORT" "$SAMPLE_SIZE" "$EXPECTED_STABLE_LAG" <<'PY'
import json
import sys

report = json.load(open(sys.argv[1], encoding="utf-8"))
sample_size = int(sys.argv[2])
expected_stable_lag = int(sys.argv[3])
assert report["ok"] is True, report
assert report["stable_lag"] == expected_stable_lag, report
assert report["scan_height"] - report["stable_height"] == expected_stable_lag, report
assert report["lag_window_touched_candidate_count"] >= 1, report
assert report["verified_script_count"] == sample_size, report
assert report["gettxout_checked_count"] >= 1, report
assert report["mismatch_count"] == 0, report
PY

  regtest_log "Bitcoin Core UTXO audit passed; report=${AUDIT_REPORT}"
}

main "$@"
