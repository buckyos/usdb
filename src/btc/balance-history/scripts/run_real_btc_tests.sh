#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
LOG_PREFIX="[balance-history-real-btc]"
SUITE="${1:-correctness}"
SIZE="small"
LIST_ONLY=0
PROFILE_SEGMENT=""

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
  profile-early    Runs profile on the earliest blk files.
  profile-mid      Runs profile around the middle complete blk files.
  profile-recent   Runs profile on the latest complete blk files.
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
  USDB_BH_REAL_BTC_METRICS_FILE=/path/to/metrics.jsonl

Profile-only env:
  USDB_BH_REAL_BTC_PROFILE_START_FILE=0
  USDB_BH_REAL_BTC_PROFILE_FILE_COUNT=4
  USDB_BH_REAL_BTC_CACHE_SLEEP_MS=0
USAGE
}

log() {
  echo "${LOG_PREFIX} $*"
}

find_latest_blk_file_index() {
  local blocks_dir="${BTC_DATA_DIR%/}/blocks"
  local path base number max_index=-1

  if [[ ! -d "$blocks_dir" ]]; then
    echo "Missing BTC blocks dir: ${blocks_dir}" >&2
    exit 2
  fi

  shopt -s nullglob
  for path in "$blocks_dir"/blk*.dat; do
    base="$(basename "$path")"
    number="${base#blk}"
    number="${number%.dat}"
    if [[ "$number" =~ ^[0-9]+$ ]]; then
      number=$((10#$number))
      if (( number > max_index )); then
        max_index="$number"
      fi
    fi
  done
  shopt -u nullglob

  if (( max_index < 0 )); then
    echo "No blk*.dat files found under ${blocks_dir}" >&2
    exit 2
  fi

  echo "$max_index"
}

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "Missing required env: ${name}" >&2
    usage >&2
    exit 2
  fi
}

compute_profile_start_file() {
  local count="$1"
  local latest latest_complete start

  if [[ -n "${USDB_BH_REAL_BTC_PROFILE_START_FILE+x}" ]]; then
    echo "$USDB_BH_REAL_BTC_PROFILE_START_FILE"
    return 0
  fi

  case "$PROFILE_SEGMENT" in
    ""|early)
      echo 0
      ;;
    mid)
      latest="$(find_latest_blk_file_index)"
      latest_complete="$latest"
      if (( latest_complete > 0 )); then
        latest_complete=$((latest_complete - 1))
      fi
      start=$((latest_complete / 2 - count / 2))
      if (( start < 0 )); then
        start=0
      fi
      echo "$start"
      ;;
    recent)
      latest="$(find_latest_blk_file_index)"
      latest_complete="$latest"
      if (( latest_complete > 0 )); then
        latest_complete=$((latest_complete - 1))
      fi
      start=$((latest_complete - count + 1))
      if (( start < 0 )); then
        start=0
      fi
      echo "$start"
      ;;
    *)
      echo "Unknown profile segment: ${PROFILE_SEGMENT}" >&2
      exit 2
      ;;
  esac
}

append_runner_metric() {
  local event="$1"
  local filter="$2"
  local exit_code="$3"
  local started="$4"
  local ended="$5"

  if [[ -z "${USDB_BH_REAL_BTC_METRICS_FILE:-}" ]]; then
    return 0
  fi

  mkdir -p "$(dirname "$USDB_BH_REAL_BTC_METRICS_FILE")"
  python3 - "$USDB_BH_REAL_BTC_METRICS_FILE" "$event" "$filter" "$exit_code" "$started" "$ended" <<'PY'
import json
import os
import sys
from datetime import datetime, timezone

path, event, filter_name, exit_code, started, ended = sys.argv[1:]
started_i = int(started)
ended_i = int(ended)
record = {
    "component": "balance-history-real-btc-runner",
    "event": event,
    "timestamp_utc": datetime.now(timezone.utc).isoformat(),
    "suite": os.environ.get("USDB_BH_REAL_BTC_SUITE"),
    "size": os.environ.get("USDB_BH_REAL_BTC_SIZE"),
    "profile_segment": os.environ.get("USDB_BH_REAL_BTC_PROFILE_SEGMENT"),
    "filter": filter_name,
    "exit_code": int(exit_code),
    "started_unix_sec": started_i,
    "ended_unix_sec": ended_i,
    "duration_sec": ended_i - started_i,
    "subset_file_count": int(os.environ.get("USDB_BH_REAL_BTC_SUBSET_FILE_COUNT", "0")),
    "profile_start_file": int(os.environ.get("USDB_BH_REAL_BTC_PROFILE_START_FILE", "0")),
    "profile_file_count": int(os.environ.get("USDB_BH_REAL_BTC_PROFILE_FILE_COUNT", "0")),
    "btc_data_dir": os.environ.get("BTC_DATA_DIR"),
    "btc_rpc_url": os.environ.get("BTC_RPC_URL"),
}
with open(path, "a", encoding="utf-8") as f:
    f.write(json.dumps(record, sort_keys=True) + "\n")
PY
}

run_cargo_filter() {
  local filter="$1"
  local started ended exit_code
  log "Running cargo test filter '${filter}'"

  started="$(date +%s)"
  set +e
  (
    cd "$REPO_ROOT/src/btc"
    cargo test -p balance-history --lib "$filter" -- --nocapture --test-threads=1
  )
  exit_code=$?
  set -e
  ended="$(date +%s)"
  append_runner_metric "cargo_filter_completed" "$filter" "$exit_code" "$started" "$ended"
  return "$exit_code"
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
  USDB_BH_REAL_BTC_PROFILE_START_FILE="$(compute_profile_start_file "$USDB_BH_REAL_BTC_PROFILE_FILE_COUNT")"

  export USDB_BH_REAL_BTC_SUBSET_FILE_COUNT
  export USDB_BH_REAL_BTC_PROFILE_START_FILE
  export USDB_BH_REAL_BTC_PROFILE_FILE_COUNT
  export USDB_BH_REAL_BTC_CACHE_FILE_COUNT="${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT}"
  export USDB_BH_REAL_BTC_CACHE_START_FILE="${USDB_BH_REAL_BTC_PROFILE_START_FILE}"
  export USDB_BH_REAL_BTC_PROFILE_SEGMENT="${PROFILE_SEGMENT:-custom}"
  export USDB_BH_REAL_BTC_SUITE="$SUITE"
  export USDB_BH_REAL_BTC_SIZE="$SIZE"
  : "${USDB_BH_REAL_BTC_METRICS_FILE:=${REPO_ROOT}/target/balance-history-real-btc/metrics.jsonl}"
  export USDB_BH_REAL_BTC_METRICS_FILE

  log "size=${SIZE}, subset_files=${USDB_BH_REAL_BTC_SUBSET_FILE_COUNT}, profile_segment=${USDB_BH_REAL_BTC_PROFILE_SEGMENT}, profile_start_file=${USDB_BH_REAL_BTC_PROFILE_START_FILE}, profile_files=${USDB_BH_REAL_BTC_PROFILE_FILE_COUNT}, metrics_file=${USDB_BH_REAL_BTC_METRICS_FILE}"
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
    profile-early)
      PROFILE_SEGMENT="early"
      SUITE_FILTERS=(real_btc_profile)
      ;;
    profile-mid)
      PROFILE_SEGMENT="mid"
      SUITE_FILTERS=(real_btc_profile)
      ;;
    profile-recent)
      PROFILE_SEGMENT="recent"
      SUITE_FILTERS=(real_btc_profile)
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

select_suite
apply_size_defaults

if [[ "$LIST_ONLY" == "1" ]]; then
  print_selected_filters
  exit 0
fi

run_selected_suite
