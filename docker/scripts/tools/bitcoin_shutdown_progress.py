"""Bounded shutdown log observations, never evidence of process exit or write progress."""

import argparse
import math
import os
from pathlib import Path
import re
import stat
import time

from bitcoin_import_progress import timestamp


MAX_READ_BYTES = 256 * 1024


def shutdown_stage(message: str) -> str | None:
    """Describe known Core events without treating completed steps as live progress."""
    flush = re.search(r"Flushing large \((\d+) entries\) UTXO set to disk", message)
    if flush:
        return (
            f"Flushing UTXO set to disk ({int(flush[1]):,} entries; "
            "completion percentage unavailable)"
        )
    mempool = re.search(r"Dumped mempool:.*? (\d+) bytes dumped to file", message)
    if mempool:
        return f"Mempool saved ({int(mempool[1]):,} bytes; completed step)"
    if re.search(r"Shutdown(?::)? [Ii]n progress", message):
        return "Stopping Core services"
    if re.search(r"Shutdown(?::)? [Dd]one\b", message):
        return "Core reported shutdown complete; awaiting container exit"
    if "Flushed fee estimates" in message:
        return "Fee estimates saved (completed step)"
    if re.search(r"Writing \d+ mempool transactions to file", message):
        return "Saving mempool transactions"
    if "thread exit" in message:
        return "Worker thread exited (completed step)"
    return None


def read_shutdown_progress(path: Path, since: float, now: float) -> dict:
    """Only use complete, timestamped lines from the current stop request.

    Logs can be missing, rotated or untrusted. Observation failure must not change
    the shutdown lifecycle; a bounded regular-file read also avoids blocking it.
    """
    if not math.isfinite(since) or not math.isfinite(now) or since > now:
        return {}
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode):
                return {}
            offset = max(0, info.st_size - MAX_READ_BYTES)
            source.seek(offset)
            data = source.read(MAX_READ_BYTES)
        if offset:
            data = data.partition(b"\n")[2]
        result = {}
        # Core's default timestamps have second precision; discard partial lines.
        for line in data.decode("utf-8", errors="replace").split("\n")[:-1]:
            parts = line.split(" ", 1)
            observed = timestamp(parts[0])
            if len(parts) != 2 or observed is None:
                continue
            if not math.floor(since) <= observed < math.floor(now) + 1:
                continue
            stage = shutdown_stage(parts[1])
            if stage:
                result = {
                    "stage": stage,
                    "logged_at": parts[0],
                    "age_seconds": max(0, int(now - observed)),
                }
        return result
    except (OSError, ValueError, OverflowError):
        return {}


def render_shutdown_progress(record: dict) -> str:
    """Keep log freshness separate from the shell's total shutdown wait time."""
    if not record:
        return "  Last observed stage: unavailable (no current shutdown-stage log)"
    age = record["age_seconds"]
    elapsed = f"{age // 3600:02d}:{age % 3600 // 60:02d}:{age % 60:02d}"
    return (
        f"  Last observed stage: {record['stage']}\n"
        f"  Last stage log: {record['logged_at']} | No newer stage log for {elapsed}"
    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--log-file", type=Path, required=True)
    parser.add_argument("--since", type=float, required=True)
    args = parser.parse_args()
    print(render_shutdown_progress(read_shutdown_progress(args.log_file, args.since, time.time())))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
