"""Bounded Ord milestones and live activity without opening its database."""

from __future__ import annotations

import json
import os
from pathlib import Path
import re
import threading
import time

MAX_LOG_BYTES = 2 * 1024**2
FIELDS = ("index_phase", "processing_height", "commit_target_height", "commit_elapsed_secs",
          "last_commit_at_ms", "last_commit_duration_secs", "last_activity_at_ms",
          "process_read_bytes", "process_write_bytes", "sample_read_bytes", "sample_write_bytes",
          "sample_elapsed_secs", "shutdown_elapsed_secs", "ord_exit_code",
          "index_cache_bytes", "commit_interval", "shutdown_started_at_ms", "resource_profile", "restart_reason")
PHASES = {"STARTING", "RECOVERING", "PROCESSING", "COMMITTING", "IDLE"}


class OrdObservation:
    """Keep block logs in memory; persist only transitions and minute heartbeats."""

    def __init__(self, root: Path):
        self.root = root
        self.lock = threading.RLock()
        self.values = {"index_phase": "STARTING"}
        self.commit_started = None
        self.last_log = 0.0
        self.last_io = None
        self.reader = None

    def event(self, kind: str, **values) -> None:
        """Serialize records from the log reader and supervisor during rotation."""
        with self.lock:
            self._event(kind, **values)

    def _event(self, kind: str, **values) -> None:
        """Rotate three operator-readable files independently of Docker removal."""
        record = dict(schema_version="usdb-ord-event:v1", event=kind,
                      observed_at_ms=int(time.time() * 1000), **values)
        line = json.dumps(record, allow_nan=False) + "\n"
        print("Ord milestone: " + line.rstrip(), flush=True)
        try:
            path = self.root / "ord-events.jsonl"
            if path.exists() and path.stat().st_size >= MAX_LOG_BYTES:
                for number in (2, 1):
                    source = path if number == 1 else path.with_name(path.name + ".1")
                    if source.exists():
                        source.replace(path.with_name(path.name + f".{number}"))
            descriptor = os.open(path, os.O_WRONLY | os.O_APPEND | os.O_CREAT | os.O_NOFOLLOW, 0o600)
            with os.fdopen(descriptor, "a", encoding="utf-8") as stream:
                stream.write(line)
        except OSError:
            # Observability must never interrupt a database commit or shutdown.
            print("Ord milestone persistence unavailable; check data directory access/free space", flush=True)

    def _finish_commit(self, evidence: str) -> None:
        if self.commit_started is not None:
            elapsed = int(max(0, time.monotonic() - self.commit_started))
            self.values.update(last_commit_at_ms=int(time.time() * 1000),
                               last_commit_duration_secs=elapsed)
            self.event("commit_finished", height=self.values.get("commit_target_height"),
                       elapsed_secs=elapsed, evidence=evidence)
            self.commit_started = None
            self.values.pop("commit_elapsed_secs", None)

    def consume(self, line: str) -> bool:
        """Recognize pinned Ord 0.29 messages; never persist arbitrary log text."""
        with self.lock:
            block = re.search(r"\bBlock (\d+) at .* with (\d+) transactions", line)
            commit = re.search(r"\bCommitting at block height (\d+), (\d+) outputs traversed, (\d+) in map, (\d+) cached", line)
            if block:
                if self.commit_started is not None:
                    if int(block[1]) > self.values["commit_target_height"]:
                        self._finish_commit("next_block_started")
                    else:
                        self.event("commit_observation_reset", height=int(block[1]), reason="block_replayed")
                        self.commit_started = None
                        self.values.pop("commit_elapsed_secs", None)
                self.values.update(index_phase="PROCESSING", processing_height=int(block[1]))
            elif commit:
                self.commit_started = time.monotonic()
                self.values.update(index_phase="COMMITTING", commit_target_height=max(0, int(commit[1]) - 1))
                self.event("commit_started", height=self.values["commit_target_height"],
                           outputs=int(commit[2]), cached_utxos=int(commit[3]))
            elif "needs recovery. This can take a long time" in line:
                self.values["index_phase"] = "RECOVERING"
                self.event("recovery_started")
            elif "Listening on http" in line:
                if self.values["index_phase"] in {"STARTING", "RECOVERING"}:
                    self.values["index_phase"] = "IDLE"
                self.event("database_opened")
            elif "Wrote " in line and " outputs in " in line:
                return True
            else:
                return False
            self.values["last_activity_at_ms"] = int(time.time() * 1000)
            return True

    def attach(self, child) -> None:
        """Continuously drain the pipe so a verbose index cannot block on logging."""
        def read():
            while True:
                line = child.stdout.readline(16385)
                if not line:
                    break
                if len(line) > 16384:
                    while line and not line.endswith("\n"):
                        line = child.stdout.readline(16385)
                    print("Ord log line exceeded capture limit", flush=True)
                    continue
                if not self.consume(line):
                    print(line.rstrip(), flush=True)
        if getattr(child, "stdout", None) is not None:
            self.reader = threading.Thread(target=read, name="ord-logs", daemon=True)
            self.reader.start()

    def snapshot(self, child=None, committed=None) -> dict:
        """Report actual I/O separately from committed block progress."""
        with self.lock:
            now = time.monotonic()
            if self.commit_started is not None:
                self.values["commit_elapsed_secs"] = int(max(0, now - self.commit_started))
                if type(committed) is int and committed >= self.values["commit_target_height"]:
                    self._finish_commit("committed_height_observed")
                    self.values["index_phase"] = "IDLE"
            if child is not None and getattr(child, "pid", None) is not None:
                try:
                    counters = dict(line.split(": ") for line in Path(f"/proc/{child.pid}/io").read_text().splitlines())
                    read, written = int(counters["read_bytes"]), int(counters["write_bytes"])
                    self.values.update(process_read_bytes=read, process_write_bytes=written)
                    if self.last_io is not None:
                        sampled, old_read, old_write = self.last_io
                        self.values.update(sample_elapsed_secs=max(1, int(now - sampled)),
                                           sample_read_bytes=max(0, read - old_read),
                                           sample_write_bytes=max(0, written - old_write))
                        if read > old_read or written > old_write:
                            self.values["last_activity_at_ms"] = int(time.time() * 1000)
                    self.last_io = now, read, written
                except (OSError, ValueError, KeyError):
                    for key in ("sample_elapsed_secs", "sample_read_bytes", "sample_write_bytes"):
                        self.values.pop(key, None)
                    self.last_io = None
            if now - self.last_log >= 60:
                self.event("activity", committed_height=committed, **self.values)
                self.last_log = now
            return dict(self.values)
