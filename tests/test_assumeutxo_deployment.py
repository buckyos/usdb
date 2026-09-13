#!/usr/bin/env python3
"""Exercise native deployment renderers and entrypoints without touching node data."""

import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / "docker/scripts"
CATALOG = ROOT / "src/btc/balance-history/src/bootstrap/checkpoints/mainnet-935000.json"
ORIGIN_HASH = "000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551"


class AssumeutxoDeploymentTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="assumeutxo-deployment-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.bh_root = self.root / "balance-history"
        self.indexer_root = self.root / "indexer"
        self.calls = self.root / "calls.json"
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        for name in ("balance-history", "usdb-indexer"):
            binary = bin_dir / name
            binary.write_text(
                "#!/usr/bin/env python3\n"
                "import json, os, sys\n"
                "from pathlib import Path\n"
                "Path(os.environ['SERVICE_CALLS']).write_text(json.dumps(sys.argv))\n"
                "raise SystemExit(int(os.environ.get('SERVICE_EXIT_CODE', '0')))\n",
                encoding="utf-8",
            )
            binary.chmod(0o755)
        # Deliberately do not inherit an operator's RPC, snapshot or service settings.
        self.env = {
            "PATH": f"{bin_dir}:{os.environ['PATH']}",
            "PYTHONDONTWRITEBYTECODE": "1",
            "SNAPSHOT_MODE": "assumeutxo",
            "BH_ROOT_DIR": str(self.bh_root),
            "USDB_INDEXER_ROOT_DIR": str(self.indexer_root),
            "USDB_GENESIS_BLOCK_HEIGHT": "963800",
            "BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH": ORIGIN_HASH,
            "BH_ASSUMEUTXO_SNAPSHOT_FILE": str(self.root / "not-mounted-utxos.dat"),
            "BTC_AUTH_MODE": "none",
            "SERVICE_CALLS": str(self.calls),
        }

    def run_script(self, name, *args, changes=None):
        return subprocess.run(
            ["bash", str(SCRIPTS / name), *map(str, args)],
            env={**self.env, **(changes or {})}, text=True, capture_output=True,
            timeout=10, check=False,
        )

    def test_native_identity_and_genesis_boundaries(self):
        snapshot = json.loads(CATALOG.read_text())["snapshot"]
        config = self.bh_root / "config.toml"
        for height, block_hash in ((935000, snapshot["base_hash"]), (963800, ORIGIN_HASH), (1000000, "1" * 64)):
            with self.subTest(height=height):
                result = self.run_script("helpers/render_balance_history_config.sh", config, changes={
                    "USDB_GENESIS_BLOCK_HEIGHT": str(height),
                    "BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH": block_hash,
                })
                self.assertEqual(result.returncode, 0, result.stderr)
                document = tomllib.loads(config.read_text())
                self.assertEqual(document["bootstrap"]["identity"], {
                    "snapshot": snapshot, "origin_height": height, "origin_block_hash": block_hash,
                })
                self.assertNotIn("snapshot", document)
                self.assertNotIn("checkpoint", document["bootstrap"])
                self.assertEqual(document["sync"]["local_loader_threshold"], 500)
                self.assertFalse(self.calls.exists())

    def test_invalid_native_inputs_preserve_config_and_do_not_launch(self):
        config = self.bh_root / "config.toml"
        self.bh_root.mkdir()
        config.write_text("existing configuration\n")
        invalid = {
            "USDB_GENESIS_BLOCK_HEIGHT": "934999",
            "BH_ASSUMEUTXO_BASE_HEIGHT": "940000",
            "BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH": "0" * 64,
            "BH_ASSUMEUTXO_SNAPSHOT_FILE": "relative.dat",
            "BTC_NETWORK": "testnet4",
            "INSCRIPTION_SOURCE": "ord",
            "INSCRIPTION_SOURCE_SHADOW_COMPARE": "true",
            "BH_SNAPSHOT_FILE": "/legacy/core.db",
            "BH_SNAPSHOT_MANIFEST": "/legacy/core.json",
            "USDB_INDEXER_CHECKPOINT_MANIFEST": "/legacy/indexer.json",
            "BH_SCRIPT_REGISTRY_ENABLED": "1",
            "BH_SCRIPT_REGISTRY_RECORD_URL": "https://example.invalid/record.json",
            "BH_SCRIPT_REGISTRY_ARTIFACT_ID": "legacy-registry",
            "INSCRIPTION_FIXTURE_FILE": "/legacy/fixture.json",
            "BH_ASSUMEUTXO_IMPORT_BATCH_SIZE": "0",
            "BH_ASSUMEUTXO_REPLAY_BATCH_SIZE": "-1",
            "BH_SYNC_MAX_SYNC_BLOCK_HEIGHT": "963799",
            "BH_SYNC_BATCH_SIZE": "not-an-integer",
            "BH_SYNC_MAX_MEMORY_PERCENT": "100",
            "BH_SCRIPT_REGISTRY_QUERY_BATCH_SIZE": "1001",
            "BH_SCRIPT_REGISTRY_SLOW_QUERY_MS": "0",
            "BTC_AUTH_MODE": "unsupported",
            "BH_RPC_PORT": "65536",
        }
        for key, value in invalid.items():
            with self.subTest(key=key):
                result = self.run_script("entrypoints/start_balance_history.sh", changes={key: value})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("configuration rejected", result.stderr)
                self.assertEqual(config.read_text(), "existing configuration\n")
                self.assertFalse(self.calls.exists())
        mismatch = self.run_script("helpers/render_balance_history_config.sh", config, changes={
            "USDB_GENESIS_BLOCK_HEIGHT": "935000",
        })
        self.assertNotEqual(mismatch.returncode, 0)
        self.assertIn("G=B", mismatch.stderr)
        for key, value in {"BH_ASSUMEUTXO_IMPORT_BATCH_SIZE": "1000001",
                           "BH_ASSUMEUTXO_REPLAY_BATCH_SIZE": "101", "USDB_GENESIS_BLOCK_HEIGHT": "4294967295"}.items():
            with self.subTest(key=key, value=value):
                result = self.run_script("entrypoints/start_balance_history.sh", changes={key: value})
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("configuration rejected", result.stderr)
                self.assertEqual(config.read_text(), "existing configuration\n")
                self.assertFalse(self.calls.exists())

    def test_credentials_and_unicode_roundtrip_with_private_configs(self):
        user, password = 'operator"\\\n', 'fake-credential"\\\t\n\x7f雪🔑'
        for helper, filename, parse, section in (
            ("render_balance_history_config.sh", "config.toml", tomllib.loads, "btc"),
            ("render_usdb_indexer_config.sh", "config.json", json.loads, "bitcoin"),
        ):
            with self.subTest(helper=helper):
                config = self.root / filename
                result = self.run_script(f"helpers/{helper}", config, changes={
                    "BTC_AUTH_MODE": "userpass", "BTC_RPC_USER": user, "BTC_RPC_PASSWORD": password,
                    "BH_ASSUMEUTXO_SNAPSHOT_FILE": str(self.root / '原生🔑"\\.dat'),
                    "BH_ROOT_DIR": str(self.root / "余额🔑"),
                })
                self.assertEqual(result.returncode, 0, result.stderr)
                document = parse(config.read_text())
                self.assertEqual(document[section]["auth"], {"UserPass": [user, password]})
                self.assertEqual(config.stat().st_mode & 0o777, 0o600)
                self.assertNotIn("fake-credential", result.stdout + result.stderr)
                rejected = self.run_script(f"helpers/{helper}", config, changes={
                    "BTC_AUTH_MODE": "userpass", "BTC_RPC_USER": "", "BTC_RPC_PASSWORD": password,
                })
                self.assertNotEqual(rejected.returncode, 0)
                self.assertNotIn("fake-credential", rejected.stdout + rejected.stderr)
                self.assertEqual(parse(config.read_text()), document)

    def test_legacy_installers_skip_all_data_writes_in_native_mode(self):
        for name in ("snapshot_loader.sh", "script_registry_installer.sh"):
            with self.subTest(name=name):
                result = self.run_script(f"entrypoints/{name}")
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertFalse(self.bh_root.exists())
                self.assertFalse(self.indexer_root.exists())
                self.assertFalse(self.calls.exists())
                invalid = self.run_script(f"entrypoints/{name}", changes={"BH_SCRIPT_REGISTRY_ENABLED": "1"})
                self.assertNotEqual(invalid.returncode, 0)
                self.assertFalse(self.bh_root.exists())

    def test_bh_starts_native_service_without_install_marker(self):
        # A TCP listener represents Core RPC availability; no chain or node is started.
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            result = self.run_script("entrypoints/start_balance_history.sh", changes={
                "BTC_RPC_URL": f"http://127.0.0.1:{listener.getsockname()[1]}",
                "WAIT_FOR_BTC_TIMEOUT_SECS": "2", "SERVICE_EXIT_CODE": "17",
            })
        self.assertEqual(result.returncode, 17, result.stderr)
        self.assertEqual(json.loads(self.calls.read_text())[1:], ["--root-dir", str(self.bh_root)])
        self.assertTrue((self.bh_root / "config.toml").is_file())
        self.assertFalse((self.bh_root / "bootstrap/snapshot-loader.done.json").exists())

    def test_indexer_starts_while_upstreams_are_unavailable(self):
        # Zero TCP timeouts would fail the legacy gate even if an endpoint were open.
        result = self.run_script("entrypoints/start_usdb_indexer.sh", changes={
            "BTC_RPC_URL": "http://127.0.0.1:1", "BALANCE_HISTORY_RPC_URL": "http://127.0.0.1:1",
            "WAIT_FOR_BH_TIMEOUT_SECS": "0", "WAIT_FOR_BTC_TIMEOUT_SECS": "0",
            "BH_ASSUMEUTXO_SNAPSHOT_FILE": "", "BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH": "",
        })
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(self.calls.read_text())[1:], ["--root-dir", str(self.indexer_root)])
        config = json.loads((self.indexer_root / "config.json").read_text())
        self.assertEqual(config["usdb"]["genesis_block_height"], 963800)
        self.assertEqual(config["usdb"]["inscription_source"], "bitcoind")
        self.assertFalse(config["usdb"]["inscription_source_shadow_compare"])
        self.assertFalse(self.bh_root.exists())

    def test_indexer_invalid_input_does_not_replace_config_or_launch(self):
        self.indexer_root.mkdir()
        config = self.indexer_root / "config.json"
        config.write_text("existing configuration\n")
        for key, value in {"INSCRIPTION_SOURCE": "ord", "USDB_GENESIS_BLOCK_HEIGHT": "934999",
                           "USDB_INDEXER_RPC_PORT": "0", "USDB_INDEXER_RPC_SERVER_ENABLED": "invalid"}.items():
            with self.subTest(key=key):
                result = self.run_script("entrypoints/start_usdb_indexer.sh", changes={key: value})
                self.assertNotEqual(result.returncode, 0)
                self.assertEqual(config.read_text(), "existing configuration\n")
                self.assertFalse(self.calls.exists())

    def test_packaged_catalog_works_without_rust_source_tree(self):
        scripts = self.root / "image/opt/usdb/docker/scripts"
        tool = scripts / "tools/assumeutxo_bootstrap.py"
        metadata = scripts / "data/assumeutxo/mainnet-935000.json"
        tool.parent.mkdir(parents=True)
        metadata.parent.mkdir(parents=True)
        shutil.copyfile(SCRIPTS / "tools/assumeutxo_bootstrap.py", tool)
        shutil.copyfile(CATALOG, metadata)
        output = self.root / "image-config.toml"
        result = subprocess.run([sys.executable, str(tool), "render", "--output", str(output)],
                                env=self.env, text=True, capture_output=True, check=False, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(tomllib.loads(output.read_text())["bootstrap"]["identity"]["snapshot"],
                         json.loads(CATALOG.read_text())["snapshot"])
        metadata.unlink()
        missing = subprocess.run([sys.executable, str(tool), "render", "--output", str(output)],
                                 env=self.env, text=True, capture_output=True, check=False, timeout=10)
        self.assertNotEqual(missing.returncode, 0)
        self.assertIn("configuration rejected", missing.stderr)


if __name__ == "__main__":
    unittest.main()
