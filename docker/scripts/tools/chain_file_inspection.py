"""Inspect chain identity and recovery markers without opening the database.

This standalone module also runs on stdin in the pinned chain image when the
host operator cannot traverse files owned by the container's geth process.
"""
from __future__ import annotations

import hashlib
import json
import os
from pathlib import Path
import re
import stat
import sys
from datetime import datetime

SCHEMA = "usdb-chain-file-inspection:v1"
INCIDENT_SCHEMA = "usdb-deep-btc-reorg-incident:v1"
MAX_INCIDENT_BYTES = 256 * 1024


def incident_report(root: Path) -> dict:
    """Observe a durable halt without exporting RPC URLs, errors or arbitrary JSON.

    Marker presence is itself a stop condition in the runtime. Invalid metadata
    must therefore never hide a latched incident. No observation acknowledges,
    removes or rewrites the source record, including legacy v1 records.
    """
    path = root / "recovery/deep-btc-reorg/halted.json"
    try:
        metadata = path.lstat()
    except FileNotFoundError:
        return {"status": "available", "events": []}
    event = {"event_id": None, "code": "DEEP_REORG_HALTED", "service": "usdb_chain",
             "severity": "critical", "recovery": "manual_intervention", "latched": True,
             "source": "deep_btc_reorg_marker", "detected_at": None,
             "evidence_status": "invalid"}
    try:
        if not stat.S_ISREG(metadata.st_mode) or metadata.st_size > MAX_INCIDENT_BYTES:
            raise ValueError("invalid incident file")
        descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
        with os.fdopen(descriptor, "rb") as source:
            if not stat.S_ISREG(os.fstat(source.fileno()).st_mode):
                raise ValueError("invalid incident file")
            data = source.read(MAX_INCIDENT_BYTES + 1)
        if len(data) > MAX_INCIDENT_BYTES:
            raise ValueError("incident file exceeds limit")
        # Older images have no UUID. A content fingerprint is stable across
        # observations and restarts; consumers must scope it by node identity.
        event["event_id"] = "legacy-sha256:" + hashlib.sha256(data).hexdigest()
        value = json.loads(data, object_pairs_hook=_object)
        if not isinstance(value, dict) or value.get("schema_version") != INCIDENT_SCHEMA:
            raise ValueError("invalid incident schema")
        reason = value.get("reason")
        if reason not in ("upstream_reorg_epoch_advanced", "upstream_reorg_epoch_regressed"):
            raise ValueError("invalid incident reason")
        epochs = {key: value.get(key) for key in ("baseline_epoch", "observed_epoch")}
        if any(type(v) is not int or not 0 <= v <= 2**64 - 1 for v in epochs.values()):
            raise ValueError("invalid incident epochs")
        if ((reason.endswith("advanced") and epochs["observed_epoch"] <= epochs["baseline_epoch"])
                or (reason.endswith("regressed") and epochs["observed_epoch"] >= epochs["baseline_epoch"])):
            raise ValueError("inconsistent incident epochs")
        detected = value.get("detected_at")
        if not isinstance(detected, str) or len(detected) > 40 or datetime.fromisoformat(detected.replace("Z", "+00:00")).tzinfo is None:
            raise ValueError("invalid incident timestamp")
        event.update(epochs, reason=reason, detected_at=detected, evidence_status="available")
        if re.fullmatch(r"[0-9a-f]{32}", str(value.get("incident_id", ""))):
            event["event_id"] = value["incident_id"]
    except PermissionError:
        # Presence was already established. Losing access to its contents must
        # not erase the known halt; directory traversal failures above can still
        # use the caller's read-only container fallback.
        event["evidence_status"] = "unavailable"
    except (OSError, ValueError, UnicodeError, RecursionError):
        pass
    return {"status": "available", "events": [event]}


def _exists(path: Path, *, regular: bool = False) -> bool:
    """Keep permission errors visible on Python versions where Path.exists hides them."""
    try:
        info = path.stat()
    except (FileNotFoundError, NotADirectoryError):
        return False
    return stat.S_ISREG(info.st_mode) if regular else True


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _object(pairs):
    value = {}
    for key, item in pairs:
        if key in value:
            raise ValueError(f"duplicate recovery JSON key: {key}")
        value[key] = item
    return value


def inspect_files(root: Path, action: str, marker_name: str) -> dict:
    """Return metadata and digests only; never emit nodekey bytes or write a file."""
    if action == "incidents":
        return incident_report(root)
    if action == "binding":
        data, key = root / "geth/chaindata", root / "geth/nodekey"
        if not _exists(data / "CURRENT", regular=True) or not _exists(key, regular=True):
            return {"initialized": False}
        if not marker_name or Path(marker_name).name != marker_name:
            raise ValueError("invalid dataset marker name")
        info = data.stat()
        return {"initialized": True, "data_device": info.st_dev, "data_inode": info.st_ino,
                "dataset_sha256": _sha256(root / marker_name), "node_key_sha256": _sha256(key)}
    if action == "guard":
        guard = root / "recovery/deep-btc-reorg"
        halted = _exists(guard / "halted.json")
        present = not halted and _exists(guard / "baseline.json")
        epoch = None
        if present:
            baseline = json.loads((guard / "baseline.json").read_text(), object_pairs_hook=_object)
            if not isinstance(baseline, dict):
                raise ValueError("recovery baseline must be an object")
            epoch = baseline.get("upstream_reorg_epoch")
            if type(epoch) is not int or epoch < 0:
                raise ValueError("recovery baseline has an invalid upstream_reorg_epoch")
        return {"halted": halted, "baseline_present": present, "baseline_epoch": epoch}
    raise ValueError(f"unsupported chain file inspection: {action}")


if __name__ == "__main__":
    try:
        if len(sys.argv) != 4:
            raise ValueError("expected action, data root and dataset marker name")
        action, root, marker = sys.argv[1:]
        print(json.dumps({"schema_version": SCHEMA, "action": action,
                          "result": inspect_files(Path(root), action, marker)}, sort_keys=True))
    except (OSError, ValueError) as error:
        print(f"Chain file inspection failed: {error}", file=sys.stderr)
        sys.exit(1)
