"""Bounded, read-only observations of Core's snapshot import log; never readiness evidence."""

from collections import OrderedDict
from datetime import datetime
import math
import os
from pathlib import Path
import re
import stat


MAX_READ_BYTES = 8 * 1024 * 1024
_CACHE = OrderedDict()
_LOADING = re.compile(r"\[snapshot\] loading (\d+) coins from snapshot ([0-9a-f]{64})")
_PROGRESS = re.compile(r"\[snapshot\] (\d+) coins loaded \((\d+(?:\.\d+)?)%,")
_LOADED = re.compile(r"\[snapshot\] loaded (\d+) \([^)]*\) coins from snapshot ([0-9a-f]{64})")


def timestamp(value):
    """Accept only timezone-qualified times so old log entries cannot look current."""
    try:
        parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
        return parsed.timestamp() if parsed.tzinfo else None
    except (AttributeError, TypeError, ValueError, OverflowError):
        return None


def _observe(line, state, base_hash, since):
    """Track a single attempt, retaining the last count while Core flushes its cache."""
    parts = line.split(" ", 1)
    observed = timestamp(parts[0])
    if len(parts) != 2 or observed is None or observed < since:
        return
    message = parts[1]
    start, end = _LOADING.search(message), _LOADED.search(message)
    if start or end:
        match = start or end
        state.clear()
        if match[2] != base_hash:
            state["other_snapshot"] = True
            return
        count = int(match[1])
        if count <= 0:
            return
        state.update(phase="reading" if start else "flushing", total_coins=count,
                     imported_coins=0 if start else count, progress_percent=0.0 if start else None,
                     stage_started_at=observed, updated_at=observed)
        return
    if state.get("other_snapshot"):
        return
    progress = _PROGRESS.search(message)
    phase = None
    if progress:
        count, percent = int(progress[1]), float(progress[2])
        if not 0 <= percent <= 100 or count <= 0 or count > state.get("total_coins", count):
            return
        state.update(imported_coins=count, progress_percent=percent)
        phase = "reading"
    elif "FlushSnapshotToDisk: flushing coins cache " in message:
        if " started" in message:
            phase = "flushing_cache"
        elif " completed" in message:
            phase = "reading"
    elif "FlushSnapshotToDisk: saving snapshot chainstate " in message:
        if " started" in message:
            phase = "flushing"
        elif " completed" in message:
            phase = "verifying"
    elif "[snapshot] validated snapshot " in message:
        phase = "activating"
    elif f"[snapshot] successfully activated snapshot {base_hash}" in message:
        phase = "activating"
    if phase:
        if phase != state.get("phase"):
            state["stage_started_at"] = observed
        state.update(phase=phase, updated_at=observed)


def read_import_progress(path: Path, base_hash: str, since: float | None) -> dict:
    """Read a bounded tail once, then appended bytes; reset on restart, truncation or rotation.

    `since` binds observations to the current Core process and activation attempt.
    Missing/rotated logs may lose precision; callers must fall back to indeterminate
    progress. This in-memory cache never writes to the Core datadir or its journals.
    """
    if type(since) not in (int, float) or not math.isfinite(since) or since <= 0:
        return {}
    if not re.fullmatch(r"[0-9a-f]{64}", base_hash):
        return {}
    since = math.floor(since)  # Core's default log timestamps have second precision.
    key = (str(path), base_hash, since)
    try:
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            info = os.fstat(source.fileno())
            if not stat.S_ISREG(info.st_mode):
                return {}
            identity = (info.st_dev, info.st_ino)
            cached = _CACHE.get(key)
            if cached is None or cached["identity"] != identity or not 0 <= info.st_size - cached["offset"] <= MAX_READ_BYTES:
                offset = max(0, info.st_size - MAX_READ_BYTES)
                cached = dict(identity=identity, offset=offset, partial=b"", state={})
                source.seek(offset)
                content = source.read(MAX_READ_BYTES)
                if offset:
                    # The first byte may be in the middle of a line.
                    _, _, content = content.partition(b"\n")
            else:
                source.seek(cached["offset"])
                content = cached["partial"] + source.read(MAX_READ_BYTES)
            cached["offset"] = source.tell()
            lines = content.split(b"\n")
            cached["partial"] = lines.pop()[-4096:]
            for line in lines:
                if b"[snapshot]" in line or b"FlushSnapshotToDisk:" in line:
                    _observe(line.decode("utf-8", errors="replace"), cached["state"], base_hash, since)
            _CACHE[key] = cached
            _CACHE.move_to_end(key)
            while len(_CACHE) > 8:
                _CACHE.popitem(last=False)
            result = dict(cached["state"])
            return result if result.get("phase") else {}
    except (OSError, ValueError):
        _CACHE.pop(key, None)
        return {}
