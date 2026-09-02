#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import subprocess
import tempfile
import unittest
from pathlib import Path


REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
LOADER = REPOSITORY_ROOT / "docker/scripts/entrypoints/snapshot_loader.sh"


class SnapshotLoaderTests(unittest.TestCase):
    def test_balance_history_import_receives_progress_file_and_writes_marker(self) -> None:
        with tempfile.TemporaryDirectory(prefix="snapshot-loader-test-") as temporary:
            root = Path(temporary)
            service_root = root / "balance-history"
            snapshot = root / "snapshot.db"
            snapshot.write_bytes(b"snapshot")
            bin_dir = root / "bin"
            bin_dir.mkdir()
            invocation = root / "balance-history.args"
            fake_balance_history = bin_dir / "balance-history"
            fake_balance_history.write_text(
                """#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$@" >"${BALANCE_HISTORY_ARGS_FILE:?}"
mkdir -p "${BH_ROOT_DIR:?}/db"
printf 'rocksdb\n' >"${BH_ROOT_DIR}/db/CURRENT"
""",
                encoding="utf-8",
            )
            fake_balance_history.chmod(0o755)
            stale_progress = service_root / "bootstrap/snapshot-loader.progress.json"
            stale_progress.parent.mkdir(parents=True)
            stale_progress.write_text('{"state":"stale"}\n', encoding="utf-8")

            environment = {
                **os.environ,
                "PATH": f"{bin_dir}:{os.environ['PATH']}",
                "SNAPSHOT_MODE": "balance-history",
                "BH_ROOT_DIR": str(service_root),
                "BH_SNAPSHOT_FILE": str(snapshot),
                "BH_SNAPSHOT_MANIFEST": "",
                "BTC_AUTH_MODE": "none",
                "BALANCE_HISTORY_ARGS_FILE": str(invocation),
            }
            result = subprocess.run(
                [str(LOADER)],
                env=environment,
                check=False,
                capture_output=True,
                text=True,
            )

            self.assertEqual(result.returncode, 0, result.stderr)
            arguments = invocation.read_text(encoding="utf-8").splitlines()
            self.assertEqual(
                arguments,
                [
                    "--root-dir",
                    str(service_root),
                    "install-snapshot",
                    "--file",
                    str(snapshot),
                    "--progress-file",
                    str(stale_progress),
                ],
            )
            self.assertFalse(stale_progress.exists())
            marker = json.loads(
                (service_root / "bootstrap/snapshot-loader.done.json").read_text(
                    encoding="utf-8"
                )
            )
            self.assertEqual(marker["snapshot_mode"], "balance-history")
            self.assertEqual(marker["snapshot_file"], str(snapshot))


if __name__ == "__main__":
    unittest.main()
