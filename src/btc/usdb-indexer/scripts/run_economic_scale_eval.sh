#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
MANIFEST_PATH="${REPO_ROOT}/src/btc/Cargo.toml"
OUTPUT_DIR="${USDB_ECONOMIC_SCALE_OUTPUT_DIR:-${REPO_ROOT}/src/btc/target/economic-scale}"
PAGE_LIMIT="${USDB_ECONOMIC_SCALE_PAGE_LIMIT:-100}"
LEADER_COUNT="${USDB_ECONOMIC_SCALE_LEADER_COUNT:-1}"
CONCURRENT_CLIENTS="${USDB_ECONOMIC_SCALE_CONCURRENT_CLIENTS:-0}"
CONCURRENT_ITERATIONS="${USDB_ECONOMIC_SCALE_CONCURRENT_ITERATIONS:-1}"
COLD_CACHE="${USDB_ECONOMIC_SCALE_COLD_CACHE:-0}"

if (($# > 0)); then
    SIZES=("$@")
else
    read -r -a SIZES <<<"${USDB_ECONOMIC_SCALE_SIZES:-100 1000 10000}"
fi

mkdir -p "${OUTPUT_DIR}"

for numeric in "$PAGE_LIMIT" "$LEADER_COUNT" "$CONCURRENT_ITERATIONS"; do
    if [[ ! "$numeric" =~ ^[1-9][0-9]*$ ]]; then
        echo "invalid positive economic scale setting: ${numeric}" >&2
        exit 2
    fi
done
if [[ ! "$CONCURRENT_CLIENTS" =~ ^[0-9]+$ ]]; then
    echo "invalid concurrent client count: ${CONCURRENT_CLIENTS}" >&2
    exit 2
fi
case "$COLD_CACHE" in
    0|1|true|TRUE|yes|YES|false|FALSE|no|NO)
        ;;
    *)
        echo "invalid cold-cache setting: ${COLD_CACHE}" >&2
        exit 2
        ;;
esac
if [[ "$COLD_CACHE" =~ ^(1|true|TRUE|yes|YES)$ ]] && ! command -v dd >/dev/null 2>&1; then
    echo "dd is required for file-level page-cache eviction" >&2
    exit 1
fi

for size in "${SIZES[@]}"; do
    if [[ ! "${size}" =~ ^[1-9][0-9]*$ ]]; then
        echo "invalid scale size: ${size}" >&2
        exit 2
    fi

    if ((LEADER_COUNT > size)); then
        echo "leader count ${LEADER_COUNT} exceeds scale size ${size}" >&2
        exit 2
    fi

    report_file="${OUTPUT_DIR}/economic-scale-${size}-leaders-${LEADER_COUNT}-limit-${PAGE_LIMIT}-clients-${CONCURRENT_CLIENTS}-cold-${COLD_CACHE}.json"
    echo "running economic scale evaluation: standard=${size}, collab=${size}, leaders=${LEADER_COUNT}, limit=${PAGE_LIMIT}, clients=${CONCURRENT_CLIENTS}, cold_cache=${COLD_CACHE}"
    USDB_ECONOMIC_SCALE_SIZE="${size}" \
        USDB_ECONOMIC_SCALE_PAGE_LIMIT="${PAGE_LIMIT}" \
        USDB_ECONOMIC_SCALE_LEADER_COUNT="${LEADER_COUNT}" \
        USDB_ECONOMIC_SCALE_CONCURRENT_CLIENTS="${CONCURRENT_CLIENTS}" \
        USDB_ECONOMIC_SCALE_CONCURRENT_ITERATIONS="${CONCURRENT_ITERATIONS}" \
        USDB_ECONOMIC_SCALE_COLD_CACHE="${COLD_CACHE}" \
        USDB_ECONOMIC_SCALE_REPORT_FILE="${report_file}" \
        cargo test \
            --release \
            --manifest-path "${MANIFEST_PATH}" \
            -p usdb-indexer \
            test_economic_view_scale_profile \
            -- \
            --ignored \
            --nocapture \
            --test-threads=1
done

echo "economic scale reports: ${OUTPUT_DIR}"
