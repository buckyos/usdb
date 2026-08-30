#!/usr/bin/env python3

from __future__ import annotations

import importlib.util
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


MODULE_PATH = Path(__file__).with_name("prepare_release_installer.py")
SPEC = importlib.util.spec_from_file_location("prepare_release_installer", MODULE_PATH)
assert SPEC is not None and SPEC.loader is not None
BUILDER = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = BUILDER
SPEC.loader.exec_module(BUILDER)

RELEASE_ID = "usdb-testnet-v0-r1"


class PrepareReleaseInstallerTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="release-installer-test-")
        self.root = Path(self.temporary.name)
        self.installer = self.root / "install_usdb_node.sh"
        self.manifest = self.root / "usdb-release-manifest.json"
        self.node_kit = self.root / f"{RELEASE_ID}-node-kit.tar.gz"
        self.installer.write_text("#!/usr/bin/env bash\nprintf 'installer:%s\\n' \"$*\"\n", encoding="utf-8")
        self.installer.chmod(0o755)
        self.manifest.write_text("{}\n", encoding="utf-8")
        self.node_kit.write_bytes(b"node-kit")

    def tearDown(self) -> None:
        self.temporary.cleanup()

    def test_builds_executable_release_bound_installer(self) -> None:
        output = self.root / f"install-{RELEASE_ID}.sh"
        result = BUILDER.build_release_installer(
            release_id=RELEASE_ID,
            repository="buckyos/usdb",
            installer_path=self.installer,
            manifest_path=self.manifest,
            node_kit_path=self.node_kit,
            output_path=output,
            release_base_url=self.root.as_uri(),
        )
        self.assertEqual(result, output.resolve())
        content = output.read_text(encoding="utf-8")
        self.assertIn(f"release_id={RELEASE_ID}", content)
        self.assertIn("manifest_sha256=", content)
        self.assertIn("node_kit_sha256=", content)

        completed = subprocess.run(
            [str(output), "--install-root", "/tmp/example"],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(completed.returncode, 0, completed.stderr)
        self.assertIn(f"--release-id {RELEASE_ID}", completed.stdout)
        self.assertIn("--expected-manifest-sha256", completed.stdout)
        self.assertIn("--expected-node-kit-sha256", completed.stdout)

    def test_rejects_wrong_node_kit_name_and_existing_output(self) -> None:
        with self.assertRaisesRegex(ValueError, "node kit must be named"):
            BUILDER.build_release_installer(
                release_id=RELEASE_ID,
                repository="buckyos/usdb",
                installer_path=self.installer,
                manifest_path=self.manifest,
                node_kit_path=self.manifest,
                output_path=self.root / "output.sh",
            )

        output = self.root / "existing.sh"
        output.touch()
        with self.assertRaisesRegex(ValueError, "refusing to replace"):
            BUILDER.build_release_installer(
                release_id=RELEASE_ID,
                repository="buckyos/usdb",
                installer_path=self.installer,
                manifest_path=self.manifest,
                node_kit_path=self.node_kit,
                output_path=output,
            )


if __name__ == "__main__":
    unittest.main()
