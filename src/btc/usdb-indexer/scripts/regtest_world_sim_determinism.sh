#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_SIM_SCRIPT="${SCRIPT_DIR}/regtest_world_sim.sh"
COMPARE_SCRIPT="${SCRIPT_DIR}/compare_world_sim_reports.py"

WORK_DIR="${WORK_DIR:-/tmp/usdb-world-determinism}"
RUN1_WORK_DIR="${RUN1_WORK_DIR:-${WORK_DIR}/run1}"
RUN2_WORK_DIR="${RUN2_WORK_DIR:-${WORK_DIR}/run2}"

SIM_SEED="${SIM_SEED:-20260309}"
SIM_BLOCKS="${SIM_BLOCKS:-300}"
SIM_FAIL_FAST="${SIM_FAIL_FAST:-1}"
SIM_REPORT_FLUSH_EVERY="${SIM_REPORT_FLUSH_EVERY:-1}"

RUN1_REPORT_FILE="${RUN1_REPORT_FILE:-${RUN1_WORK_DIR}/world-sim-report.jsonl}"
RUN2_REPORT_FILE="${RUN2_REPORT_FILE:-${RUN2_WORK_DIR}/world-sim-report.jsonl}"

log() {
  echo "[world-sim-determinism] $*"
}

require_file() {
  local path="$1"
  if [[ ! -f "$path" ]]; then
    echo "Required file not found: $path" >&2
    exit 1
  fi
}

run_once() {
  local run_tag="$1"
  local run_work_dir="$2"
  local report_file="$3"

  log "Starting ${run_tag}: seed=${SIM_SEED}, blocks=${SIM_BLOCKS}, work_dir=${run_work_dir}"
  env \
    WORK_DIR="${run_work_dir}" \
    SIM_SEED="${SIM_SEED}" \
    SIM_BLOCKS="${SIM_BLOCKS}" \
    SIM_FAIL_FAST="${SIM_FAIL_FAST}" \
    SIM_REPORT_ENABLED=1 \
    SIM_REPORT_FILE="${report_file}" \
    SIM_REPORT_FLUSH_EVERY="${SIM_REPORT_FLUSH_EVERY}" \
    "${WORLD_SIM_SCRIPT}"

  if [[ ! -f "${report_file}" ]]; then
    log "Missing report after ${run_tag}: report=${report_file}"
    exit 1
  fi
  log "Completed ${run_tag}: report=${report_file}"
}

main() {
  require_file "${WORLD_SIM_SCRIPT}"
  require_file "${COMPARE_SCRIPT}"

  mkdir -p "${WORK_DIR}" "${RUN1_WORK_DIR}" "${RUN2_WORK_DIR}"

  run_once "run1" "${RUN1_WORK_DIR}" "${RUN1_REPORT_FILE}"
  run_once "run2" "${RUN2_WORK_DIR}" "${RUN2_REPORT_FILE}"

  log "Comparing reports"
  python3 "${COMPARE_SCRIPT}" --lhs "${RUN1_REPORT_FILE}" --rhs "${RUN2_REPORT_FILE}"
  log "Determinism check passed."
}

main "$@"
