#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
MANIFEST_PATH="${REPO_ROOT}/src/btc/Cargo.toml"
OUTPUT_DIR="${USDB_ECONOMIC_SCALE_OUTPUT_DIR:-${REPO_ROOT}/src/btc/target/economic-scale}"
PAGE_LIMIT="${USDB_ECONOMIC_SCALE_PAGE_LIMIT:-100}"

if (($# > 0)); then
    SIZES=("$@")
else
    read -r -a SIZES <<<"${USDB_ECONOMIC_SCALE_SIZES:-100 1000 10000}"
fi

mkdir -p "${OUTPUT_DIR}"

for size in "${SIZES[@]}"; do
    if [[ ! "${size}" =~ ^[1-9][0-9]*$ ]]; then
        echo "invalid scale size: ${size}" >&2
        exit 2
    fi

    report_file="${OUTPUT_DIR}/economic-scale-${size}-limit-${PAGE_LIMIT}.json"
    echo "running economic scale evaluation: standard=${size}, collab=${size}, limit=${PAGE_LIMIT}"
    USDB_ECONOMIC_SCALE_SIZE="${size}" \
        USDB_ECONOMIC_SCALE_PAGE_LIMIT="${PAGE_LIMIT}" \
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
