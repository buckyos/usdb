#!/usr/bin/env python3
"""Exercise repeatable setup against packaged native node fixtures, without services."""

import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import resource_policy as policy
import usdb_mining as mining
import usdb_minting as minting
import usdb_node as node
import usdb_peers as peers
from common.native_node import native_kit
from common.minting import disk_space


class SetupEditTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)
        disk_patch = mock.patch.object(minting, "disk_usage", return_value=disk_space())
        disk_patch.start()
        self.addCleanup(disk_patch.stop)
        for name, value in (("effective_memory_bytes", 64 * policy.GIB),
                            ("_host_memory_bytes", 64 * policy.GIB),
                            ("_collect_compose_services", {})):
            patch = mock.patch.object(node, name, return_value=value)
            patch.start()
            self.addCleanup(patch.stop)
        with mock.patch.object(node, "_validate_data_root_capacity"):
            node.configure_node(self.layout, data_root=self.root / "data", role="full", miner_address="",
                miner_threads=1, bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private",
                resource_management="auto")
        self.original = self.layout.node_env.read_bytes()
        self.backup = self.root / "node.env.setup-backup"

    def update(self, values):
        node._atomic_write_private(self.layout.node_env, node.upsert_env(self.layout.node_env.read_text(), values))

    def edit(self, answers=None, **kwargs):
        self.output = io.StringIO()
        self.prompts = []

        def answer(prompt):
            self.prompts.append(prompt)
            if callable(answers):
                return answers(prompt)
            return next((value for prefix, value in (answers or {}).items() if prompt.startswith(prefix)), "")

        # These operations belong to first installation, never to an edit.
        with mock.patch.object(node, "_validate_data_root_capacity", side_effect=AssertionError("initial disk check")), \
                mock.patch.object(node, "configure_node", side_effect=AssertionError("recreated node")):
            return node.setup_node(self.layout, input_fn=answer, output=self.output, **kwargs)

    def test_monitor_default_and_explicit_opt_out_survive_repeated_setup(self):
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_MONITOR_ENABLED"], "1")
        self.edit({"Enable node monitor": "n"})
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_MONITOR_ENABLED"], "0")
        original = self.layout.node_env.read_bytes()
        self.edit()
        self.assertEqual(self.layout.node_env.read_bytes(), original)
        self.edit({"Enable node monitor": "y"})
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_MONITOR_ENABLED"], "1")

    def test_enter_preserves_miner_identity_credentials_data_and_steady_budget(self):
        env = node.read_env(self.layout.node_env)
        self.update({**policy.build_resource_plan(64 * policy.GIB, "steady", env).environment(),
                     "USDB_NODE_ROLE": "miner", "USDB_MINER_ADDRESS": "0x" + "12" * 20,
                     "USDB_MINER_THREADS": "3", "USDB_CHAIN_EXTRA_ARGS": "--cache=777"})
        self.update({"USDB_CHAIN_IMAGE": "ghcr.io/buckyos/usdb-chain@sha256:" + "f" * 64})
        journal = node._resource_state_path(self.layout)
        journal.write_bytes(b"existing transition state")
        key = Path(env["USDB_CHAIN_DATA_HOST_DIR"]) / "nodekey"
        key.write_bytes(b"existing private identity")
        before = self.layout.node_env.read_bytes()
        result = self.edit()
        self.assertTrue(result.edited)
        self.assertEqual(self.layout.node_env.read_bytes(), before)
        self.assertEqual(journal.read_bytes(), b"existing transition state")
        self.assertEqual(key.read_bytes(), b"existing private identity")
        self.assertFalse(self.backup.exists())
        self.assertIn("No configuration changes", self.output.getvalue())
        self.assertIn("activate-release", self.output.getvalue())
        self.assertNotIn(env["BTC_RPC_PASSWORD"], self.output.getvalue())

    def test_enable_ord_and_explorer_preserves_other_configuration_and_credentials(self):
        old = node.read_env(self.layout.node_env)
        before = {path: path.read_bytes() for path in self.root.rglob("*") if path.is_file()}
        self.edit({"Provide full Explorer": "y", "Enable local minting": "y"})
        env = node.read_env(self.layout.node_env)
        self.assertEqual((env["BTC_TXINDEX"], env["USDB_MINTING_ENABLED"]), ("1", "1"))
        self.assertEqual((env["USDB_CHAIN_GCMODE"], env["USDB_CHAIN_TRACING"]), ("archive", "1"))
        for phase in policy.PHASES:
            plan = policy.build_resource_plan(64 * policy.GIB, phase, env)
            self.assertEqual(plan.limits["ORD_MEMORY_LIMIT"], 4 * policy.GIB)
            self.assertLessEqual(plan.total_bytes, 64 * policy.GIB)
        self.assertEqual(json.loads((minting.data_path(self.root / "data") / "identity.json").read_text()), minting.IDENTITY)
        for path, content in before.items():
            if path != self.layout.node_env:
                self.assertEqual(path.read_bytes(), content, str(path))
        self.assertEqual(self.backup.read_bytes(), self.original)
        self.assertEqual(self.backup.stat().st_mode & 0o777, 0o600)
        self.assertEqual(self.layout.node_env.stat().st_mode & 0o777, 0o600)
        self.assertEqual(env["BTC_RPC_PASSWORD"], old["BTC_RPC_PASSWORD"])
        for key in ("USDB_CHAIN_DATA_HOST_DIR", "USDB_DATA_ROOT", *self.layout.images):
            self.assertEqual(env[key], old[key])
        self.assertIn("no prepare --replace", self.output.getvalue())
        self.assertFalse(list(self.root.glob(".setup-*")))

    def test_mixed_query_settings_and_manual_custom_limits_survive_enter(self):
        self.update({"USDB_RESOURCE_MODE": "manual", "USDB_CHAIN_GCMODE": "archive", "USDB_CHAIN_EXTRA_ARGS": "--gcmode=archive --cache=777",
                     "BTC_MEMORY_LIMIT": "16g", "BTC_MEMORY_SWAP_LIMIT": "16g", "BTC_DBCACHE_MB": "6144"})
        before = self.layout.node_env.read_bytes()
        self.edit()
        self.assertEqual(self.layout.node_env.read_bytes(), before)
        self.assertTrue(any("Explorer support (keep" in prompt for prompt in self.prompts))
        self.assertTrue(any("[keep]" in prompt for prompt in self.prompts))

    def test_minting_keeps_manual_allocations_and_existing_ord_customization(self):
        bitcoin = node.BITCOIN_RESOURCE_PROFILES[node.DEFAULT_BITCOIN_RESOURCE_PROFILE]
        custom = {"USDB_RESOURCE_MODE": "manual", "BTC_RESOURCE_PROFILE": node.DEFAULT_BITCOIN_RESOURCE_PROFILE,
                  "BTC_MEMORY_LIMIT": bitcoin["memory_limit"], "BTC_MEMORY_SWAP_LIMIT": bitcoin["memory_swap_limit"],
                  "BTC_DBCACHE_MB": bitcoin["dbcache_mb"], "BH_MEMORY_LIMIT": "20g", "BH_MEMORY_SWAP_LIMIT": "22g",
                  "BH_SYNC_UTXO_MAX_CACHE_BYTES": str(4 * policy.GIB),
                  "BH_SYNC_BALANCE_MAX_CACHE_BYTES": str(8 * policy.GIB),
                  "ORD_MEMORY_LIMIT": "6g", "ORD_INDEX_CACHE_BYTES": str(2 * policy.GIB),
                  "ORD_MIN_FREE_BYTES": str(75 * policy.GIB)}
        self.update(custom)
        self.edit({"Enable local minting": "y"})
        env = node.read_env(self.layout.node_env)
        for key, value in custom.items():
            self.assertEqual(env[key], value)
        self.assertIn("memory 6.0 GiB, index cache 2.0 GiB, free-disk reserve 75.0 GiB", self.output.getvalue())

    def test_query_edit_migrates_legacy_flags_without_losing_unrelated_arguments(self):
        self.update({"USDB_CHAIN_GCMODE": "archive", "USDB_CHAIN_EXTRA_ARGS": "--gcmode=archive --cache=777"})
        self.edit({"Explorer support (keep": "full"})
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["USDB_CHAIN_TRACING"], "1")
        self.assertEqual(env["USDB_CHAIN_EXTRA_ARGS"], "--cache=777")

    def test_disable_ord_retains_index_and_custom_limits(self):
        self.edit({"Enable local minting": "y"})
        index = minting.data_path(self.root / "data") / "index.redb"
        index.write_bytes(b"valuable index")
        self.update({"ORD_MEMORY_LIMIT": str(6 * policy.GIB), "ORD_INDEX_CACHE_BYTES": str(2 * policy.GIB)})
        self.edit({"Enable local minting": "n"})
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["BTC_TXINDEX"], "0")
        self.assertEqual(env["ORD_MEMORY_LIMIT"], str(6 * policy.GIB))
        self.assertEqual(index.read_bytes(), b"valuable index")

    def test_cancel_and_input_eof_preserve_configuration_without_creating_ord(self):
        def eof(prompt):
            if prompt.startswith("Save these changes"):
                raise EOFError()
            return "y" if prompt.startswith("Enable local minting") else ""

        for answers in ({"Enable local minting": "y", "Save these changes": "n"}, eof):
            with self.subTest(answers=answers), self.assertRaisesRegex(ValueError, "setup cancelled"):
                self.edit(answers)
            self.assertEqual(self.layout.node_env.read_bytes(), self.original)
            self.assertFalse(self.backup.exists())
            self.assertFalse(minting.data_path(self.root / "data").exists())

    def test_ord_capacity_failure_preserves_config_backup_and_resource_journal(self):
        self.backup.write_bytes(b"existing backup")
        journal = node._resource_state_path(self.layout)
        journal.write_bytes(b"existing resource journal")
        with mock.patch.object(minting, "disk_usage", return_value=disk_space(299 * policy.GIB)):
            with self.assertRaisesRegex(ValueError, "300.0 GiB required"):
                self.edit({"Enable local minting": "y"})
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)
        self.assertEqual(self.backup.read_bytes(), b"existing backup")
        self.assertEqual(journal.read_bytes(), b"existing resource journal")
        self.assertFalse(minting.data_path(self.root / "data").exists())
        self.assertFalse(any(prompt.startswith("Save these changes") for prompt in self.prompts))

    def test_ord_capacity_is_checked_again_after_save_confirmation(self):
        with mock.patch.object(minting, "disk_usage", side_effect=[disk_space(), disk_space(299 * policy.GIB)]):
            with self.assertRaisesRegex(ValueError, "Insufficient space"):
                self.edit({"Enable local minting": "y"})
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)
        self.assertFalse(self.backup.exists())
        self.assertFalse(minting.data_path(self.root / "data").exists())

    def test_invalid_candidate_never_replaces_live_configuration(self):
        self.update({"ORD_MEMORY_LIMIT": "1g"})
        before = self.layout.node_env.read_bytes()
        with self.assertRaisesRegex(ValueError, "Ord requires at least"):
            self.edit({"Enable local minting": "y"})
        self.assertEqual(self.layout.node_env.read_bytes(), before)
        self.assertFalse(self.backup.exists())
        self.assertFalse(minting.data_path(self.root / "data").exists())
        self.assertFalse(list(self.root.glob(".setup-*")))

    def test_validation_failure_preserves_live_file_and_existing_backup(self):
        self.backup.write_bytes(b"older backup")
        with mock.patch.object(node, "_validate_node_config", side_effect=ValueError("incompatible dataset")):
            with self.assertRaisesRegex(ValueError, "incompatible dataset"):
                self.edit({"Provide full Explorer": "y"})
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)
        self.assertEqual(self.backup.read_bytes(), b"older backup")

    def test_rejects_active_unknown_and_unobservable_services_before_prompts(self):
        for state in ("running", "restarting", "paused", "unknown", None):
            with mock.patch.object(node, "_collect_compose_services", return_value={"btc-node": {"state": state}}):
                with self.assertRaisesRegex(ValueError, "usdb-node down"):
                    self.edit()
            self.assertEqual(self.prompts, [])
        with mock.patch.object(node, "_collect_compose_services", side_effect=ValueError("Docker unavailable")):
            with self.assertRaisesRegex(ValueError, "Docker unavailable"):
                self.edit()
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)

    def test_pending_peer_or_mining_operations_cannot_be_bypassed(self):
        for module in (peers, mining):
            with mock.patch.object(module, "pending", return_value=True):
                with self.assertRaisesRegex(ValueError, "operation is pending"):
                    self.edit()
            self.assertEqual(self.prompts, [])

    def test_concurrent_operator_edit_is_not_overwritten(self):
        def answer(prompt):
            if prompt.startswith("Save these changes"):
                self.update({"USDB_MINER_THREADS": "4"})
            return "y" if prompt.startswith("Provide full Explorer") else ""

        with self.assertRaisesRegex(ValueError, "changed during setup"):
            self.edit(answer)
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_MINER_THREADS"], "4")
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_CHAIN_GCMODE"], "full")
        self.assertFalse(self.backup.exists())

    def test_container_started_during_confirmation_prevents_write(self):
        with mock.patch.object(node, "_collect_compose_services", side_effect=[{}, {"ord-server": {"state": "running"}}]):
            with self.assertRaisesRegex(ValueError, "usdb-node down"):
                self.edit({"Provide full Explorer": "y"})
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)

    def test_resource_recalculation_is_explicit_and_clears_old_plan(self):
        journal = node._resource_state_path(self.layout)
        state = dict(schema_version="usdb-resource-state:v1", bundle_id=self.layout.bundle_id,
                     phase="bitcoin", recover_services=[], pending=False)
        journal.write_text(json.dumps(state))
        self.edit({"Recalculate automatic": "y"})
        self.assertFalse(journal.exists())
        self.assertTrue(self.backup.exists())
        self.assertIn("Resource transition journal", self.output.getvalue())

    def test_minting_changes_rebudget_each_current_phase_without_rolling_back(self):
        for entrypoint in ("setup", "set-minting"):
            for phase in policy.PHASES:
                with self.subTest(entrypoint=entrypoint, phase=phase):
                    env = node.read_env(self.layout.node_env)
                    before = policy.build_resource_plan(64 * policy.GIB, phase, env).environment()
                    self.update(before)
                    for active in (True, False):
                        if entrypoint == "setup":
                            self.edit({"Enable local minting": "y" if active else "n"})
                        else:
                            minting.configure(self.layout, active, node)
                        current = node.read_env(self.layout.node_env)
                        self.assertEqual(current["BTC_TXINDEX"], "1" if active else "0")
                        self.assertEqual(current["USDB_RESOURCE_PHASE"], phase)
                        self.assertEqual(current["BTC_RESOURCE_PROFILE"], "managed-" + phase)
                        policy.validate_resource_environment(current, 64 * policy.GIB)
                        if active:
                            self.assertEqual(int(current["ORD_MEMORY_LIMIT"]), 4 * policy.GIB)
                            self.assertLessEqual(int(current["BTC_MEMORY_LIMIT"]), int(before["BTC_MEMORY_LIMIT"]))
                            self.assertLess(int(current["BH_MEMORY_LIMIT"]), int(before["BH_MEMORY_LIMIT"]))
                        else:
                            self.assertEqual(current["BTC_MEMORY_LIMIT"], before["BTC_MEMORY_LIMIT"])
                            self.assertEqual(current["BH_MEMORY_LIMIT"], before["BH_MEMORY_LIMIT"])

    def test_setup_shows_readable_policy_budgets_without_ord_tuning_prompts(self):
        env = node.read_env(self.layout.node_env)
        self.update(policy.build_resource_plan(64 * policy.GIB, "steady", env).environment())
        self.edit({"Enable local minting": "y"})
        text = self.output.getvalue()
        self.assertIn("memory 4.0 GiB, index cache 1.0 GiB, free-disk reserve 50.0 GiB", text)
        self.assertIn("phase=steady", text)
        self.assertIn("calculated budgets, not manual overrides", text)
        self.assertIn("BH_MEMORY_LIMIT: 24.0 GiB -> 21.0 GiB", text)
        self.assertIn("BTC_DBCACHE_MB: 4.0 GiB -> 3.7 GiB", text)
        self.assertNotIn(str(4 * policy.GIB), text)
        self.assertFalse(any(prompt.startswith(("Adjust Ord", "ORD_")) for prompt in self.prompts))

    def test_readable_memory_prompts_keep_exact_custom_budgets_on_enter(self):
        env = {**node.read_env(self.layout.node_env), "USDB_EXTERNAL_MEMORY_BUDGET": str(policy.GIB + 123),
               "USDB_BH_MEMORY_CAP": "1024g"}
        self.update({**env, **policy.build_resource_plan(64 * policy.GIB, "steady", env).environment()})
        before = self.layout.node_env.read_bytes()
        self.edit({"Adjust memory budgets": "y"})
        self.assertEqual(self.layout.node_env.read_bytes(), before)
        self.assertIn("USDB_EXTERNAL_MEMORY_BUDGET [1.0 GiB]: ", self.prompts)
        self.assertIn("USDB_BH_MEMORY_CAP [1.0 TiB]: ", self.prompts)

    def test_explicit_resource_recalculation_retains_existing_automatic_phase(self):
        for phase in policy.PHASES:
            with self.subTest(phase=phase):
                env = node.read_env(self.layout.node_env)
                self.update(policy.build_resource_plan(64 * policy.GIB, phase, env).environment())
                self.edit({"Recalculate automatic": "y"})
                self.assertEqual(node.read_env(self.layout.node_env)["USDB_RESOURCE_PHASE"], phase)
                node.set_resource_policy(self.layout, "auto", {"USDB_BTC_STEADY_MEMORY_CAP": "6g"})
                env = node.read_env(self.layout.node_env)
                self.assertEqual(env["USDB_RESOURCE_PHASE"], phase)
                policy.validate_resource_environment(env, 64 * policy.GIB)

    def test_cli_resource_overrides_and_manual_mode_defaults(self):
        env = node.read_env(self.layout.node_env)
        self.update(policy.build_resource_plan(64 * policy.GIB, "steady", env).environment())
        self.edit(resource_management="manual", bitcoin_resource_profile=node.DEFAULT_BITCOIN_RESOURCE_PROFILE)
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["USDB_RESOURCE_MODE"], "manual")
        before = self.layout.node_env.read_bytes()
        self.edit()
        self.assertEqual(self.layout.node_env.read_bytes(), before)
        self.edit(resource_management="auto", resource_caps={"USDB_BTC_IBD_MEMORY_CAP": "20g"})
        env = node.read_env(self.layout.node_env)
        self.assertEqual(env["USDB_BTC_IBD_MEMORY_CAP"], "20g")
        self.assertEqual(env["USDB_RESOURCE_PHASE"], "bitcoin")
        self.assertLessEqual(int(env["BTC_MEMORY_LIMIT"]), 20 * policy.GIB)

    def test_cli_edit_does_not_install_controller_apply_firewall_or_select_release(self):
        args = node.build_parser().parse_args(["setup"])
        real_setup = node.setup_node
        output = io.StringIO()

        def answer(prompt):
            return "y" if prompt.startswith("Manage this host firewall") else ""

        with mock.patch.object(node.sys.stdin, "isatty", return_value=True), \
                mock.patch.object(node.sys.stdout, "isatty", return_value=True), \
                mock.patch.object(node, "_controller_install_context", side_effect=AssertionError("sudo")), \
                mock.patch.object(node, "install_controller_unit", side_effect=AssertionError("systemd")), \
                mock.patch.object(node, "run_firewall_action", side_effect=AssertionError("firewall")), \
                mock.patch.object(node, "activate_release", side_effect=AssertionError("activate")), \
                mock.patch.object(node, "setup_node", side_effect=lambda layout, **kw: real_setup(layout, input_fn=answer, output=output, **kw)):
            self.assertEqual(node._execute_command(self.layout, args), 0)
        self.assertEqual(node.read_env(self.layout.node_env)["USDB_FIREWALL_MODE"], "managed")
        self.assertIn("firewall apply --confirm", output.getvalue())
        self.assertIn("up --foreground", output.getvalue())

    def test_explicit_p2p_flags_are_not_silently_ignored_when_editing(self):
        with self.assertRaisesRegex(ValueError, "peers configure"):
            self.edit(p2p_options={"requested": "ipv4"})
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)

    def test_backup_symlink_is_rejected_and_target_preserved(self):
        target = self.root / "unrelated"
        target.write_bytes(b"preserve")
        self.backup.symlink_to(target)
        with self.assertRaisesRegex(ValueError, "backup must not be a symlink"):
            self.edit({"Provide full Explorer": "y"})
        self.assertEqual(target.read_bytes(), b"preserve")
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)

    def test_packaged_cli_includes_editor_and_help(self):
        tool = self.layout.kit_root / "docker/scripts/tools/usdb_node.py"
        self.assertTrue(tool.with_name("usdb_setup.py").is_file())
        result = subprocess.run([sys.executable, str(tool), "setup", "--help"], capture_output=True, text=True, check=True)
        self.assertIn("retain current mode when editing", " ".join(result.stdout.split()))

    def test_packaged_main_runs_existing_setup_without_host_installation(self):
        script = """
import sys
from pathlib import Path
kit, config = sys.argv[1:]
sys.path.insert(0, str(Path(kit) / "docker/scripts/tools"))
import usdb_node as node
node._collect_compose_services = lambda layout: {}
node.effective_memory_bytes = lambda: 64 * 1024**3
sys.stdin.isatty = lambda: True
sys.stdout.isatty = lambda: True
sys.argv = ["usdb-node", "--kit-root", kit, "--node-env", config, "setup"]
raise SystemExit(node.main())
"""
        result = subprocess.run([sys.executable, "-c", script, str(self.layout.kit_root), str(self.layout.node_env)],
                                input="\n" * 20, capture_output=True, text=True, check=True)
        self.assertIn("No configuration changes", result.stdout)
        self.assertEqual(self.layout.node_env.read_bytes(), self.original)


if __name__ == "__main__":
    unittest.main()
