#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
OUTPUT_ROOT="${WORLD_SOAK_OUTPUT_ROOT:-/tmp/usdb-world-soak-matrix}"
WORKSPACE_ROOT="${WORLD_SOAK_WORKSPACE_ROOT:-${OUTPUT_ROOT}/workspaces}"
BLOCKS="${WORLD_SOAK_BLOCKS:-2500}"
AGENT_COUNT="${WORLD_SOAK_AGENT_COUNT:-24}"
PARALLELISM="${WORLD_SOAK_PARALLELISM:-1}"
BASE_PORT="${WORLD_SOAK_BASE_PORT:-29100}"
PORT_STRIDE="${WORLD_SOAK_PORT_STRIDE:-20}"
ORDINAL_OFFSET="${WORLD_SOAK_ORDINAL_OFFSET:-0}"
KEEP_WORKSPACES="${WORLD_SOAK_KEEP_WORKSPACES:-0}"

read -r -a SEEDS <<<"${WORLD_SOAK_SEEDS:-41 42 43}"

require_positive_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    echo "${name} must be a positive integer, have: ${value}" >&2
    exit 2
  fi
}

require_nonnegative_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[0-9]+$ ]]; then
    echo "${name} must be a non-negative integer, have: ${value}" >&2
    exit 2
  fi
}

require_boolean() {
  local name="$1"
  local value="$2"
  case "$value" in
    0|1)
      ;;
    *)
      echo "${name} must be 0 or 1, have: ${value}" >&2
      exit 2
      ;;
  esac
}

require_positive_integer WORLD_SOAK_BLOCKS "$BLOCKS"
require_positive_integer WORLD_SOAK_AGENT_COUNT "$AGENT_COUNT"
require_positive_integer WORLD_SOAK_PARALLELISM "$PARALLELISM"
require_positive_integer WORLD_SOAK_PORT_STRIDE "$PORT_STRIDE"
require_nonnegative_integer WORLD_SOAK_ORDINAL_OFFSET "$ORDINAL_OFFSET"
require_boolean WORLD_SOAK_KEEP_WORKSPACES "$KEEP_WORKSPACES"
if ((${#SEEDS[@]} == 0)); then
  echo "WORLD_SOAK_SEEDS must contain at least one seed" >&2
  exit 2
fi
for seed in "${SEEDS[@]}"; do
  require_positive_integer WORLD_SOAK_SEED "$seed"
done

mkdir -p "$OUTPUT_ROOT" "$WORKSPACE_ROOT"

assert_port_available() {
  local port="$1"
  python3 - "$port" <<'PY'
import socket
import sys

port = int(sys.argv[1])
with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as sock:
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("127.0.0.1", port))
PY
}

run_seed() {
  local ordinal="$1"
  local seed="$2"
  local port_base=$((BASE_PORT + ordinal * PORT_STRIDE))
  local work_dir="${WORKSPACE_ROOT}/seed-${seed}"
  local report_file="${OUTPUT_ROOT}/seed-${seed}-report.jsonl"
  local simulator_log="${OUTPUT_ROOT}/seed-${seed}-simulator.log"
  local driver_log="${OUTPUT_ROOT}/seed-${seed}-driver.log"
  local resource_file="${OUTPUT_ROOT}/seed-${seed}-resource.txt"
  local summary_file="${OUTPUT_ROOT}/seed-${seed}-summary.json"
  local recovery_file="${OUTPUT_ROOT}/seed-${seed}-recovery.json"
  local started_at
  local resumed_from_recovery=0
  local -a time_prefix=()
  local -a port_offsets=(0 1 2 10 11 12)
  started_at="$(date +%s)"
  if [[ -f "$recovery_file" ]]; then
    resumed_from_recovery=1
    port_offsets=(10 11 12)
  fi
  for offset in "${port_offsets[@]}"; do
    assert_port_available "$((port_base + offset))"
  done
  if [[ -x /usr/bin/time ]]; then
    time_prefix=(/usr/bin/time -v -o "$resource_file")
  else
    printf 'GNU /usr/bin/time is unavailable; resource summary omitted.\n' >"$resource_file"
  fi

  mkdir -p "$work_dir"
  echo "[world-soak-matrix] starting seed=${seed}, blocks=${BLOCKS}, agents=${AGENT_COUNT}, work_dir=${work_dir}"
  if ! "${time_prefix[@]}" \
    env \
      WORK_DIR="$work_dir" \
      AGENT_COUNT="$AGENT_COUNT" \
      BTC_RPC_PORT="$port_base" \
      BTC_P2P_PORT="$((port_base + 1))" \
      BH_RPC_PORT="$((port_base + 10))" \
      USDB_INDEXER_RPC_PORT="$((port_base + 11))" \
      ORD_SERVER_PORT="$((port_base + 12))" \
      SIM_BLOCKS="$BLOCKS" \
      SIM_SEED="$seed" \
      SIM_MAX_ACTIONS_PER_BLOCK=2 \
      SIM_INITIAL_ACTIVE_AGENTS=6 \
      SIM_AGENT_GROWTH_INTERVAL_BLOCKS=50 \
      SIM_AGENT_GROWTH_STEP=1 \
      SIM_FAIL_FAST=1 \
      SIM_REPORT_FILE="$report_file" \
      SIM_REPORT_FLUSH_EVERY=10 \
      SIM_LOG_FILE="$simulator_log" \
      SIM_RECOVERY_STATE_FILE="$recovery_file" \
      SIM_AGENT_SELF_CHECK_INTERVAL_BLOCKS=5 \
      SIM_AGENT_SELF_CHECK_SAMPLE_SIZE=12 \
      SIM_GLOBAL_CROSS_CHECK_INTERVAL_BLOCKS=50 \
      SIM_GLOBAL_CROSS_CHECK_LEADERBOARD_TOP_N=20 \
      SIM_GLOBAL_CROSS_CHECK_OWNER_SAMPLE_SIZE=12 \
      SIM_ECONOMIC_PAGE_LIMIT=32 \
      SIM_ECONOMIC_BOOTSTRAP_ENABLED=1 \
      SIM_VALIDATOR_SAMPLE_ENABLED=1 \
      SIM_VALIDATOR_SAMPLE_MODE=candidate_set \
      SIM_VALIDATOR_SAMPLE_TAMPER_ENABLED=1 \
      SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=100 \
      SIM_VALIDATOR_SAMPLE_SIZE=10 \
      SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE=2 \
      SIM_REORG_INTERVAL_BLOCKS=500 \
      SIM_REORG_DEPTH=3 \
      SIM_REORG_MAX_EVENTS=4 \
      "$SCRIPT_DIR/regtest_world_sim.sh" >"$driver_log" 2>&1; then
    echo "[world-soak-matrix] seed ${seed} failed; preserving ${work_dir}" >&2
    return 1
  fi

  local finished_at duration_sec workspace_bytes
  finished_at="$(date +%s)"
  duration_sec=$((finished_at - started_at))
  workspace_bytes="$(du -sb "$work_dir" | awk '{print $1}')"
  python3 - "$seed" "$BLOCKS" "$duration_sec" "$workspace_bytes" \
    "$resumed_from_recovery" "$report_file" "$summary_file" <<'PY'
import json
import pathlib
import sys

seed = int(sys.argv[1])
blocks = int(sys.argv[2])
duration_sec = int(sys.argv[3])
workspace_bytes = int(sys.argv[4])
resumed_from_recovery = sys.argv[5] == "1"
report_path = pathlib.Path(sys.argv[6])
summary_path = pathlib.Path(sys.argv[7])

session_end = None
for line in report_path.read_text(encoding="utf-8").splitlines():
    payload = json.loads(line)
    if payload.get("event") == "session_end":
        session_end = payload
if session_end is None:
    raise SystemExit(f"missing session_end in {report_path}")

metrics = session_end["final_metrics"]
failures = {key: value for key, value in metrics.items() if key.endswith("_fail") and value}
if failures:
    raise SystemExit(f"non-zero failure metrics for seed {seed}: {failures}")

summary = {
    "seed": seed,
    "blocks": blocks,
    "duration_seconds": duration_sec,
    "duration_scope": (
        "recovery_stage" if resumed_from_recovery else "full_run"
    ),
    "workspace_bytes_before_cleanup": workspace_bytes,
    "resumed_from_recovery": resumed_from_recovery,
    "final_metrics": metrics,
}
summary_path.write_text(json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8")
PY

  for name in ord-server.log balance-history.log usdb-indexer.log; do
    if [[ -f "${work_dir}/${name}" ]]; then
      cp "${work_dir}/${name}" "${OUTPUT_ROOT}/seed-${seed}-${name}"
    fi
  done
  if [[ "$KEEP_WORKSPACES" == "0" ]]; then
    case "$work_dir" in
      "${WORKSPACE_ROOT}"/seed-*)
        rm -rf "$work_dir"
        ;;
      *)
        echo "refusing to remove unexpected workspace path: ${work_dir}" >&2
        return 1
        ;;
    esac
  fi
  echo "[world-soak-matrix] completed seed=${seed}, duration_sec=${duration_sec}"
}

pids=()
seeds_by_pid=()
matrix_failed=0
wait_oldest() {
  local pid="${pids[0]}"
  local seed="${seeds_by_pid[0]}"
  if ! wait "$pid"; then
    echo "[world-soak-matrix] seed ${seed} failed" >&2
    matrix_failed=1
  fi
  pids=("${pids[@]:1}")
  seeds_by_pid=("${seeds_by_pid[@]:1}")
}

for ordinal in "${!SEEDS[@]}"; do
  seed="${SEEDS[$ordinal]}"
  run_seed "$((ordinal + ORDINAL_OFFSET))" "$seed" &
  pids+=("$!")
  seeds_by_pid+=("$seed")
  if ((${#pids[@]} >= PARALLELISM)); then
    wait_oldest
  fi
done
while ((${#pids[@]} > 0)); do
  wait_oldest
done
if ((matrix_failed != 0)); then
  exit 1
fi

python3 - "$OUTPUT_ROOT" "${SEEDS[@]}" <<'PY'
import json
import pathlib
import sys

root = pathlib.Path(sys.argv[1])
seeds = [int(value) for value in sys.argv[2:]]
summaries = [
    json.loads((root / f"seed-{seed}-summary.json").read_text(encoding="utf-8"))
    for seed in seeds
]
metric_names = sorted(
    {
        name
        for summary in summaries
        for name in summary["final_metrics"]
    }
)
aggregate = {
    "schema_version": "usdb-world-soak-matrix:v2",
    "seeds": seeds,
    "runs": summaries,
    "total_blocks": sum(item["blocks"] for item in summaries),
    "total_duration_seconds": sum(item["duration_seconds"] for item in summaries),
    "total_workspace_bytes_before_cleanup": sum(
        item["workspace_bytes_before_cleanup"] for item in summaries
    ),
    "final_metrics_totals": {
        name: sum(item["final_metrics"].get(name, 0) for item in summaries)
        for name in metric_names
    },
}
(root / "matrix-summary.json").write_text(
    json.dumps(aggregate, indent=2, sort_keys=True) + "\n",
    encoding="utf-8",
)
PY

echo "[world-soak-matrix] reports: ${OUTPUT_ROOT}"
