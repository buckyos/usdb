"""Ensure container preflight failures preserve full-bootstrap acceptance evidence."""
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
RUNNER = ROOT / "docker/scripts/entrypoints/start_sourcedao_bootstrap.sh"


class BootstrapRunnerTests(unittest.TestCase):
    def test_runner_preserves_completed_state_on_error_and_disabled_mode(self):
        for mode, scope in [("disabled", "dao-dividend-only"), ("dev-workspace", "full")]:
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                state = root / "state.json"
                original = json.dumps({"scope": "full", "status": "completed", "operations": [{"tx_hash": "retained"}]})
                state.write_text(original)
                env = dict(os.environ, BOOTSTRAP_DIR=directory, SOURCE_DAO_BOOTSTRAP_STATE_FILE=str(state),
                           SOURCE_DAO_BOOTSTRAP_MODE=mode, SOURCE_DAO_BOOTSTRAP_SCOPE=scope,
                           SOURCE_DAO_REPO_DIR=str(root / "missing-repo"))
                env.pop("SOURCE_DAO_BOOTSTRAP_PRIVATE_KEY", None)
                result = subprocess.run(["bash", str(RUNNER)], env=env, capture_output=True, text=True)
                self.assertEqual(result.returncode, 0 if mode == "disabled" else 1, result.stderr)
                self.assertEqual(state.read_text(), original)
                status = json.loads(Path(f"{state}.runner-status.json").read_text())
                self.assertEqual(status["status"], "disabled" if mode == "disabled" else "error")


if __name__ == "__main__":
    unittest.main()
