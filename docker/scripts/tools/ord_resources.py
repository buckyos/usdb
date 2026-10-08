"""Ord application-cache tiers inside a fixed, whole-node-budgeted ceiling."""

import json
from pathlib import Path


class CachePolicy:
    """Use sustained live canonical observations, never height alone, to downshift."""

    def __init__(self, root: Path, catchup: int, steady: int, enabled: bool):
        if not 0 < steady <= catchup:
            raise ValueError("Ord steady cache must be positive and no larger than catch-up cache")
        self.path = root / "resource-profile.json"
        self.catchup, self.steady = catchup, steady
        self.enabled = enabled and catchup != steady
        self.profile = "catchup" if self.enabled else "fixed"
        self.since = None
        self.last_sample = None
        # This file is only a cache-size hint. It never establishes readiness or
        # permits indexing before the current Core prerequisite checks pass.
        try:
            if self.enabled and not self.path.is_symlink() and self.path.stat().st_size <= 1024:
                hint = json.loads(self.path.read_text())
                if isinstance(hint, dict) and hint.get("profile") == "steady":
                    self.profile = "steady"
        except (OSError, ValueError):
            pass

    @property
    def cache(self):
        return self.steady if self.profile == "steady" else self.catchup

    def target(self, report, now):
        """Ignore brief lag/RPC failures; gaps between probes break the evidence window."""
        if not self.enabled:
            return None
        if self.last_sample is not None and not 0 < now - self.last_sample <= 30:
            self.since = None
        self.last_sample = now
        gap = report.get("ord_gap")
        eligible = (report.get("state") == "READY" and report.get("canonical") is True
                    if self.profile == "catchup" else
                    report.get("state") == "INDEXING" and type(gap) is int and gap >= 1000)
        if not eligible:
            self.since = None
            return None
        if self.since is None:
            self.since = now
        delay = 60 if self.profile == "catchup" else 300
        return ("steady" if self.profile == "catchup" else "catchup") if now - self.since >= delay else None

    def apply(self, target):
        """Persist only after the previous Ord writer has exited cleanly."""
        if target not in {"catchup", "steady"}:
            raise ValueError("invalid Ord cache profile")
        self.profile = target
        self.since = self.last_sample = None
        try:
            temporary = self.path.with_suffix(".tmp")
            if self.path.is_symlink() or temporary.is_symlink():
                raise OSError("symlink profile path")
            temporary.write_text(json.dumps(dict(profile=target)) + "\n")
            temporary.replace(self.path)
        except OSError:
            print("Ord resource profile hint could not be saved; next startup may use catch-up cache", flush=True)
