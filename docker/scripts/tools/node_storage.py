"""Read-only accounting for reusable Bitcoin data during first node setup."""

import os
from pathlib import Path
import stat


# Even a large retained Core store must leave room for fresh BH/indexer stores,
# temporary bootstrap work and subsequent growth. Optional Ord is additional.
MIN_REBUILD_FREE_BYTES = 512 * 1024**3
BITCOIN_STORES = ("blocks", "chainstate", "chainstate_snapshot")


def retained_bitcoin_bytes(root: Path, bitcoin: Path, marker_name: str, expected_marker: str) -> tuple[int, str]:
    """Count allocated Core store bytes on the root filesystem, without opening DBs.

    Only an identity-matching dataset can reduce the first-install free-space
    requirement. Logs, wallets, optional indexes and raw snapshots are excluded.
    Unreadable trees receive no credit; links and other filesystems are skipped.
    This is capacity accounting, not validation of Bitcoin database contents.
    """
    try:
        candidate = root
        for part in bitcoin.relative_to(root).parts:
            candidate /= part
            if candidate.is_symlink():
                return 0, "Bitcoin path contains a symlink; no retained-data allowance applied."
        marker = bitcoin / marker_name
        if not marker.exists():
            return 0, "Bitcoin dataset identity is missing; no retained-data allowance applied."
        if marker.is_symlink() or not marker.is_file() or marker.read_text(encoding="utf-8") != expected_marker:
            return 0, "Bitcoin dataset identity does not match; no retained-data allowance applied."
        device = root.stat().st_dev
        if bitcoin.stat().st_dev != device:
            return 0, "Bitcoin data is on another filesystem; no retained-data allowance applied."
        total = 0
        seen = set()
        pending = [bitcoin / name for name in BITCOIN_STORES if (bitcoin / name).exists()]
        while pending:
            path = pending.pop()
            info = path.lstat()
            if info.st_dev != device or stat.S_ISLNK(info.st_mode):
                continue
            if stat.S_ISDIR(info.st_mode):
                with os.scandir(path) as entries:
                    pending.extend(Path(entry.path) for entry in entries)
            elif stat.S_ISREG(info.st_mode) and info.st_nlink == 1:
                key = (info.st_dev, info.st_ino)
                if key not in seen:
                    seen.add(key)
                    # Sparse files and preallocation beyond EOF are not usable
                    # retained payload. Linux reports allocation in 512-byte units.
                    total += min(info.st_size, info.st_blocks * 512)
        return total, ""
    except OSError as error:
        return 0, f"Cannot measure retained Bitcoin data ({error.strerror}); no retained-data allowance applied."
    except UnicodeError:
        return 0, "Bitcoin dataset identity is unreadable; no retained-data allowance applied."


def storage_hint(path: Path, sys_root: Path = Path("/sys")) -> dict:
    """Use backing-device hints, including partitions and device-mapper slaves.

    A virtual disk's rotational flag is only a hint, not a performance test.
    Unknown or mixed topology must not be advertised as a proven fast disk.
    """
    requested = path.expanduser().resolve()
    existing = requested
    while not existing.exists() and existing != existing.parent:
        existing = existing.parent
    devices, flags, unknown = [], [], False
    try:
        device = existing.stat().st_dev
        pending = [sys_root / "dev/block" / f"{os.major(device)}:{os.minor(device)}"]
        seen = set()
        while pending:
            current = pending.pop().resolve()
            if current in seen:
                continue
            seen.add(current)
            if len(seen) > 128:
                raise ValueError("block device topology is too large")
            if (current / "partition").exists():
                current = current.parent
            slaves = list((current / "slaves").iterdir()) if (current / "slaves").is_dir() else []
            if slaves:
                pending.extend(slaves)
                continue
            devices.append(current.name)
            try:
                flag = (current / "queue/rotational").read_text().strip()
            except OSError:
                unknown = True
                continue
            flags.append(flag)
            unknown |= flag not in {"0", "1"}
    except (OSError, ValueError):
        unknown = True
    kind = "rotational" if "1" in flags else "unknown" if unknown or not flags else "non-rotational"
    return dict(path=str(requested), devices=sorted(set(devices)), kind=kind,
                recommended_profile="slow-disk" if kind == "rotational" else "balanced")
