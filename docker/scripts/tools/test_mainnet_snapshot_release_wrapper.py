#!/usr/bin/env python3

from __future__ import annotations

import json
import os
import stat
import subprocess
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]
SNAPSHOT_SCRIPT = REPO_ROOT / "src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh"


class MainnetSnapshotReleaseWrapperTests(unittest.TestCase):
    def setUp(self) -> None:
        self.temporary = tempfile.TemporaryDirectory(prefix="snapshot-release-wrapper-")
        self.root = Path(self.temporary.name)
        self.snapshot_root = self.root / "snapshot"
        self.live_root = self.root / "live"
        self.bitcoin_data = self.root / "bitcoin"
        self.bitcoin_bin = self.root / "bitcoin-bin"
        self.invocations = self.root / "distribution-invocations.jsonl"
        self.snapshot_tool_invocations = self.root / "snapshot-tool-invocations.jsonl"
        self.balance_history_invocations = self.root / "balance-history-invocations.jsonl"
        self.height = 42
        self.height_dir = f"{self.height:012d}"
        self.block_hash = "1" * 64
        self.revision = "a" * 40
        self.file_hash = "3" * 64

        (self.bitcoin_data / "blocks").mkdir(parents=True)
        (self.bitcoin_data / ".cookie").write_text("user:password", encoding="utf-8")
        self.bitcoin_bin.mkdir()
        self._write_executable(
            self.bitcoin_bin / "bitcoin-cli",
            f'''#!/usr/bin/env bash
case "$*" in
  *getblockchaininfo*) printf '%s\n' '{{"chain":"main","blocks":200,"headers":200,"initialblockdownload":false,"pruned":false}}' ;;
  *getblockcount*) printf '%s\n' '200' ;;
  *getblockhash*) printf '%s\n' '{self.block_hash}' ;;
  *) printf 'unexpected bitcoin-cli invocation: %s\n' "$*" >&2; exit 2 ;;
esac
''',
        )
        self._write_executable(
            self.bitcoin_bin / "bitcoind",
            "#!/usr/bin/env bash\nprintf '%s\\n' 'Bitcoin Core version v28.1'\n",
        )
        self.aws = self.root / "aws"
        self._write_executable(self.aws, "#!/usr/bin/env bash\nexit 0\n")
        self.snapshot_tool = self.root / "balance-history-snapshot-tool"
        self._write_executable(
            self.snapshot_tool,
            f'''#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_SNAPSHOT_TOOL_INVOCATIONS"
case "$*" in
  *memory-plan*) printf '%s\n' '{{"source":"test","memory_limit_bytes":1000000,"total_cache_bytes":660000,"utxo_cache_bytes":165000,"balance_cache_bytes":495000,"max_memory_percent":80}}' ;;
  *finalize-artifact*) printf '%s\n' '{{"height":{self.height},"network":"bitcoin","btc_block_hash":"{self.block_hash}","snapshot_id":"{'2' * 64}","artifact_dir":"snapshots/{self.height_dir}/{self.block_hash}","snapshot_file":"snapshot_{self.height}.db","manifest_file":"snapshot_{self.height}.manifest.json","signature_file":"snapshot_{self.height}.manifest.sig","file_sha256":"{self.file_hash}","signing_key_id":"test-signer","trusted_keys_sha256":"{'4' * 64}"}}' ;;
  *) printf 'unexpected snapshot-tool invocation: %s\n' "$*" >&2; exit 2 ;;
esac
''',
        )
        self.balance_history = self.root / "balance-history"
        self._write_executable(
            self.balance_history,
            '''#!/usr/bin/env bash
printf '%s\n' "$*" >>"$FAKE_BALANCE_HISTORY_INVOCATIONS"
exit 0
''',
        )

        target_root = self.snapshot_root / "targets"
        target_root.mkdir(parents=True)
        (target_root / f"{self.height_dir}.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "network": "bitcoin",
                    "height": self.height,
                    "btc_block_hash": self.block_hash,
                    "code_revision": self.revision,
                }
            ),
            encoding="utf-8",
        )
        artifact = self.snapshot_root / "builder/snapshots" / self.height_dir / self.block_hash
        artifact.mkdir(parents=True)
        (artifact / f"snapshot_{self.height}.db").write_bytes(b"test snapshot")
        (artifact / "complete.json").write_text(
            json.dumps(
                {
                    "snapshot_file": f"snapshot_{self.height}.db",
                    "file_sha256": self.file_hash,
                }
            ),
            encoding="utf-8",
        )
        marker = self.snapshot_root / "releases/finalized" / f"{self.height_dir}-{self.block_hash}"
        marker.mkdir(parents=True)
        (marker / "artifact-finalized.json").write_text(
            json.dumps(
                {
                    "version": 1,
                    "height": self.height,
                    "network": "bitcoin",
                    "btc_block_hash": self.block_hash,
                    "snapshot_id": "2" * 64,
                    "snapshot_file": f"snapshot_{self.height}.db",
                    "manifest_file": f"snapshot_{self.height}.manifest.json",
                    "signature_file": f"snapshot_{self.height}.manifest.sig",
                    "file_sha256": self.file_hash,
                    "signing_key_id": "test-signer",
                    "trusted_keys_sha256": "4" * 64,
                    "producer_revision": self.revision,
                    "finalizer_revision": "b" * 40,
                    "finalized_at_utc": "2026-08-30T00:00:00+00:00",
                }
            ),
            encoding="utf-8",
        )
        key_root = self.root / "keys"
        key_root.mkdir()
        (key_root / "test-signer.trusted-keys.json").write_text("{}", encoding="utf-8")

        self.distribution_tool = self.root / "snapshot_distribution.py"
        self.distribution_tool.write_text(
            '''#!/usr/bin/env python3
import json
import os
import pathlib
import sys

command = sys.argv[1]
arguments = sys.argv[2:]
with open(os.environ["FAKE_INVOCATIONS"], "a", encoding="utf-8") as output:
    output.write(json.dumps({"command": command, "arguments": arguments}) + "\\n")

def value(name):
    return arguments[arguments.index(name) + 1]

if command == "prepare":
    output_dir = pathlib.Path(value("--output-dir"))
    output_dir.mkdir(parents=True, exist_ok=True)
    record = output_dir / "snapshot-release-record-test.json"
    record.write_text("{}", encoding="utf-8")
    print(json.dumps({
        "record_path": str(record),
        "record_sha256": "2" * 64,
        "record_url": value("--public-base-url") + "/snapshot-records/v2/" + "2" * 64 + ".json",
        "snapshot_release_id": "test-release",
    }))
elif command == "upload":
    print(json.dumps({"record_url": "https://usdb-snapshot.tbudr.top/snapshot-records/v2/test.json"}))
else:
    raise SystemExit("unexpected command")
''',
            encoding="utf-8",
        )

        self.environment = os.environ.copy()
        self.environment.update(
            {
                "BALANCE_HISTORY_ROOT": str(self.live_root),
                "BALANCE_HISTORY_BIN": str(self.balance_history),
                "BITCOIN_BIN_DIR": str(self.bitcoin_bin),
                "BITCOIN_DATA_DIR": str(self.bitcoin_data),
                "FAKE_INVOCATIONS": str(self.invocations),
                "FAKE_BALANCE_HISTORY_INVOCATIONS": str(self.balance_history_invocations),
                "FAKE_SNAPSHOT_TOOL_INVOCATIONS": str(self.snapshot_tool_invocations),
                "SNAPSHOT_AWS_EXECUTABLE": str(self.aws),
                "SNAPSHOT_KEY_ROOT": str(key_root),
                "SNAPSHOT_ROOT": str(self.snapshot_root),
                "SNAPSHOT_SIGNER_ID": "test-signer",
                "SNAPSHOT_TOOL_BIN": str(self.snapshot_tool),
                "SNAPSHOT_DISTRIBUTION_TOOL": str(self.distribution_tool),
            }
        )

    def tearDown(self) -> None:
        self.temporary.cleanup()

    @staticmethod
    def _write_executable(path: Path, content: str) -> None:
        path.write_text(content, encoding="utf-8")
        path.chmod(path.stat().st_mode | stat.S_IXUSR)

    def _run(self, command: str, *, environment: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            ["bash", str(SNAPSHOT_SCRIPT), command, "--height", str(self.height)],
            cwd=REPO_ROOT,
            env=environment or self.environment,
            check=False,
            capture_output=True,
            text=True,
        )

    def _invocation_values(self) -> list[dict[str, object]]:
        return [json.loads(line) for line in self.invocations.read_text(encoding="utf-8").splitlines()]

    def test_prepare_release_infers_all_finalized_artifact_inputs(self) -> None:
        result = self._run("prepare-release")
        self.assertEqual(result.returncode, 0, result.stderr)
        invocation = self._invocation_values()[0]
        self.assertEqual(invocation["command"], "prepare")
        arguments = invocation["arguments"]
        self.assertEqual(arguments[arguments.index("--producer-revision") + 1], self.revision)
        self.assertEqual(
            arguments[arguments.index("--artifact-dir") + 1],
            str(self.snapshot_root / "builder/snapshots" / self.height_dir / self.block_hash),
        )
        self.assertEqual(
            arguments[arguments.index("--finalization-marker") + 1],
            str(
                self.snapshot_root
                / "releases/finalized"
                / f"{self.height_dir}-{self.block_hash}"
                / "artifact-finalized.json"
            ),
        )
        self.assertEqual(
            arguments[arguments.index("--public-base-url") + 1],
            "https://usdb-snapshot.tbudr.top",
        )

    def test_finalize_checks_hash_and_signature_without_restoring_rocksdb(self) -> None:
        marker = (
            self.snapshot_root
            / "releases/finalized"
            / f"{self.height_dir}-{self.block_hash}"
            / "artifact-finalized.json"
        )
        marker.unlink()
        result = self._run("finalize")
        self.assertEqual(result.returncode, 0, result.stderr)
        finalized = json.loads(marker.read_text(encoding="utf-8"))
        self.assertEqual(finalized["file_sha256"], self.file_hash)
        self.assertEqual(finalized["producer_revision"], self.revision)
        snapshot_calls = self.snapshot_tool_invocations.read_text(encoding="utf-8")
        self.assertIn("finalize-artifact", snapshot_calls)
        self.assertNotIn(" verify ", snapshot_calls)
        self.assertFalse(self.balance_history_invocations.exists())

    def test_validate_install_is_explicit_and_idempotently_attested(self) -> None:
        first = self._run("validate-install")
        self.assertEqual(first.returncode, 0, first.stderr)
        reports = list(
            (self.snapshot_root / "releases/validation-reports").glob(
                f"validate-install-{self.height}-{self.block_hash}-*.json"
            )
        )
        self.assertEqual(len(reports), 1)
        report = reports[0]
        attestation = json.loads(report.read_text(encoding="utf-8"))
        self.assertEqual(attestation["file_sha256"], self.file_hash)
        invocations = self.balance_history_invocations.read_text(encoding="utf-8").splitlines()
        self.assertEqual(len(invocations), 1)
        self.assertIn("install-snapshot", invocations[0])

        second = self._run("validate-install")
        self.assertEqual(second.returncode, 0, second.stderr)
        replayed = self.balance_history_invocations.read_text(encoding="utf-8").splitlines()
        self.assertEqual(replayed, invocations)

    def test_publish_uses_r2_defaults_and_profile_after_prepare(self) -> None:
        result = self._run("publish")
        self.assertEqual(result.returncode, 0, result.stderr)
        invocations = self._invocation_values()
        self.assertEqual([item["command"] for item in invocations], ["prepare", "upload"])
        arguments = invocations[1]["arguments"]
        self.assertEqual(arguments[arguments.index("--bucket") + 1], "usdb-snapshot")
        self.assertEqual(
            arguments[arguments.index("--endpoint-url") + 1],
            "https://87e0bdf811b13ee87fd0bcec7a4fd1e7.r2.cloudflarestorage.com",
        )
        self.assertEqual(arguments[arguments.index("--aws-region") + 1], "auto")
        self.assertEqual(arguments[arguments.index("--aws-profile") + 1], "usdb-snapshot-publisher")
        self.assertEqual(arguments[arguments.index("--s3-upload-concurrency") + 1], "16")
        self.assertEqual(arguments[arguments.index("--s3-chunk-size-mib") + 1], "64")
        self.assertIn("--progress", arguments)
        self.assertEqual(list((self.snapshot_root / "releases").glob("*.tar")), [])

    def test_archive_is_explicit_and_idempotent(self) -> None:
        first = self._run("archive")
        self.assertEqual(first.returncode, 0, first.stderr)
        archive = self.snapshot_root / "releases" / f"balance-history-mainnet-{self.height}-{self.block_hash}.tar"
        checksum = archive.with_suffix(".tar.sha256")
        self.assertTrue(archive.is_file())
        self.assertTrue(checksum.is_file())
        self.assertFalse(self.invocations.exists())

        second = self._run("archive")
        self.assertEqual(second.returncode, 0, second.stderr)
        self.assertIn("Archive already exists and passed checksum verification", second.stdout)

    def test_publish_can_use_standard_aws_environment_without_profile(self) -> None:
        environment = self.environment.copy()
        environment["SNAPSHOT_AWS_PROFILE"] = ""
        result = self._run("publish", environment=environment)
        self.assertEqual(result.returncode, 0, result.stderr)
        arguments = self._invocation_values()[1]["arguments"]
        self.assertNotIn("--aws-profile", arguments)

    def test_prepare_release_rejects_missing_finalization_marker(self) -> None:
        marker = (
            self.snapshot_root
            / "releases/finalized"
            / f"{self.height_dir}-{self.block_hash}"
            / "artifact-finalized.json"
        )
        marker.unlink()
        result = self._run("prepare-release")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Artifact finalization marker is missing", result.stderr)
        self.assertFalse(self.invocations.exists())


if __name__ == "__main__":
    unittest.main()
