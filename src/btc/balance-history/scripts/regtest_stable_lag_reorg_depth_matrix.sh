#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MATRIX_WORK_DIR="${MATRIX_WORK_DIR:-$(mktemp -d /tmp/usdb-bh-stable-lag-matrix-XXXXXX)}"
EXPECTED_STABLE_LAG="${EXPECTED_STABLE_LAG:-5}"
BASE_PORT="${BASE_PORT:-30710}"
PORT_STRIDE="${PORT_STRIDE:-40}"
REGTEST_LOG_PREFIX="[stable-lag-depth-matrix]"

log() {
  echo "${REGTEST_LOG_PREFIX} $*"
}

main() {
  if (( EXPECTED_STABLE_LAG <= 1 )); then
    log "EXPECTED_STABLE_LAG must be greater than 1"
    exit 1
  fi

  local case_index relation depth port_base case_work_dir
  local -a relations=("lt" "eq" "gt")
  local -a depths=(
    "$((EXPECTED_STABLE_LAG - 1))"
    "$EXPECTED_STABLE_LAG"
    "$((EXPECTED_STABLE_LAG + 1))"
  )

  mkdir -p "$MATRIX_WORK_DIR"
  log "Running stable-lag reorg depth matrix in ${MATRIX_WORK_DIR}"

  for case_index in "${!depths[@]}"; do
    relation="${relations[$case_index]}"
    depth="${depths[$case_index]}"
    port_base=$((BASE_PORT + case_index * PORT_STRIDE))
    case_work_dir="${MATRIX_WORK_DIR}/depth-${relation}-${depth}"

    log "START depth ${relation} lag: depth=${depth}, lag=${EXPECTED_STABLE_LAG}"
    WORK_DIR="$case_work_dir" \
    EXPECTED_STABLE_LAG="$EXPECTED_STABLE_LAG" \
    REORG_DEPTH="$depth" \
    BTC_RPC_PORT="$((port_base + 22))" \
    BTC_P2P_PORT="$((port_base + 23))" \
    ONLINE_RPC_PORT="$port_base" \
    OFFLINE_RPC_PORT="$((port_base + 1))" \
    JOINER_RPC_PORT="$((port_base + 2))" \
    WALLET_NAME="bhstablelag${relation}${depth}" \
      bash "${SCRIPT_DIR}/regtest_stable_lag_reorg_depth_case.sh"
    log "PASS depth ${relation} lag: depth=${depth}, lag=${EXPECTED_STABLE_LAG}"
  done

  log "Stable-lag reorg depth matrix passed."
}

main "$@"
