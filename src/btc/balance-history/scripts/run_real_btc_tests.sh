#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
LOG_PREFIX="[balance-history-real-btc]"
SUITE="${1:-correctness}"

usage() {
  cat <<'USAGE'
Usage:
  USDB_BH_REAL_BTC=1 BTC_DATA_DIR=/path/to/bitcoin/datadir BTC_RPC_URL=http://127.0.0.1:8332 \
    bash src/btc/balance-history/scripts/run_real_btc_tests.sh [suite]

Suites:
  correctness  Runs local blk/RPC correctness checks only.
  profile      Runs manual local blk cache/profile checks.
  all          Runs correctness and profile.

Required env:
  USDB_BH_REAL_BTC=1
  BTC_DATA_DIR=/path/to/bitcoin/datadir
  BTC_RPC_URL=http://127.0.0.1:8332

Optional env:
  BTC_COOKIE_FILE=/path/to/.cookie
  BTC_RPC_USER=user
  BTC_RPC_PASSWORD=password
  BTC_NETWORK=bitcoin|testnet|regtest|signet|testnet4
  BTC_BLOCK_MAGIC=0xD9B4BEF9

Profile-only env:
  USDB_BH_REAL_BTC_CACHE_START_FILE=0
  USDB_BH_REAL_BTC_CACHE_FILE_COUNT=4
  USDB_BH_REAL_BTC_CACHE_SLEEP_MS=0
USAGE
}

log() {
  echo "${LOG_PREFIX} $*"
}

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "Missing required env: ${name}" >&2
    usage >&2
    exit 2
  fi
}

run_cargo_filter() {
  local filter="$1"
  log "Running cargo test filter '${filter}'"
  (
    cd "$REPO_ROOT/src/btc"
    cargo test -p balance-history --lib "$filter" -- --nocapture
  )
}

run_correctness() {
  run_cargo_filter real_btc_correctness
}

run_profile() {
  run_cargo_filter real_btc_profile
}

case "${1:-}" in
  -h|--help)
    usage
    exit 0
    ;;
esac

require_env USDB_BH_REAL_BTC
if [[ "${USDB_BH_REAL_BTC}" != "1" ]]; then
  echo "USDB_BH_REAL_BTC must be set to 1." >&2
  exit 2
fi
require_env BTC_DATA_DIR
require_env BTC_RPC_URL

case "$SUITE" in
  correctness)
    run_correctness
    ;;
  profile)
    run_profile
    ;;
  all)
    run_correctness
    run_profile
    ;;
  *)
    echo "Unknown real BTC test suite: ${SUITE}" >&2
    usage >&2
    exit 2
    ;;
esac
