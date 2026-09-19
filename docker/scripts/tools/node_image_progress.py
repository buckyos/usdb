"""Cross-process image preparation observations; never a readiness gate."""

from __future__ import annotations

import hashlib
import json
import math
import os
from pathlib import Path
import sys
import time
import uuid


GROUPS = {"bitcoin": "Bitcoin Core", "runtime": "USDB chain / services"}


def _path(layout):
    return layout.node_env.with_name("image-preparation.json")


def _binding(layout):
    """Do not reuse observations after release activation or reconfiguration."""
    return [layout.release_id, layout.bundle_id,
            hashlib.sha256(layout.node_env.read_bytes()).hexdigest()]


def _process_identity(pid):
    """Distinguish live Linux processes across PID reuse and host restarts."""
    if type(pid) is not int or pid <= 0:
        raise ValueError("Invalid image preparation PID")
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    if fields[0] in {"Z", "X"}:
        raise ValueError("Image preparation process exited")
    return [Path("/proc/sys/kernel/random/boot_id").read_text().strip(), fields[19]]


def _read_record(path):
    if path.is_symlink() or not path.is_file() or path.stat().st_size > 8192:
        return {}
    value = json.loads(path.read_text())
    return value if isinstance(value, dict) else {}


class ImagePreparation:
    """Publish only while the startup process is executing image pulls."""

    def __init__(self, layout):
        self.layout = layout
        self.record = {}

    def __enter__(self):
        try:
            self.record = dict(schema_version="usdb-image-preparation:v1", attempt=uuid.uuid4().hex,
                               binding=_binding(self.layout), pid=os.getpid(),
                               process_identity=_process_identity(os.getpid()),
                               started_monotonic=time.monotonic())
        except (OSError, ValueError, IndexError):
            self._warn()
        return self

    def set_group(self, group):
        if group not in GROUPS:
            raise ValueError("Unknown image preparation group")
        if not self.record:
            return
        from usdb_node import _atomic_write_private
        self.record["group"] = group
        try:
            _atomic_write_private(_path(self.layout), json.dumps(self.record) + "\n")
        except OSError:
            self._warn()

    def __exit__(self, *_exc):
        try:
            # A delayed exit must not remove another startup attempt's observation.
            if self.record and _read_record(_path(self.layout)).get("attempt") == self.record["attempt"]:
                _path(self.layout).unlink()
        except (OSError, ValueError):
            self._warn()

    @staticmethod
    def _warn():
        print("WARNING Image preparation progress could not be recorded; see startup logs", file=sys.stderr)


def read_image_preparation(layout):
    """Ignore obsolete observations without imposing a time limit on slow pulls."""
    try:
        value = _read_record(_path(layout))
        if (value.get("schema_version") != "usdb-image-preparation:v1"
                or value.get("group") not in GROUPS or value.get("binding") != _binding(layout)
                or value.get("process_identity") != _process_identity(value.get("pid"))):
            return None
        started = value.get("started_monotonic")
        now = time.monotonic()
        if type(started) not in (int, float) or not math.isfinite(started) or not 0 <= started <= now:
            return None
        return dict(group=value["group"], elapsed_secs=int(now - started))
    except (OSError, ValueError, IndexError, TypeError):
        return None


def add_image_preparation(layout, report):
    """Augment the dashboard without changing service or controller readiness."""
    preparation = read_image_preparation(layout)
    if preparation is None:
        return report
    report["image_preparation"] = preparation
    report["components"].insert(0, dict(id="images", label="Container images", state="INSTALLING",
        detail="Pulling " + GROUPS[preparation["group"]], current=None, total=None, progress_percent=None,
        progress_phase="images", stage_elapsed_secs=preparation["elapsed_secs"]))
    for item in report["components"]:
        if item["id"] == "bitcoin" and item.get("progress_phase") == "not_started":
            item.update(state="WAITING", detail="Waiting for container images before Core startup")
        if item["id"] == "snapshot" and item["state"] == "WAITING" and item.get("progress_phase") == "waiting_for_core":
            item["detail"] = "Waiting for container images before snapshot download"
    if report["overall_state"] not in {"FAILED", "BLOCKED"}:
        report["overall_state"] = "INSTALLING"
    return report
