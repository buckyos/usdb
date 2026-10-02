#!/usr/bin/env python3
"""Exercise frozen scope selection, renderers and launcher isolation without live nodes."""
import copy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
TOOLS = ROOT / "docker/scripts/tools"
sys.path.insert(0, str(TOOLS))
import registry_scope as scope
from validate_network_bundle import read_env, validate_network_bundle


class RegistryScopeDeploymentTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-registry-scope-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.catalog = self.root / "artifacts/registry.json"
        self.catalog.parent.mkdir()
        self.catalog.write_text('{"catalog": "frozen-fixture"}\n')
        self.network = dict(btc_source=dict(network_id="btc-mainnet", rules_scope="usdb-testnet-v1",
                                           activation_registry_id="a" * 64), artifacts={
            scope.CATALOG_ARTIFACT: dict(path="artifacts/registry.json",
                                        sha256=hashlib.sha256(self.catalog.read_bytes()).hexdigest())})
        self.env = {"PATH": os.environ["PATH"], "PYTHONDONTWRITEBYTECODE": "1", "BTC_AUTH_MODE": "none",
                    "USDB_INDEXER_ROOT_DIR": str(self.root / "indexer"), "USDB_GENESIS_BLOCK_HEIGHT": "963800",
                    scope.SCOPE_ENV: "usdb-testnet-v1", scope.REGISTRY_ENV: "a" * 64,
                    scope.CATALOG_ENV: str(self.catalog)}

    def render(self, mode, changes=None):
        output = self.root / "config.json"
        result = subprocess.run(["bash", str(ROOT / "docker/scripts/helpers/render_usdb_indexer_config.sh"), str(output)],
                                env={**self.env, "SNAPSHOT_MODE": mode, **(changes or {})},
                                text=True, capture_output=True, timeout=10, check=False)
        return result, output

    def test_both_renderers_preserve_scope_pin_and_catalog(self):
        for mode in ("none", "assumeutxo"):
            with self.subTest(mode=mode):
                result, output = self.render(mode)
                self.assertEqual(result.returncode, 0, result.stderr)
                config = json.loads(output.read_text())["usdb"]
                self.assertEqual(config["rules_scope"], "usdb-testnet-v1")
                self.assertEqual(config["activation_registry_id"], "a" * 64)
                self.assertEqual(config["activation_registry_catalog_file"], str(self.catalog))

    def test_incomplete_scoped_selection_preserves_existing_config(self):
        for mode in ("none", "assumeutxo"):
            for change in ({scope.REGISTRY_ENV: ""}, {scope.CATALOG_ENV: ""},
                           {scope.SCOPE_ENV: ""}, {scope.SCOPE_ENV: "legacy"}, {scope.SCOPE_ENV: "bad--scope"},
                           {scope.CATALOG_ENV: "relative.json"}, {scope.REGISTRY_ENV: "BAD"}):
                with self.subTest(mode=mode, change=change):
                    output = self.root / "config.json"
                    output.write_text("existing config")
                    result, output = self.render(mode, change)
                    self.assertNotEqual(result.returncode, 0)
                    self.assertEqual(output.read_text(), "existing config")

    def test_legacy_without_scope_preserves_frozen_registry_pin(self):
        for mode in ("none", "assumeutxo"):
            result, output = self.render(mode, {scope.SCOPE_ENV: "", scope.CATALOG_ENV: ""})
            self.assertEqual(result.returncode, 0, result.stderr)
            config = json.loads(output.read_text())["usdb"]
            self.assertIsNone(config["rules_scope"])
            self.assertIsNone(config["activation_registry_catalog_file"])
            self.assertEqual(config["activation_registry_id"], "a" * 64)

    def test_scope_token_matches_core_contract(self):
        for value in ("legacy", "1", "usdb-testnet-v1", "a" * 64):
            self.assertEqual(scope.validate_scope(value), value)
        for value in ("", "UPPER", "with_underscore", "a--b", "-a", "a-", "a" * 65, None):
            with self.subTest(value=value), self.assertRaises(ValueError):
                scope.validate_scope(value)

    def test_frozen_catalog_requires_hash_and_container_path(self):
        env = scope.frozen_rule_environment(self.network)
        self.assertEqual(env[scope.CATALOG_ENV], "/network/registry.json")
        scope.validate_frozen_rule_selection(self.root, self.network, env)
        for change in ({scope.SCOPE_ENV: "another-scope"}, {scope.CATALOG_ENV: str(self.catalog)},
                       {scope.REGISTRY_ENV: "b" * 64}):
            with self.subTest(change=change), self.assertRaisesRegex(ValueError, "frozen registry selector mismatch"):
                scope.validate_frozen_rule_selection(self.root, self.network, {**env, **change})
        self.catalog.write_text("changed")
        with self.assertRaisesRegex(ValueError, "SHA-256 mismatch"):
            scope.validate_frozen_rule_selection(self.root, self.network, env)

    def test_catalog_cannot_escape_readonly_artifacts_mount(self):
        for relative in ("../registry.json", "/host/registry.json", "artifacts/../registry.json", "elsewhere/registry.json", "artifacts//registry.json", None):
            network = copy.deepcopy(self.network)
            network["artifacts"][scope.CATALOG_ARTIFACT]["path"] = relative
            with self.subTest(relative=relative), self.assertRaises(ValueError):
                scope.frozen_rule_environment(network)
        self.catalog.unlink()
        outside = self.root / "outside.json"
        outside.write_text("{}")
        with tempfile.TemporaryDirectory() as other:
            foreign = Path(other) / "registry.json"
            foreign.write_text("{}")
            self.catalog.symlink_to(foreign)
            with self.assertRaisesRegex(ValueError, "escapes"):
                scope.validate_frozen_rule_selection(self.root, self.network, scope.frozen_rule_environment(self.network))

    def test_legacy_network_cannot_select_external_catalog(self):
        for value in ("legacy", None):
            network = copy.deepcopy(self.network)
            if value is None:
                del network["btc_source"]["rules_scope"]
            else:
                network["btc_source"]["rules_scope"] = value
            with self.subTest(scope=value), self.assertRaisesRegex(ValueError, "Legacy networks"):
                scope.frozen_rule_environment(network)

    def test_checked_in_v0_keeps_legacy_identity_and_registry(self):
        bundle = ROOT / "docker/networks/testnet-v0"
        network = validate_network_bundle(bundle)
        self.assertEqual(scope.rules_scope_identity(network["btc_source"]), {})
        scope.validate_frozen_rule_selection(bundle, network, read_env(bundle / "network.env"))

    def test_launcher_ignores_shell_registry_overrides(self):
        bin_dir = self.root / "bin"
        bin_dir.mkdir()
        output = self.root / "docker-env.json"
        docker = bin_dir / "docker"
        docker.write_text("#!/usr/bin/env python3\nimport json, os\nfrom pathlib import Path\n"
                          "Path(os.environ['TEST_OUTPUT']).write_text(json.dumps({key: os.environ.get(key) for key in "
                          + repr(scope.SELECTOR_ENV_KEYS) + "}))\n")
        docker.chmod(0o755)
        node_env = self.root / "node.env"
        node_env.write_text("")
        result = subprocess.run(["bash", str(TOOLS / "run_testnet_runtime.sh"), "ps"],
                                env={**self.env, "PATH": f"{bin_dir}:{os.environ['PATH']}",
                                     "USDB_TESTNET_NODE_ENV": str(node_env), "TEST_OUTPUT": str(output)},
                                text=True, capture_output=True, timeout=10, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(output.read_text()), dict.fromkeys(scope.SELECTOR_ENV_KEYS))


if __name__ == "__main__":
    unittest.main()
