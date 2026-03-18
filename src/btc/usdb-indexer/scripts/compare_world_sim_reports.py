#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
from pathlib import Path
from typing import Any


class ReportCompareError(Exception):
    pass


def load_jsonl(path: Path) -> list[dict[str, Any]]:
    if not path.exists():
        raise ReportCompareError(f"report file not found: {path}")
    if not path.is_file():
        raise ReportCompareError(f"report path is not a file: {path}")

    events: list[dict[str, Any]] = []
    with path.open("r", encoding="utf-8") as f:
        for line_no, raw in enumerate(f, start=1):
            text = raw.strip()
            if not text:
                continue
            try:
                obj = json.loads(text)
            except Exception as e:  # noqa: BLE001
                raise ReportCompareError(
                    f"failed to parse JSONL line: file={path}, line={line_no}, error={e}, text={text[:256]}"
                ) from e
            if not isinstance(obj, dict):
                raise ReportCompareError(
                    f"invalid JSONL event object: file={path}, line={line_no}, value={obj}"
                )
            events.append(obj)
    return events


def find_single_event(events: list[dict[str, Any]], event_name: str) -> dict[str, Any]:
    matched = [evt for evt in events if str(evt.get("event")) == event_name]
    if len(matched) != 1:
        raise ReportCompareError(
            f"expected exactly one '{event_name}' event, got={len(matched)}"
        )
    return matched[0]


def collect_tick_events(events: list[dict[str, Any]]) -> list[dict[str, Any]]:
    ticks = [evt for evt in events if str(evt.get("event")) == "tick"]
    ticks.sort(key=lambda v: int(v.get("tick", 0)))
    return ticks


def collect_named_events(events: list[dict[str, Any]], event_name: str) -> list[dict[str, Any]]:
    matched = [evt for evt in events if str(evt.get("event")) == event_name]
    matched.sort(key=lambda v: int(v.get("tick", 0)))
    return matched


def format_mismatch(prefix: str, left: Any, right: Any) -> str:
    return (
        f"{prefix}\n"
        f"  lhs={json.dumps(left, ensure_ascii=False, sort_keys=True)}\n"
        f"  rhs={json.dumps(right, ensure_ascii=False, sort_keys=True)}"
    )


def normalize_reorg_event(evt: dict[str, Any]) -> dict[str, Any]:
    cross_check = evt.get("global_cross_check_info")
    normalized_cross_check: dict[str, Any] | None = None
    if isinstance(cross_check, dict):
        normalized_cross_check = {
            "tick": cross_check.get("tick"),
            "block_height": cross_check.get("block_height"),
            "top_n": cross_check.get("top_n"),
            "leaderboard_compared_count": cross_check.get("leaderboard_compared_count"),
            "active_owner_count": cross_check.get("active_owner_count"),
            "sampled_owner_count": cross_check.get("sampled_owner_count"),
            "sampled_balance_sum": cross_check.get("sampled_balance_sum"),
            "snapshot_total_balance": cross_check.get("snapshot_total_balance"),
        }

    return {
        "tick": evt.get("tick"),
        "depth": evt.get("depth"),
        "rollback_start_height": evt.get("rollback_start_height"),
        "rollback_target_height": evt.get("rollback_target_height"),
        "tip_height": evt.get("tip_height"),
        "loaded_pass_rows": evt.get("loaded_pass_rows"),
        "unknown_owner_rows": evt.get("unknown_owner_rows"),
        "active_owner_rows": evt.get("active_owner_rows"),
        "global_cross_check_info": normalized_cross_check,
    }


def compare_reports(
    lhs_path: Path,
    rhs_path: Path,
    tick_fields: list[str],
) -> None:
    lhs_events = load_jsonl(lhs_path)
    rhs_events = load_jsonl(rhs_path)

    lhs_start = find_single_event(lhs_events, "session_start")
    rhs_start = find_single_event(rhs_events, "session_start")
    lhs_end = find_single_event(lhs_events, "session_end")
    rhs_end = find_single_event(rhs_events, "session_end")

    start_keys = [
        "seed",
        "action_seed",
        "diagnostic_seed",
        "blocks",
        "total_agents",
        "initial_active_agents",
        "policy_mode",
        "scripted_cycle",
        "agent_self_check_enabled",
        "agent_self_check_interval_blocks",
        "agent_self_check_sample_size",
        "global_cross_check_enabled",
        "global_cross_check_interval_blocks",
        "global_cross_check_leaderboard_top_n",
        "global_cross_check_owner_sample_size",
        "reorg_interval_blocks",
        "reorg_depth",
        "reorg_max_events",
    ]
    for key in start_keys:
        if lhs_start.get(key) != rhs_start.get(key):
            raise ReportCompareError(
                format_mismatch(f"session_start mismatch on key='{key}'", lhs_start.get(key), rhs_start.get(key))
            )

    lhs_metrics = lhs_end.get("final_metrics")
    rhs_metrics = rhs_end.get("final_metrics")
    if lhs_metrics != rhs_metrics:
        raise ReportCompareError(
            format_mismatch("final_metrics mismatch", lhs_metrics, rhs_metrics)
        )

    lhs_ticks = collect_tick_events(lhs_events)
    rhs_ticks = collect_tick_events(rhs_events)
    if len(lhs_ticks) != len(rhs_ticks):
        raise ReportCompareError(
            format_mismatch(
                "tick count mismatch",
                {"tick_count": len(lhs_ticks)},
                {"tick_count": len(rhs_ticks)},
            )
        )

    for idx, (lhs_tick, rhs_tick) in enumerate(zip(lhs_ticks, rhs_ticks), start=1):
        lhs_tick_id = int(lhs_tick.get("tick", 0))
        rhs_tick_id = int(rhs_tick.get("tick", 0))
        if lhs_tick_id != rhs_tick_id:
            raise ReportCompareError(
                format_mismatch(
                    f"tick id mismatch at pair_index={idx}",
                    {"tick": lhs_tick_id},
                    {"tick": rhs_tick_id},
                )
            )

        for key in tick_fields:
            if lhs_tick.get(key) != rhs_tick.get(key):
                raise ReportCompareError(
                    format_mismatch(
                        f"tick field mismatch: tick={lhs_tick_id}, key='{key}'",
                        lhs_tick.get(key),
                        rhs_tick.get(key),
                    )
                )

    lhs_reorgs = collect_named_events(lhs_events, "reorg")
    rhs_reorgs = collect_named_events(rhs_events, "reorg")
    if len(lhs_reorgs) != len(rhs_reorgs):
        raise ReportCompareError(
            format_mismatch(
                "reorg event count mismatch",
                {"reorg_count": len(lhs_reorgs)},
                {"reorg_count": len(rhs_reorgs)},
            )
        )

    for idx, (lhs_reorg, rhs_reorg) in enumerate(zip(lhs_reorgs, rhs_reorgs), start=1):
        lhs_normalized = normalize_reorg_event(lhs_reorg)
        rhs_normalized = normalize_reorg_event(rhs_reorg)
        if lhs_normalized != rhs_normalized:
            raise ReportCompareError(
                format_mismatch(
                    f"reorg event mismatch at index={idx}",
                    lhs_normalized,
                    rhs_normalized,
                )
            )


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        prog="compare_world_sim_reports",
        description="Compare two world-sim JSONL reports for deterministic replay checks",
    )
    parser.add_argument("--lhs", required=True, help="First JSONL report path")
    parser.add_argument("--rhs", required=True, help="Second JSONL report path")
    parser.add_argument(
        "--tick-fields",
        default=(
            "block_height,actions,action_failed,verify_failed,"
            "agent_self_checked,agent_self_check_failed,"
            "global_cross_checked,global_cross_check_failed,reorg_applied,"
            "known_passes,tick_action_type_counts,"
            "pass_total,pass_active,pass_invalid,"
            "active_addresses,active_total_balance"
        ),
        help="Comma-separated tick fields to compare",
    )
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    lhs = Path(args.lhs)
    rhs = Path(args.rhs)
    tick_fields = [v.strip() for v in str(args.tick_fields).split(",") if v.strip()]
    if not tick_fields:
        raise ReportCompareError("tick_fields must not be empty")

    compare_reports(lhs, rhs, tick_fields)
    print(
        "[world-sim-compare] reports are deterministic: "
        f"lhs={lhs}, rhs={rhs}, tick_fields={tick_fields}"
    )
    return 0


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except ReportCompareError as e:
        print(f"[world-sim-compare] compare failed: {e}")
        raise SystemExit(1)
