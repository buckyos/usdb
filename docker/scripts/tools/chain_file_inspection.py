"""Inspect chain identity and recovery markers without opening the database.

This standalone module also runs on stdin in the pinned chain image when the
host operator cannot traverse files owned by the container's geth process.
"""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
import stat
import sys

SCHEMA = "usdb-chain-file-inspection:v1"


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
