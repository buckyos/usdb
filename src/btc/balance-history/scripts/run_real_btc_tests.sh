#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
LOG_PREFIX="[balance-history-real-btc]"
SUITE="${1:-correctness}"
SIZE="small"
LIST_ONLY=0

declare -a SUITE_FILTERS=()

usage() {
  cat <<'USAGE'
Usage:
  USDB_BH_REAL_BTC=1 BTC_DATA_DIR=/path/to/bitcoin/datadir BTC_RPC_URL=http://127.0.0.1:8332 \
    bash src/btc/balance-history/scripts/run_real_btc_tests.sh [suite] [options]

Suites:
  correctness      Runs local blk/RPC correctness checks.
  loader-index     Builds a local-loader index from the configured subset and checks RPC parity.
  loader-restore   Checks persisted local-loader index restore and rebuild behavior.
  blk-reader       Compares blk record reader and block loader output.
  block-cache      Checks local block file cache correctness.
  latest-rpc       Checks latest complete blk file samples against RPC.
  profile          Runs manual local blk reader/cache profile checks.
  profile-reader   Runs blk reader memory profile only.
  profile-cache    Runs block file cache prefetch profile only.
  all              Runs correctness and profile.

Options:
  --size SIZE   Test data scale: tiny, small, medium, large, full. Default: small.
  --list        Print the cargo test filters that would run.
  -h, --help    Show this help.

Size mapping:
  tiny     correctness subset: 2 blk files, profile: 1 blk file
  small    correctness subset: 4 blk files,  profile: 4 blk files
  medium   correctness subset: 8 blk files,  profile: 16 blk files
  large    correctness subset: 16 blk files, profile: 64 blk files
  full     correctness subset: 32 blk files, profile: 256 blk files

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
  USDB_BH_REAL_BTC_PROFILE_START_FILE=0
  USDB_BH_REAL_BTC_PROFILE_FILE_COUNT=4
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

apply_size_defaults() {
  case "$SIZE" in
    tiny)
      : "${USDB_BH_REAL_BTC_SUBSET_FILE_COUNT:=2}"
      : "${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT:=1}"
      ;;
    small)
      : "${USDB_BH_REAL_BTC_SUBSET_FILE_COUNT:=4}"
      : "${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT:=4}"
      ;;
    medium)
      : "${USDB_BH_REAL_BTC_SUBSET_FILE_COUNT:=8}"
      : "${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT:=16}"
      ;;
    large)
      : "${USDB_BH_REAL_BTC_SUBSET_FILE_COUNT:=16}"
      : "${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT:=64}"
      ;;
    full)
      : "${USDB_BH_REAL_BTC_SUBSET_FILE_COUNT:=32}"
      : "${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT:=256}"
      ;;
    *)
      echo "Unknown size: ${SIZE}" >&2
      usage >&2
      exit 2
      ;;
  esac
  : "${USDB_BH_REAL_BTC_PROFILE_START_FILE:=0}"

  export USDB_BH_REAL_BTC_SUBSET_FILE_COUNT
  export USDB_BH_REAL_BTC_PROFILE_START_FILE
  export USDB_BH_REAL_BTC_PROFILE_FILE_COUNT
  export USDB_BH_REAL_BTC_CACHE_FILE_COUNT="${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT}"
  export USDB_BH_REAL_BTC_CACHE_START_FILE="${USDB_BH_REAL_BTC_PROFILE_START_FILE}"

  log "size=${SIZE}, subset_files=${USDB_BH_REAL_BTC_SUBSET_FILE_COUNT}, profile_start_file=${USDB_BH_REAL_BTC_PROFILE_START_FILE}, profile_files=${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT}"
}

select_suite() {
  case "$SUITE" in
    correctness)
      SUITE_FILTERS=(real_btc_correctness)
      ;;
    loader-index)
      SUITE_FILTERS=(real_btc_correctness_local_loader_build_index_matches_rpc_on_sample_heights)
      ;;
    loader-restore)
      SUITE_FILTERS=(
        real_btc_correctness_restore_block_index_from_db
        real_btc_correctness_build_index_rebuilds_after_corrupted_persisted_state
      )
      ;;
    blk-reader)
      SUITE_FILTERS=(real_btc_correctness_read_blk_blocks_matches_direct_reader_on_subset_files)
      ;;
    block-cache)
      SUITE_FILTERS=(real_btc_correctness_block_file_cache)
      ;;
    latest-rpc)
      SUITE_FILTERS=(real_btc_correctness_latest_complete_blk_file_blocks_are_available_via_rpc)
      ;;
    profile)
      SUITE_FILTERS=(real_btc_profile)
      ;;
    profile-reader)
      SUITE_FILTERS=(real_btc_profile_blk_file_reader_memory_usage)
      ;;
    profile-cache)
      SUITE_FILTERS=(real_btc_profile_block_file_cache_prefetch_sample_range)
      ;;
    all)
      SUITE_FILTERS=(real_btc_correctness real_btc_profile)
      ;;
    *)
      echo "Unknown real BTC test suite: ${SUITE}" >&2
      usage >&2
      exit 2
      ;;
  esac
}

parse_args() {
  if [[ $# -gt 0 && "$1" != -* ]]; then
    SUITE="$1"
    shift
  fi

  while [[ $# -gt 0 ]]; do
    case "$1" in
      --size)
        if [[ $# -lt 2 ]]; then
          echo "--size requires an argument." >&2
          exit 2
        fi
        SIZE="$2"
        shift
        ;;
      --size=*)
        SIZE="${1#--size=}"
        ;;
      --list)
        LIST_ONLY=1
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        echo "Unknown option: $1" >&2
        usage >&2
        exit 2
        ;;
    esac
    shift
  done
}

print_selected_filters() {
  log "suite=${SUITE}, size=${SIZE}, filters:"
  local filter
  for filter in "${SUITE_FILTERS[@]}"; do
    echo "  ${filter}"
  done
}

run_selected_suite() {
  local filter
  for filter in "${SUITE_FILTERS[@]}"; do
    run_cargo_filter "$filter"
  done
}

parse_args "$@"

require_env USDB_BH_REAL_BTC
if [[ "${USDB_BH_REAL_BTC}" != "1" ]]; then
  echo "USDB_BH_REAL_BTC must be set to 1." >&2
  exit 2
fi
require_env BTC_DATA_DIR
require_env BTC_RPC_URL

apply_size_defaults
select_suite

if [[ "$LIST_ONLY" == "1" ]]; then
  print_selected_filters
  exit 0
fi

run_selected_suite
