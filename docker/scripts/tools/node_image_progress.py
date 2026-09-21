"""Cross-process image preparation observations; never a readiness gate."""

from __future__ import annotations

import hashlib
from datetime import datetime, timezone
import json
import math
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import uuid


GROUPS = {"bitcoin": "Bitcoin Core", "runtime": "USDB chain / services"}
IMAGES = {"bitcoin": (("USDB_BITCOIN_IMAGE", "btc-node"),),
          "runtime": (("USDB_SERVICES_IMAGE", "balance-history"), ("USDB_CHAIN_IMAGE", "usdb-chain"))}


def _safe_detail(value):
    """Do not persist registry signed URLs, proxy credentials, or terminal controls."""
    value = re.sub(r"https?://[^\s\"']+", "<registry-url>", str(value))
    return " ".join("".join(c for c in value if c.isprintable() or c.isspace()).split())[:800]


def image_cached(reference):
    """Require the exact digest and a locally inspectable linux/amd64 image.

    Engine's containerd inspection selects only manifests whose config and layers
    are available. Listing an image ID alone is not sufficient after partial pulls.
    """
    if not re.fullmatch(r"[a-zA-Z0-9._:/-]+@sha256:[0-9a-f]{64}", reference):
        raise ValueError("Image preparation requires a digest-pinned release image")
    try:
        result = subprocess.run(["docker", "image", "inspect", reference],
                                capture_output=True, text=True, timeout=15)
        if result.returncode:
            return False
        values = json.loads(result.stdout)
        if not isinstance(values, list) or len(values) != 1 or not isinstance(values[0], dict):
            return False
        value = values[0]
        return (reference in value.get("RepoDigests", []) and value.get("Os") == "linux"
                and value.get("Architecture") == "amd64"
                and isinstance(value.get("RootFS"), dict)
                and bool(value["RootFS"].get("Layers")))
    except (OSError, ValueError, TypeError, subprocess.TimeoutExpired):
        return False


def prepare_image_group(layout, group, preparation, *, output_to_stderr, quiet_progress):
    """Reuse complete pinned images; pull only one service per missing image."""
    from usdb_node import read_env, run_helper
    env = read_env(layout.node_env)
    preparation.set_group(group)
    helper = "run_testnet_bitcoin.sh" if group == "bitcoin" else "run_testnet_runtime.sh"
    preparation.output = sys.stderr if output_to_stderr else sys.stdout
    preparation.quiet = quiet_progress
    for index, (key, service) in enumerate(IMAGES[group], 1):
        reference = env[key]
        preparation.begin_image(reference, index, len(IMAGES[group]))
        if image_cached(reference):
            preparation.cached()
            continue
        preparation.begin_pull()
        args = ["pull"] if group == "bitcoin" else ["pull", service]
        run_helper(layout, helper, args, on_output=preparation.observe_line)
        preparation.record["phase"] = "cached"
        preparation._last_line = ""
        preparation._publish(force=True)


def clear_failed_preparation(layout):
    """An explicit startup with --skip-pull dismisses the previous pull failure."""
    try:
        value = _read_record(_path(layout))
        if value.get("phase") == "failed" and value.get("binding") == _binding(layout):
            _path(layout).unlink()
    except (OSError, ValueError):
        pass


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
    """Publish live preparation progress and retain failures for the next attempt."""

    def __init__(self, layout):
        self.layout = layout
        self.record = {}
        self.layers = {}
        self.output = sys.stderr
        self.quiet = False
        self._written = self._logged = -math.inf
        self._last_line = ""
        self._warned = False

    def __enter__(self):
        try:
            binding = _binding(self.layout)
            identity = _process_identity(os.getpid())
            self.record = dict(schema_version="usdb-image-preparation:v1", attempt=uuid.uuid4().hex,
                               binding=binding, pid=os.getpid(), process_identity=identity,
                               started_monotonic=time.monotonic(), pull_attempts={}, retry_count=0)
            previous = _read_record(_path(self.layout))
            if (previous.get("binding") == binding and isinstance(previous.get("process_identity"), list)
                    and previous["process_identity"][:1] == identity[:1]):
                attempts = previous.get("pull_attempts", {})
                if isinstance(attempts, dict) and all(type(n) is int and 0 < n < 1000000 for n in attempts.values()):
                    self.record["pull_attempts"] = attempts
                    self.record["retry_count"] = sum(max(0, count - 1) for count in attempts.values())
                    for key in ("last_error", "last_error_at"):
                        if isinstance(previous.get(key), str):
                            self.record[key] = previous[key]
                    started = previous.get("started_monotonic")
                    if type(started) in (int, float) and 0 <= started <= time.monotonic():
                        self.record["started_monotonic"] = started
        except (OSError, ValueError, IndexError):
            self._warn()
        return self

    def set_group(self, group):
        if group not in GROUPS:
            raise ValueError("Unknown image preparation group")
        if not self.record:
            return
        self.record["group"] = group
        self.record["phase"] = "checking"
        self._publish(force=True)

    def _publish(self, *, force=False):
        """Throttle atomic progress writes; stage/error transitions are immediate."""
        from usdb_node import _atomic_write_private
        now = time.monotonic()
        if not self.record or (not force and now - self._written < 1):
            return
        try:
            _atomic_write_private(_path(self.layout), json.dumps(self.record) + "\n")
            self._written = now
        except OSError:
            self._warn()

    def begin_image(self, reference, index, count):
        self.layers = {}
        self._last_line = ""
        self.record.update(image=reference, image_index=index, image_count=count, phase="checking",
                           downloaded_bytes=None, download_total_bytes=None, completed_layers=0,
                           layer_count=0, reused_layers=0, image_attempt=0)
        self._publish(force=True)

    def cached(self):
        self.record["phase"] = "cached"
        print(f"Using cached image {self.record['image']}; no registry pull needed", file=self.output, flush=True)
        self._publish(force=True)

    def begin_pull(self):
        attempts = self.record.setdefault("pull_attempts", {})
        reference = self.record["image"]
        attempts[reference] = attempts.get(reference, 0) + 1
        self.record.update(phase="pulling", image_attempt=attempts[reference],
                           retry_count=sum(max(0, count - 1) for count in attempts.values()))
        print(f"Pulling {reference} (attempt {attempts[reference]})", file=self.output, flush=True)
        self._publish(force=True)

    def observe_line(self, line):
        """Consume Compose JSON events; counts describe this image's observed layers."""
        previous_bytes = self.record.get("downloaded_bytes")
        try:
            event = json.loads(line)
        except ValueError:
            event = None
        if not isinstance(event, dict) or not isinstance(event.get("text"), str):
            if line.strip():
                self._last_line = _safe_detail(line)
                print(self._last_line, file=self.output, flush=True)
            return
        text = event["text"]
        layer_id = event.get("id", "")
        if (isinstance(layer_id, str) and re.fullmatch(r"[0-9a-f]{12,64}", layer_id)
                and (layer_id in self.layers or len(self.layers) < 512)):
            layer = self.layers.setdefault(layer_id, dict(current=None, total=None, done=False, reused=False))
            if text == "Downloading":
                layer.update(done=False, reused=False)
                for source, key in (("current", "current"), ("total", "total")):
                    value = event.get(source)
                    if type(value) is int and value >= 0:
                        layer[key] = value
            elif text in {"Download complete", "Pull complete", "Already exists"}:
                layer["done"] = True
                layer["reused"] = layer["reused"] or text == "Already exists"
                if layer["total"]:
                    layer["current"] = layer["total"]
            downloaded = [item for item in self.layers.values() if item["current"] is not None and not item["reused"]]
            pending = [item for item in self.layers.values() if not item["reused"]]
            self.record.update(
                downloaded_bytes=sum(item["current"] for item in downloaded) if downloaded else None,
                download_total_bytes=sum(item["total"] for item in pending) if pending and all(item["total"] for item in pending) else None,
                completed_layers=sum(item["done"] for item in self.layers.values()),
                reused_layers=sum(item["reused"] for item in self.layers.values()), layer_count=len(self.layers))
        message = _safe_detail(" ".join(str(event.get(k, "")) for k in ("text", "details", "status")))
        error = text.lower() in {"error", "warning"} or event.get("status") == "Error"
        if error:
            self._last_line = message
            self.record.update(last_error=message, last_error_at=datetime.now(timezone.utc).isoformat(timespec="seconds"))
        now = time.monotonic()
        if error or (not self.quiet and now - self._logged >= 10):
            print(f"Image {layer_id}: {message}", file=self.output, flush=True)
            self._logged = now
        first_bytes = previous_bytes is None and self.record.get("downloaded_bytes") is not None
        self._publish(force=error or first_bytes)

    def __exit__(self, exc_type, exc, _traceback):
        try:
            # A delayed exit must not remove another startup attempt's observation.
            if self.record.get("attempt") and _read_record(_path(self.layout)).get("attempt") == self.record["attempt"]:
                if exc_type is None or self.record.get("phase") == "cached":
                    _path(self.layout).unlink()
                else:
                    self.record.update(phase="failed", last_error=self._last_line or _safe_detail(exc),
                                       finished_monotonic=time.monotonic(),
                                       last_error_at=datetime.now(timezone.utc).isoformat(timespec="seconds"))
                    self._publish(force=True)
        except (OSError, ValueError):
            self._warn()

    def _warn(self):
        if not self._warned:
            print("WARNING Image preparation progress could not be recorded; see startup logs", file=sys.stderr)
            self._warned = True


def read_image_preparation(layout):
    """Ignore obsolete observations without imposing a time limit on slow pulls."""
    try:
        value = _read_record(_path(layout))
        phase = value.get("phase", "pulling")
        if (value.get("schema_version") != "usdb-image-preparation:v1"
                or value.get("group") not in GROUPS or value.get("binding") != _binding(layout)
                or phase not in {"checking", "cached", "pulling", "failed"}):
            return None
        if phase == "failed":
            if value.get("process_identity", [None])[0] != _process_identity(os.getpid())[0]:
                return None
        elif value.get("process_identity") != _process_identity(value.get("pid")):
            return None
        started = value.get("started_monotonic")
        now = time.monotonic()
        if type(started) not in (int, float) or not math.isfinite(started) or not 0 <= started <= now:
            return None
        finished = value.get("finished_monotonic")
        if phase == "failed" and type(finished) in (int, float) and started <= finished <= now:
            now = finished
        result = dict(group=value["group"], elapsed_secs=int(now - started), phase=phase)
        for key in ("image", "last_error", "last_error_at"):
            if isinstance(value.get(key), str):
                result[key] = _safe_detail(value[key])
        for key in ("downloaded_bytes", "download_total_bytes", "completed_layers", "reused_layers",
                    "layer_count", "retry_count", "image_attempt", "image_index", "image_count"):
            if type(value.get(key)) is int and value[key] >= 0:
                result[key] = value[key]
        return result
    except (OSError, ValueError, IndexError, TypeError):
        return None


def add_image_preparation(layout, report):
    """Augment the dashboard without changing service or controller readiness."""
    preparation = read_image_preparation(layout)
    if preparation is None:
        return report
    report["image_preparation"] = preparation
    phase = preparation["phase"]
    detail = {"checking": "Checking local images for ", "cached": "Using cached images for ",
              "pulling": "Pulling ", "failed": "Image preparation failed for "}[phase] + GROUPS[preparation["group"]]
    report["components"].insert(0, dict(id="images", label="Container images",
        state="FAILED" if phase == "failed" else "INSTALLING", detail=detail,
        current=None, total=None, progress_percent=None, image_download=preparation,
        progress_phase="images", stage_elapsed_secs=preparation["elapsed_secs"]))
    for item in report["components"]:
        if item["id"] == "bitcoin" and item.get("progress_phase") == "not_started":
            item.update(state="WAITING", detail="Waiting for container images before Core startup")
        if item["id"] == "snapshot" and item["state"] == "WAITING" and item.get("progress_phase") == "waiting_for_core":
            item["detail"] = "Waiting for container images before snapshot download"
    if report["overall_state"] not in {"FAILED", "BLOCKED"}:
        report["overall_state"] = "FAILED" if phase == "failed" else "INSTALLING"
    return report
