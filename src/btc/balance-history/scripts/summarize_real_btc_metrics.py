#!/usr/bin/env python3

import argparse
import json
from collections import defaultdict
from pathlib import Path


def default_metrics_path() -> Path:
    repo_root = Path(__file__).resolve().parents[4]
    return repo_root / "target" / "balance-history-real-btc" / "metrics.jsonl"


def load_records(path: Path) -> list[dict]:
    records = []
    if not path.exists():
        raise SystemExit(f"metrics file not found: {path}")

    for line_no, line in enumerate(path.read_text(encoding="utf-8").splitlines(), start=1):
        line = line.strip()
        if not line:
            continue
        try:
            records.append(json.loads(line))
        except json.JSONDecodeError as exc:
            raise SystemExit(f"invalid JSON at {path}:{line_no}: {exc}") from exc
    return records


def format_number(value):
    if value is None:
        return "-"
    if isinstance(value, float):
        return f"{value:.2f}"
    if isinstance(value, int):
        return f"{value:,}"
    return str(value)


def print_table(title: str, headers: list[str], rows: list[list[object]]) -> None:
    print(f"\n## {title}")
    if not rows:
        print("(no records)")
        return

    rendered = [[format_number(cell) for cell in row] for row in rows]
    widths = [
        max(len(headers[i]), *(len(row[i]) for row in rendered))
        for i in range(len(headers))
    ]
    print(" | ".join(headers[i].ljust(widths[i]) for i in range(len(headers))))
    print(" | ".join("-" * widths[i] for i in range(len(headers))))
    for row in rendered:
        print(" | ".join(row[i].ljust(widths[i]) for i in range(len(headers))))


def summarize_runner(records: list[dict]) -> None:
    runner = [r for r in records if r.get("component") == "balance-history-real-btc-runner"]
    rows = []
    for record in runner:
        rows.append(
            [
                record.get("timestamp_utc", "-").replace("T", " ")[:19],
                record.get("suite"),
                record.get("size"),
                record.get("profile_segment"),
                record.get("filter"),
                record.get("exit_code"),
                record.get("duration_sec"),
                record.get("subset_file_count"),
                record.get("profile_start_file"),
                record.get("profile_file_count"),
            ]
        )
    print_table(
        "Runner Filters",
        [
            "time_utc",
            "suite",
            "size",
            "segment",
            "filter",
            "exit",
            "sec",
            "subset",
            "profile_start",
            "profile_files",
        ],
        rows,
    )


def summarize_runner_groups(records: list[dict]) -> None:
    groups = defaultdict(list)
    for record in records:
        if record.get("component") != "balance-history-real-btc-runner":
            continue
        key = (
            record.get("suite"),
            record.get("size"),
            record.get("profile_segment"),
            record.get("filter"),
        )
        groups[key].append(record)

    rows = []
    for (suite, size, segment, filter_name), items in sorted(groups.items()):
        durations = [int(item.get("duration_sec", 0)) for item in items]
        failures = sum(1 for item in items if item.get("exit_code") != 0)
        rows.append(
            [
                suite,
                size,
                segment,
                filter_name,
                len(items),
                failures,
                min(durations),
                sum(durations) / len(durations),
                max(durations),
            ]
        )
    print_table(
        "Runner Aggregate",
        ["suite", "size", "segment", "filter", "runs", "fail", "min_s", "avg_s", "max_s"],
        rows,
    )


def summarize_profile(records: list[dict]) -> None:
    profile = [r for r in records if r.get("component") == "balance-history-real-btc-test"]
    rows = []
    for record in profile:
        rows.append(
            [
                record.get("metric_type"),
                record.get("profile_segment"),
                record.get("test"),
                record.get("start_file"),
                record.get("requested_file_count"),
                record.get("loaded_file_count", record.get("successful_reads")),
                record.get("loaded_block_count"),
                bytes_to_mib(record.get("used_memory_bytes")),
                record.get("duration_ms"),
                rate(record.get("loaded_block_count"), record.get("duration_ms")),
            ]
        )
    print_table(
        "Profile Metrics",
        [
            "type",
            "segment",
            "test",
            "start",
            "files",
            "loaded",
            "blocks",
            "mem_mib",
            "ms",
            "blocks_s",
        ],
        rows,
    )


def bytes_to_mib(value):
    if value is None:
        return None
    return int(value) / (1024 * 1024)


def rate(count, duration_ms):
    if not count or not duration_ms:
        return None
    return int(count) / (int(duration_ms) / 1000)


def main() -> None:
    parser = argparse.ArgumentParser(description="Summarize balance-history real BTC metrics JSONL.")
    parser.add_argument("metrics_file", nargs="?", type=Path, default=default_metrics_path())
    parser.add_argument("--last", type=int, default=0, help="Only summarize the last N JSONL records.")
    args = parser.parse_args()

    records = load_records(args.metrics_file)
    if args.last > 0:
        records = records[-args.last :]

    print(f"# Balance-History Real BTC Metrics Summary")
    print(f"metrics_file: {args.metrics_file}")
    print(f"records: {len(records)}")
    summarize_runner(records)
    summarize_runner_groups(records)
    summarize_profile(records)


if __name__ == "__main__":
    main()
