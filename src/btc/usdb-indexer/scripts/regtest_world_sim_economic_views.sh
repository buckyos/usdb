#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_SIM_SCRIPT="${SCRIPT_DIR}/regtest_world_sim.sh"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-indexer-economic-world-sim-XXXXXX)}"
SIM_REPORT_FILE="${SIM_REPORT_FILE:-${WORK_DIR}/economic-world-sim-report.jsonl}"

env \
  WORK_DIR="${WORK_DIR}" \
  AGENT_COUNT="${AGENT_COUNT:-5}" \
  SIM_BLOCKS="${SIM_BLOCKS:-3}" \
  SIM_SEED="${SIM_SEED:-20260718}" \
  SIM_FAIL_FAST=1 \
  SIM_POLICY_MODE=scripted \
  SIM_SCRIPTED_CYCLE=noop \
  SIM_ECONOMIC_BOOTSTRAP_ENABLED=1 \
  SIM_ECONOMIC_PAGE_LIMIT="${SIM_ECONOMIC_PAGE_LIMIT:-2}" \
  SIM_GLOBAL_CROSS_CHECK_ENABLED=1 \
  SIM_GLOBAL_CROSS_CHECK_INTERVAL_BLOCKS=1 \
  SIM_AGENT_SELF_CHECK_ENABLED=1 \
  SIM_AGENT_SELF_CHECK_INTERVAL_BLOCKS=1 \
  SIM_VALIDATOR_SAMPLE_ENABLED=1 \
  SIM_VALIDATOR_SAMPLE_MODE=candidate_set \
  SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS=1 \
  SIM_VALIDATOR_SAMPLE_SIZE=0 \
  SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE=1 \
  SIM_REPORT_ENABLED=1 \
  SIM_REPORT_FILE="${SIM_REPORT_FILE}" \
  bash "${WORLD_SIM_SCRIPT}"

python3 - "${SIM_REPORT_FILE}" <<'PY'
import json
import sys

report_path = sys.argv[1]
with open(report_path, encoding="utf-8") as report_file:
    events = [json.loads(line) for line in report_file if line.strip()]

complete = next(
    (event for event in events if event.get("event") == "economic_bootstrap_complete"),
    None,
)
if complete is None:
    raise SystemExit("missing economic_bootstrap_complete event")

if complete.get("leader_remint_followed_collabs") != [complete.get("address_collab_v1")]:
    raise SystemExit("address collab did not exclusively follow the Leader remint")
expected_final = sorted(
    [complete.get("fixed_collab_v2"), complete.get("address_collab_v2")]
)
if complete.get("final_collab_ids") != expected_final:
    raise SystemExit("final fixed/address collab set mismatch")

session_end = next(
    (event for event in reversed(events) if event.get("event") == "session_end"),
    None,
)
if session_end is None:
    raise SystemExit("missing session_end event")
metrics = session_end.get("final_metrics") or {}
required_actions = (
    "standard_mint_ok",
    "fixed_collab_mint_ok",
    "address_collab_mint_ok",
    "standard_remint_ok",
    "fixed_collab_remint_ok",
    "address_collab_remint_ok",
)
for metric in required_actions:
    if int(metrics.get(metric, 0)) < 1:
        raise SystemExit(f"bootstrap did not execute required action: {metric}")
for metric in (
    "verify_fail",
    "global_cross_check_fail",
    "validator_sample_fail",
):
    if int(metrics.get(metric, 0)) != 0:
        raise SystemExit(f"economic world simulator reported {metric}={metrics.get(metric)}")
if int(metrics.get("validator_sample_ok", 0)) < 1:
    raise SystemExit("historical candidate/profile/breakdown replay did not run")

cross_checks = [
    event.get("global_cross_check_info") or {}
    for event in events
    if event.get("event") in {"economic_bootstrap_step", "tick"}
    and event.get("global_cross_check_info")
]
if not any(
    int(info.get("candidate_count", 0)) >= 2
    and int(info.get("active_collab_count", 0)) >= 2
    and int(info.get("breakdown_row_count", 0)) >= 2
    for info in cross_checks
):
    raise SystemExit("no cross-check observed the final multi-collab economic state")

print(
    "economic world simulator passed: "
    f"leader={complete['leader_v2']}, collabs={complete['final_collab_ids']}, "
    f"validator_samples={metrics['validator_sample_ok']}"
)
PY
