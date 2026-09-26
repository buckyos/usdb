#!/usr/bin/env python3
"""Accept release seed defaults without changing existing node or chain identities."""
from contextlib import redirect_stdout
from dataclasses import replace
import io
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import peer_sources as sources
import usdb_node as node
import usdb_mining as mining
import usdb_peers as peers
import usdb_p2p as p2p
from common.bootnodes import write_bootnodes
from common.enode import V4, V6, DNS
from common.native_node import native_kit
from common.p2p import HOST
from common.p2p_setup import SetupFixture
from common.peers import PeerFixture


class BootnodesCatalogTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.bundle = Path(temporary.name)

    def load(self):
        return sources.load_bootnodes(self.bundle, "usdb-testnet-v0")

    def test_catalog_normalizes_deduplicates_and_never_resolves_dns(self):
        write_bootnodes(self.bundle, [V4, V6, DNS.replace("seed.example.org", "SEED.EXAMPLE.ORG."), DNS])
        with mock.patch.object(socket, "getaddrinfo", side_effect=AssertionError("unexpected DNS")):
            self.assertEqual(self.load(), [V4, V6, DNS])
            self.assertEqual(sources.resolve_bootnodes(self.bundle, "usdb-testnet-v0", None), ",".join([V4, V6, DNS]))
            self.assertEqual(sources.resolve_bootnodes(self.bundle, "usdb-testnet-v0", ""), "")
            self.assertEqual(sources.resolve_bootnodes(self.bundle, "usdb-testnet-v0", V4), V4)

    def test_absent_legacy_catalog_and_explicit_empty_catalog(self):
        self.assertEqual(self.load(), [])
        write_bootnodes(self.bundle, [])
        self.assertEqual(self.load(), [])

    def test_rejects_wrong_network_schema_shape_or_endpoint(self):
        path = write_bootnodes(self.bundle, [V4])
        valid = json.loads(path.read_text())
        bad = [None, [], {}, {**valid, "schema_version": "future"},
               {**valid, "network_bundle_id": "usdb-mainnet-v0"}, {**valid, "extra": True},
               *({**valid, "bootnodes": value} for value in
                 (None, V4, [None], [""], [V4 + "/path"], [V4 + "," + DNS], [V4] * 65))]
        for value in bad:
            with self.subTest(value=value):
                path.write_text(json.dumps(value))
                with self.assertRaisesRegex(ValueError, "INVALID_BOOTNODES_CONFIG"):
                    self.load()

    def test_rejects_duplicate_fields_oversized_files_and_symlinks(self):
        path = self.bundle / sources.BOOTNODES_FILE
        for content in ('{"bootnodes": [], "bootnodes": []}', "[", " " * (sources.MAX_BOOTNODES_BYTES + 1)):
            path.write_text(content)
            with self.assertRaisesRegex(ValueError, "INVALID_BOOTNODES_CONFIG"):
                self.load()
        path.unlink()
        path.symlink_to(self.bundle / "missing")
        with self.assertRaisesRegex(ValueError, "symlinks are not allowed"):
            self.load()

    def test_setup_default_custom_and_none_for_both_roles(self):
        for role in ("full", "bootnode"):
            for choice, expected in (("", V4 + "," + DNS), ("default", V4 + "," + DNS), ("none", ""), (V6, V6)):
                with self.subTest(role=role, choice=choice), SetupFixture() as f:
                    write_bootnodes(f.layout.bundle_dir, [V4, DNS])
                    answers = {"Host data root": str(f.root / "data"), "Node role": role,
                               "Seed enode(s)": choice, "Provide full Explorer": "n",
                               "Enable local minting": "n", "Expose Bitcoin": "n"}
                    node.setup_node(f.layout, input_fn=lambda prompt: next(
                        (value for prefix, value in answers.items() if prompt.startswith(prefix)), ""), output=f.output)
                    self.assertEqual(f.configure.call_args.kwargs["bootnodes"], expected)
                    self.assertIn("Default network seeds (2)", f.output.getvalue())
                    if choice == "none":
                        self.assertIn("SEED_REQUIRED", f.output.getvalue())


class BootnodesDeploymentTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-bootnodes-test-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        # Exercises base bundle -> AssumeUTXO candidate -> immutable node kit.
        self.layout = native_kit(self.root)
        for owner, name, value in ((node, "effective_memory_bytes", 64 * 1024**3),
                                    (node, "_host_memory_bytes", 64 * 1024**3),
                                    (node, "_collect_compose_services", {}),
                                    (p2p, "host_capabilities", {**HOST, "ipv6": [], "ipv6_default_route": False}),
                                    (node, "_data_root_capacity", node.DataRootCapacity(
                                        filesystem_path=self.root, total_bytes=3 * 1024**4, free_bytes=3 * 1024**4))):
            patcher = mock.patch.object(owner, name, return_value=value)
            patcher.start()
            self.addCleanup(patcher.stop)

    def configure(self, layout=None, **kwargs):
        layout = layout or self.layout
        node.configure_node(layout, data_root=self.root / (layout.node_env.stem + "-data"),
                            role="full", miner_address="", miner_threads=1, nat="",
                            bitcoin_rpc_user=None, bitcoin_p2p="private", **kwargs)
        return node.read_env(layout.node_env)

    def test_packaging_preserves_catalog_and_first_configuration_uses_it(self):
        original = (ROOT / "docker/networks/testnet-v0/bootnodes.json").read_bytes()
        self.assertEqual((self.root / "bundle/bootnodes.json").read_bytes(), original)
        self.assertEqual((self.layout.bundle_dir / "bootnodes.json").read_bytes(), original)
        self.assertTrue((self.layout.kit_root / "docker/scripts/tools/peer_sources.py").is_file())
        defaults = sources.load_bootnodes(self.layout.bundle_dir, self.layout.bundle_id)
        self.assertTrue(defaults)
        self.assertTrue(any("@usdb-testnet.tbudr.top:31303" in seed for seed in defaults))
        env = self.configure()
        self.assertEqual(env["USDB_BOOTNODES"], ",".join(defaults))
        state, reason, _ = peers.membership(self.layout, env, syncing=False, peer_count=0)
        self.assertEqual((state, reason), ("WAITING", "WAITING_FOR_PEERS"))

    def test_configure_cli_distinguishes_omitted_custom_and_explicit_empty(self):
        for index, (arguments, expected) in enumerate((([], None), (["--bootnodes", ""], ""),
                                                      (["--bootnodes", V4 + "," + V4], V4))):
            with self.subTest(arguments=arguments):
                args = node.build_parser().parse_args(["configure", *arguments])
                layout = replace(self.layout, node_env=self.root / f"node{index}.env")
                env = self.configure(layout, bootnodes=args.bootnodes)
                self.assertEqual(env["USDB_BOOTNODES"], expected if expected is not None else
                                 ",".join(sources.load_bootnodes(layout.bundle_dir, layout.bundle_id)))

    @unittest.skipUnless(shutil.which("docker"), "Docker Compose is required")
    def test_multiple_defaults_reach_compose_without_starting_containers(self):
        write_bootnodes(self.layout.bundle_dir, [V4, DNS, V4])
        env = self.configure()
        environment = {key: value for key, value in os.environ.items()
                       if not key.startswith(("USDB_", "BTC_", "BH_", "COMPOSE_"))}
        environment.update(USDB_NETWORK_ARTIFACTS_DIR=str(self.layout.bundle_dir / "artifacts"),
                           BH_SNAPSHOT_TRUST_HOST_DIR=str(self.layout.bundle_dir / "trust"))
        command = ["docker", "compose", "--project-name", "bootnodes-test",
                   "--env-file", str(self.layout.bundle_dir / "network.env"),
                   "--env-file", str(self.layout.node_env),
                   "-f", str(self.layout.kit_root / "docker/compose.runtime.yml"),
                   "-f", str(self.layout.bundle_dir / "compose.network.yml"), "config", "--format", "json"]
        result = subprocess.run(command, env=environment, text=True, capture_output=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        chain = json.loads(result.stdout)["services"]["usdb-chain"]
        self.assertEqual(env["USDB_BOOTNODES"], V4 + "," + DNS)
        self.assertEqual(chain["environment"]["USDB_BOOTNODES"], env["USDB_BOOTNODES"])

    def test_real_setup_writes_defaults_and_existing_setup_preserves_seeds(self):
        answers = {"Host data root": str(self.root / "data"), "USDB P2P address family": "ipv4",
                   "Provide full Explorer": "n", "Enable local minting": "n"}
        node.setup_node(self.layout, input_fn=lambda prompt: next(
            (value for prefix, value in answers.items() if prompt.startswith(prefix)), ""), output=io.StringIO())
        expected = ",".join(sources.load_bootnodes(self.layout.bundle_dir, self.layout.bundle_id))
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_BOOTNODES"], expected)
        for seeds in ("", V4):
            node._atomic_write_private(self.layout.node_env, node.upsert_env(self.layout.node_env.read_text(), {"USDB_BOOTNODES": seeds}))
            original = self.layout.node_env.read_bytes()
            node.setup_node(self.layout, input_fn=lambda _: "", output=io.StringIO())
            self.assertEqual(self.layout.node_env.read_bytes(), original)

    def test_changed_defaults_preserve_identity_and_upgrade_operator_configuration(self):
        self.configure(bootnodes="")
        identity, compatibility = self.layout.network_identity, self.layout.runtime_compatibility
        write_bootnodes(self.layout.bundle_dir, [V4, DNS])
        upgraded = node.load_release_layout(self.layout.kit_root, self.layout.node_env)
        self.assertEqual(upgraded.network_identity, identity)
        self.assertEqual(upgraded.runtime_compatibility, compatibility)
        for seeds in ("", V6):
            node._atomic_write_private(self.layout.node_env, node.upsert_env(self.layout.node_env.read_text(), {"USDB_BOOTNODES": seeds}))
            original = self.layout.node_env.read_bytes()
            with redirect_stdout(io.StringIO()):
                node.activate_release(upgraded)
            self.assertEqual(self.layout.node_env.read_bytes(), original)

    def test_defaults_do_not_repopulate_removed_seeds_or_invalidate_first_node(self):
        with PeerFixture() as f:
            write_bootnodes(f.layout.bundle_dir, [V4], bundle_id=f.layout.bundle_id)
            f.layout.network_identity["network_json_sha256"] = self.layout.network_identity["network_json_sha256"]
            f.enable()
            self.assertEqual(f.run(), 0)
            record = mining.read_state(f.layout)
            f.edit(enode=V4)
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            f.edit("remove", V4)
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            write_bootnodes(self.layout.bundle_dir, [DNS])
            write_bootnodes(f.layout.bundle_dir, [DNS], bundle_id=f.layout.bundle_id)
            f.layout.network_identity["network_json_sha256"] = node.build_network_identity(
                self.layout.bundle_dir)["network_json_sha256"]
            mining.validate_start(f.layout)
            self.assertEqual(node.read_env(f.layout.node_env)["USDB_BOOTNODES"], "")
            self.assertEqual(mining.read_state(f.layout)["first_node"], record["first_node"])
            self.assertEqual(peers.observe(f.layout)["configured"], [])

    def test_malformed_catalog_blocks_release_validation(self):
        write_bootnodes(self.layout.bundle_dir, [V4], bundle_id="wrong-network")
        with self.assertRaisesRegex(ValueError, "INVALID_BOOTNODES_CONFIG"):
            node.load_release_layout(self.layout.kit_root, self.layout.node_env)

    def test_legacy_kit_without_catalog_keeps_empty_default(self):
        (self.layout.bundle_dir / sources.BOOTNODES_FILE).unlink()
        legacy = node.load_release_layout(self.layout.kit_root, self.layout.node_env)
        self.assertEqual(legacy.network_identity, self.layout.network_identity)
        self.assertEqual(self.configure(legacy)["USDB_BOOTNODES"], "")


if __name__ == "__main__":
    unittest.main()
