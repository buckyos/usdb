"""Bounded, unprivileged kernel-log evidence; never diagnose hardware from throughput."""
from __future__ import annotations

import json
import re
import subprocess
import tempfile
import time

INTERVAL_MS = 60_000
WINDOW_MS = 24 * 60 * 60 * 1000
MAX_RECORDS = 128
MAX_OUTPUT = 512 * 1024
PATTERNS = {
    "io_error": r"\bI/O error\b|\bblk_update_request:.*\berror\b|\bcritical medium error\b",
    "ata_link_error": r"\bata[0-9].*(?:hard resetting link|SError:.*(?:BadCRC|PHYRdyChg|LinkSeq))",
    "filesystem_error": r"\bEXT4-fs error\b|\bBTRFS.*(?:error|corrupt)\b|\bXFS.*(?:corrupt|I/O error)\b",
    "filesystem_read_only": r"\b(?:Remounting|re-mounting) filesystem read-only\b|\bXFS.*shut.*down\b",
}
MATCH = "|".join(PATTERNS.values())


def read_kernel_records(at):
    """Keep output and execution bounded. No sudo, host writes or raw-log export."""
    args = ["journalctl", "--kernel", "--boot", "--no-pager", "--output=json",
            "--since", "@" + str(max(0, at - WINDOW_MS) // 1000),
            "--grep", MATCH, "--case-sensitive=no", "--lines", str(MAX_RECORDS)]
    with tempfile.TemporaryFile() as output, tempfile.TemporaryFile() as errors:
        result = subprocess.run(args, stdin=subprocess.DEVNULL, stdout=output, stderr=errors, timeout=3)
        output.seek(0)
        body = output.read(MAX_OUTPUT + 1)
        errors.seek(0)
        # journalctl may return success with a permission warning and partial logs.
        # An empty partial journal must never be interpreted as a healthy disk.
        if errors.read(1) or result.returncode not in (0, 1):
            raise ValueError("kernel journal unavailable or incomplete")
    if len(body) > MAX_OUTPUT:
        raise ValueError("kernel journal output exceeded limit")
    return body.decode("utf-8").splitlines()


def summarize(lines, at):
    """Retain only classified counts and timestamps, never messages or paths."""
    groups = {}
    for line in lines:
        row = json.loads(line)
        if not isinstance(row, dict):
            raise ValueError("invalid kernel record")
        message = row.get("MESSAGE")
        if not isinstance(message, str):
            raise ValueError("invalid kernel message")
        timestamp = int(row["__REALTIME_TIMESTAMP"]) // 1000
        if not at - WINDOW_MS <= timestamp <= at:
            continue
        for code, pattern in PATTERNS.items():
            if re.search(pattern, message, re.IGNORECASE):
                item = groups.setdefault(code, dict(code=code, count=0, first_seen_ms=timestamp, last_seen_ms=timestamp))
                item["count"] += 1
                item["first_seen_ms"] = min(item["first_seen_ms"], timestamp)
                item["last_seen_ms"] = max(item["last_seen_ms"], timestamp)
    return dict(status="available", at_ms=at, window_ms=WINDOW_MS, scope="host_current_boot",
                truncated=len(lines) >= MAX_RECORDS, errors=sorted(groups.values(), key=lambda item: item["code"]))


class Collector:
    """Cache the independent disk probe so resource samples do not rescan the journal."""
    def __init__(self):
        self.value = None
        self.sampled = None

    def sample(self):
        tick = time.monotonic()
        if self.value is not None and tick - self.sampled < INTERVAL_MS / 1000:
            return self.value
        at = time.time_ns() // 1_000_000
        try:
            self.value = summarize(read_kernel_records(at), at)
        except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
            self.value = dict(status="unavailable", at_ms=at, errors=[],
                              reason="Kernel journal access failed, timed out, or returned incomplete evidence")
        self.sampled = tick
        return self.value


def evaluate(store, sample, at, settings):
    """Warn on observed errors immediately; unknown evidence never resolves an alert."""
    sampled = sample.get("at_ms")
    fresh = type(sampled) is int and 0 <= at - sampled <= INTERVAL_MS * 3
    previous = store.get("last_disk_health_sample_ms")
    if fresh and sampled == previous:
        return
    known = fresh and sample.get("status") == "available"
    value = sample if fresh else dict(status="stale" if sampled else "not_started", errors=[])
    store.put("disk_health", value)
    store.condition("disk-health-observation", "host", "DISK_HEALTH_UNAVAILABLE", not known if sampled else None,
                    at, evidence={"status": value["status"], "action": "Check kernel journal access for the monitor account"},
                    warning_ms=settings["warning_after_secs"] * 1000,
                    critical_ms=settings["critical_after_secs"] * 1000,
                    recovery_ms=settings["recovery_after_secs"] * 1000)
    errors = sample.get("errors", []) if known else []
    store.condition("disk-kernel-errors", "host", "DISK_KERNEL_ERRORS", bool(errors) if known else None,
                    at, evidence={"errors": errors, "window_ms": WINDOW_MS, "scope": "host_current_boot",
                                  "truncated": sample.get("truncated", False),
                                  "action": "Inspect kernel logs and storage path; performance tuning does not repair I/O errors"},
                    warning_ms=0, critical_ms=0, recovery_ms=settings["recovery_after_secs"] * 1000)
    if fresh:
        store.put("last_disk_health_sample_ms", sampled)
