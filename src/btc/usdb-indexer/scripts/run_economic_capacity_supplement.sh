#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
MANIFEST_PATH="${REPO_ROOT}/src/btc/Cargo.toml"
OUTPUT_DIR="${USDB_ECONOMIC_CAPACITY_OUTPUT_DIR:-${REPO_ROOT}/src/btc/target/economic-capacity}"
SIZE="${USDB_ECONOMIC_CAPACITY_SIZE:-10000}"
LEADER_COUNT="${USDB_ECONOMIC_CAPACITY_LEADER_COUNT:-32}"
CHURN_COUNT="${USDB_ECONOMIC_CAPACITY_CHURN_COUNT:-5000}"
PAGE_LIMIT="${USDB_ECONOMIC_CAPACITY_PAGE_LIMIT:-500}"
COLD_START_CLIENTS="${USDB_ECONOMIC_CAPACITY_COLD_START_CLIENTS:-8}"

require_positive_integer() {
    local name="$1"
    local value="$2"
    if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
        echo "${name} must be a positive integer, have: ${value}" >&2
        exit 2
    fi
}

require_positive_integer USDB_ECONOMIC_CAPACITY_SIZE "$SIZE"
require_positive_integer USDB_ECONOMIC_CAPACITY_LEADER_COUNT "$LEADER_COUNT"
require_positive_integer USDB_ECONOMIC_CAPACITY_CHURN_COUNT "$CHURN_COUNT"
require_positive_integer USDB_ECONOMIC_CAPACITY_PAGE_LIMIT "$PAGE_LIMIT"
require_positive_integer USDB_ECONOMIC_CAPACITY_COLD_START_CLIENTS "$COLD_START_CLIENTS"
if ((LEADER_COUNT >= SIZE)); then
    echo "leader count ${LEADER_COUNT} must be smaller than pass count ${SIZE}" >&2
    exit 2
fi
if ((CHURN_COUNT > SIZE - LEADER_COUNT)); then
    echo "churn count ${CHURN_COUNT} exceeds non-Leader standard capacity $((SIZE - LEADER_COUNT))" >&2
    exit 2
fi
if ((PAGE_LIMIT > 500)); then
    echo "page limit ${PAGE_LIMIT} exceeds RPC max limit 500" >&2
    exit 2
fi
if ((COLD_START_CLIENTS < 2)); then
    echo "cold-start client count must be at least 2" >&2
    exit 2
fi

mkdir -p "$OUTPUT_DIR"
REPORT_FILE="${OUTPUT_DIR}/economic-capacity-${SIZE}-leaders-${LEADER_COUNT}-churn-${CHURN_COUNT}-limit-${PAGE_LIMIT}-clients-${COLD_START_CLIENTS}.json"

echo "running economic capacity supplement: standard=${SIZE}, collab=${SIZE}, leaders=${LEADER_COUNT}, churn_per_kind=${CHURN_COUNT}, limit=${PAGE_LIMIT}, cold_clients=${COLD_START_CLIENTS}"
USDB_ECONOMIC_CAPACITY_SIZE="$SIZE" \
USDB_ECONOMIC_CAPACITY_LEADER_COUNT="$LEADER_COUNT" \
USDB_ECONOMIC_CAPACITY_CHURN_COUNT="$CHURN_COUNT" \
USDB_ECONOMIC_CAPACITY_PAGE_LIMIT="$PAGE_LIMIT" \
USDB_ECONOMIC_CAPACITY_COLD_START_CLIENTS="$COLD_START_CLIENTS" \
USDB_ECONOMIC_CAPACITY_REPORT_FILE="$REPORT_FILE" \
    cargo test \
        --release \
        --manifest-path "$MANIFEST_PATH" \
        -p usdb-indexer \
        test_economic_view_capacity_supplement \
        -- \
        --ignored \
        --nocapture \
        --test-threads=1

echo "economic capacity report: ${REPORT_FILE}"
