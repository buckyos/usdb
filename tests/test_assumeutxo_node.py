#!/usr/bin/env python3
"""Verify immutable native bundles, early service startup and restartable memory handoffs."""

from contextlib import redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import assumeutxo_deployment as deployment
import assumeutxo_node as native
import artifact_signing as signing
import bitcoin_release as artifacts
import release_manifest as release
import resource_policy as policy
import usdb_node as node
import usdb_p2p as p2p
from runtime_compatibility import build_runtime_compatibility
from common.controller_status import ControllerStatusFixture
from common.native_node import NativeRuntime, ORIGIN, native_kit
from common.native_docker import install_docker_recorder
from common.p2p import HOST, V6


class NativeBundleTests(unittest.TestCase):
    def test_image_preparation_pulls_selected_services_with_structured_progress(self):
        import node_image_progress as images
        layout = native_kit(self.root)
        self.configure(layout)
        binary_dir = install_docker_recorder(self.root / "bin")
        calls = self.root / "docker-calls.jsonl"
        with mock.patch.dict(os.environ, PATH=str(binary_dir) + os.pathsep + os.environ["PATH"],
                             NATIVE_DOCKER_CALLS=str(calls)), \
             mock.patch.object(images, "image_cached", return_value=False), redirect_stdout(io.StringIO()):
            with images.ImagePreparation(layout) as preparation:
                for group in ("runtime", "bitcoin"):
                    images.prepare_image_group(layout, group, preparation, output_to_stderr=False, quiet_progress=True)
        commands = [json.loads(line) for line in calls.read_text().splitlines()]
        self.assertEqual(len(commands), 3)
        self.assertEqual([command[-2:] for command in commands],
                         [["pull", "balance-history"], ["pull", "usdb-chain"], ["pull", "btc-node"]])
        for command in commands:
            self.assertEqual(command[:3], ["compose", "--progress", "json"])
        self.assertIsNone(images.read_image_preparation(layout))

    def test_reused_snapshot_is_ready_without_displaying_another_hash_scan(self):
        latest = dict(phase="snapshot_active", details=dict(report=dict(snapshot_file_reused=True)))
        item = native._snapshot_component("snapshot_active", latest, {}, failed=False, complete=True, activated=True)
        self.assertEqual(item["state"], "READY")
        self.assertIn("reused", item["detail"])
        self.assertEqual(item["progress_percent"], 100.0)

    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-native-bundle-")
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)

    def configure(self, layout):
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
             mock.patch.object(node, "_validate_data_root_capacity"):
            node.configure_node(layout, data_root=self.root / "data", role="full", miner_address="", miner_threads=1,
                                bootnodes="", nat="none", bitcoin_rpc_user=None, bitcoin_p2p="private", resource_management="auto")

    def test_native_kit_configures_without_legacy_snapshot_and_loads_in_isolation(self):
        layout = native_kit(self.root)
        self.configure(layout)
        env = node.read_env(layout.node_env)
        self.assertEqual(env["SNAPSHOT_MODE"], "assumeutxo")
        self.assertEqual(env["BTC_TXINDEX"], "0")
        self.assertEqual(env["BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH"], ORIGIN)
        self.assertEqual(env["BTC_BOOTSTRAP_MEMORY_LIMIT"], str(128 * policy.MIB))
        node._validate_node_config(layout, require_runtime=True, require_bitcoin_runtime=True)
        self.assertEqual(node._snapshot_lifecycle_status(layout, env)["state"], "native")
        tool_dir = layout.kit_root / "docker/scripts/tools"
        result = subprocess.run([sys.executable, "-c", "import sys; sys.path.insert(0, sys.argv[1]); import usdb_node, assumeutxo_node, node_image_progress; print(usdb_node.load_release_layout(__import__('pathlib').Path(sys.argv[2])).snapshot['status'])", str(tool_dir), str(layout.kit_root)],
                                cwd=self.root, capture_output=True, text=True, env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.strip(), "native")
        # Existing datasets retain their contracts; only native BH has a new source boundary.
        legacy = build_runtime_compatibility(release.build_network_identity(ROOT / "docker/networks/testnet-v0"))
        current = layout.runtime_compatibility
        for service in ("bitcoin_core", "usdb_indexer", "usdb_chain", "control_plane"):
            self.assertEqual(current["services"][service], legacy["services"][service])
        self.assertNotEqual(current["services"]["balance_history"], legacy["services"]["balance_history"])

    def test_native_contract_cannot_be_overridden_in_node_env(self):
        layout = native_kit(self.root)
        self.configure(layout)
        original = layout.node_env.read_text()
        for key, value in (("SNAPSHOT_MODE", "none"), ("BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH", "1" * 64),
                           ("BTC_ASSUMEUTXO_SOURCE_URL", "https://example.com/changed"), ("BH_SCRIPT_REGISTRY_ENABLED", "1"),
                           ("BTC_BOOTSTRAP_MEMORY_LIMIT", "64m")):
            with self.subTest(key=key):
                layout.node_env.write_text(node.upsert_env(original, {key: value}))
                with self.assertRaises(ValueError):
                    node._validate_node_config(layout, require_runtime=True, require_bitcoin_runtime=True)
        layout.node_env.write_text(original)
        # Recomputing the policy after a stopped-node change retains the observer budget.
        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
             mock.patch.object(node, "_collect_compose_services", return_value={}):
            node.set_resource_policy(layout, "auto", {})
        self.assertEqual(node.read_env(layout.node_env)["BTC_BOOTSTRAP_MEMORY_LIMIT"], str(128 * policy.MIB))

    def test_invalid_origin_never_creates_candidate(self):
        output = self.root / "candidate"
        with self.assertRaises(ValueError):
            deployment.prepare_bundle(ROOT / "docker/networks/testnet-v0", output, "0" * 64, "", None, None)
        self.assertFalse(output.exists())

    def test_shell_starts_data_only_after_budget_baseline_and_preparation(self):
        layout = native_kit(self.root)
        self.configure(layout)
        binary = install_docker_recorder(self.root / "bin")
        calls = self.root / "docker-calls.jsonl"
        environment = {**os.environ, "PATH": str(binary) + os.pathsep + os.environ["PATH"],
                       "PYTHONDONTWRITEBYTECODE": "1", "NATIVE_DOCKER_CALLS": str(calls),
                       "USDB_TESTNET_NODE_ENV": str(layout.node_env), "USDB_TESTNET_BUNDLE_DIR": str(layout.bundle_dir)}
        helper = layout.kit_root / "docker/scripts/tools/run_testnet_runtime.sh"
        for phase, ready, observer, success in (("bitcoin", "1", "0", False), ("overlap", "0", "0", False),
                                               ("overlap", "1", "1", False), ("overlap", "1", "0", True)):
            with self.subTest(phase=phase, ready=ready, observer=observer):
                env = node.read_env(layout.node_env)
                layout.node_env.write_text(node.upsert_env(layout.node_env.read_text(), policy.build_resource_plan(64 * policy.GIB, phase, env).environment()))
                calls.write_text("")
                result = subprocess.run(["bash", str(helper), "native-start-data"], capture_output=True, text=True,
                    env={**environment, "NATIVE_CORE_READY": ready, "NATIVE_OBSERVER_EXIT": observer}, timeout=15)
                self.assertEqual(result.returncode == 0, success, result.stderr)
                started = [json.loads(line) for line in calls.read_text().splitlines() if '"up"' in line]
                self.assertEqual(len(started), int(success))
                if success:
                    self.assertEqual(started[0][-5:], ["up", "-d", "--no-deps", "balance-history", "usdb-indexer"])
        for action in ("up-data", "managed-start-snapshot", "managed-start-data", "install-registry"):
            calls.write_text("")
            result = subprocess.run(["bash", str(helper), action], capture_output=True, text=True, env=environment, timeout=15)
            self.assertNotEqual(result.returncode, 0)
            self.assertEqual(calls.read_text(), "")

    def test_progress_reports_independent_background_and_does_not_trust_seal_journal(self):
        layout = native_kit(self.root)
        self.configure(layout)
        env = node.read_env(layout.node_env)
        progress = Path(env["BH_DATA_HOST_DIR"]) / "bootstrap-progress.json"
        progress.write_text(json.dumps(dict(phase="sealed", published=True, height=963800, elapsed_seconds=60)))
        services = {"btc-node": dict(state="running"), "balance-history": dict(state="running"), "btc-snapshot-bootstrap": dict(state="exited", exit_code=0)}
        core = dict(bootstrap_ready=True, tip_ready=True, history_validated=False, active_height=966674, headers=966674, background_height=105439)
        with mock.patch.object(node, "_collect_compose_services", return_value=services), \
             mock.patch.object(native, "core_progress", return_value=core), \
             mock.patch.object(node, "_read_service_readiness", return_value=(None, "RPC unavailable")), \
             mock.patch.object(node, "_chain_component", return_value=node._component_progress("usdb_chain", "WAITING", "not started")), \
             mock.patch.object(node, "controller_observed_state", return_value="STARTING"):
            report = node.collect_node_progress(layout)
        components = {item["id"]: item for item in report["components"]}
        self.assertEqual(components["bitcoin"]["state"], "READY")
        self.assertIn("history_validated=False", components["bitcoin"]["detail"])
        self.assertEqual(components["balance_history"]["state"], "STARTING")
        self.assertNotEqual(report["overall_state"], "READY")
        self.assertEqual(report["native_bootstrap"]["balance_history"]["phase"], "sealed")

    def test_native_progress_distinguishes_startup_wait_from_probe_and_container_failures(self):
        layout = native_kit(self.root)
        self.configure(layout)
        # This is the real helper result observed before the first container exists.
        probe_failure = subprocess.CompletedProcess([], 1, "", 'service "btc-node" is not running\n')
        cases = [({}, None, "WAITING", False),
                 ({"btc-node": dict(state="created")}, None, "WAITING", False),
                 ({"btc-node": dict(state="running")}, None, "STARTING", True),
                 ({}, ValueError("Docker inventory unavailable"), "STARTING", True),
                 ({"btc-node": dict(state="created", container_error="OCI runtime failed")}, None, "FAILED", True),
                 ({"btc-node": dict(state="exited", exit_code=1)}, None, "FAILED", True)]
        for services, inventory_error, expected, should_probe in cases:
            with self.subTest(services=services, inventory_error=inventory_error), \
                 mock.patch.object(node, "_collect_compose_services", return_value=services, side_effect=inventory_error), \
                 mock.patch.object(node, "run_helper", return_value=probe_failure) as helper, \
                 mock.patch.object(node, "_read_service_readiness", return_value=(None, "RPC unavailable")), \
                 mock.patch.object(node, "_chain_component", return_value=node._component_progress("usdb_chain", "WAITING", "not started")), \
                 mock.patch.object(node, "_resource_progress", return_value=({}, False)), \
                 mock.patch.object(node, "controller_observed_state", return_value="active"):
                report = node.collect_node_progress(layout)
                components = {item["id"]: item for item in report["components"]}
                bitcoin = components["bitcoin"]
                self.assertEqual(bitcoin["state"], expected)
                self.assertEqual(helper.called, should_probe)
                self.assertFalse(bitcoin["background_validation"]["available"])
                rendered = node.render_node_progress(report, width=180)
                if expected == "WAITING":
                    self.assertIn("Bitcoin Core container has not started", rendered)
                    self.assertIn("Core background history: WAITING for Core startup", rendered)
                    self.assertNotIn("invalid JSON", rendered)
                    self.assertFalse(report["native_bootstrap"]["core"]["bootstrap_ready"])
                    from node_image_progress import ImagePreparation
                    with ImagePreparation(layout) as preparation:
                        preparation.set_group("runtime")
                        pulling = node.collect_node_progress(layout)
                    self.assertEqual(pulling["overall_state"], "INSTALLING")
                    self.assertIn("Waiting for container images before snapshot download",
                                  node.render_node_progress(pulling, width=180))
                elif expected == "STARTING":
                    self.assertEqual(bitcoin["display_state"], "UNAVAILABLE")
                    self.assertIn("invalid JSON", rendered)
                    self.assertNotIn("WAITING for Core startup", rendered)
                else:
                    self.assertNotIn("display_state", bitcoin)
                    self.assertEqual(report["overall_state"], "FAILED")

    def test_chain_wait_explains_foreground_gap_even_when_both_data_services_are_ready(self):
        layout = native_kit(self.root)
        self.configure(layout)
        services = {name: dict(state="running") for name in ("btc-node", "balance-history", "usdb-indexer")}
        services["btc-snapshot-bootstrap"] = dict(state="exited", exit_code=0)
        core = dict(bootstrap_ready=True, tip_ready=False, active_height=967953, headers=967961,
                    history_validated=False, background_height=716203)
        bh = dict(consensus_ready=True, query_ready=True, phase="Indexing", stable_height=967943, current=967943, total=967943)
        indexer = dict(consensus_ready=True, synced_block_height=967943, balance_history_stable_height=967943)
        with mock.patch.object(node, "_collect_compose_services", return_value=services), \
             mock.patch.object(native, "core_progress", return_value=core), \
             mock.patch.object(node, "_read_service_readiness", side_effect=lambda *args: (bh if args[-1] == "balance-history" else indexer, None)) as readiness, \
             mock.patch("usdb_peers.read_state", return_value=None), \
             mock.patch.object(node, "_resource_progress", return_value=({}, False)) as resources:
            report = native.collect_native_progress(layout, controller_state="active")
            self.assertEqual(readiness.call_count, 2)
            components = {item["id"]: item for item in report["components"]}
            self.assertEqual(components["usdb_chain"]["detail"],
                             "Waiting for Bitcoin foreground: 8 blocks remaining (967953/967961)")
            self.assertEqual(components["balance_history"]["state"], "WAITING")
            self.assertEqual(components["usdb_indexer"]["state"], "READY")
            self.assertEqual(components["balance_history"]["current"], components["usdb_indexer"]["current"])
            self.assertEqual(components["balance_history"]["total"], 967951)
            self.assertEqual(components["usdb_indexer"]["total"], 967943)
            for details in (False, True):
                rendered = " ".join(node.render_node_progress(report, details=details).split())
                self.assertIn("Target: Bitcoin headers minus 10 confirmation blocks = 967951", rendered)
                self.assertIn("Target: balance-history available stable height = 967943", rendered)
            # Real chain initialization failures must take precedence over upstream waits.
            services["usdb-chain-init"] = dict(state="exited", exit_code=1)
            failed = native.collect_native_progress(layout, controller_state="active")
            chain = next(item for item in failed["components"] if item["id"] == "usdb_chain")
            self.assertEqual(chain["state"], "FAILED")
            self.assertIn("usdb-chain-init", chain["detail"])
            del services["usdb-chain-init"]

            def managed_restart(_layout, _env, _services, components):
                chain = next(item for item in components if item["id"] == "usdb_chain")
                chain.update(state="STARTING", detail="waiting for managed service startup")
                return {}, True

            resources.side_effect = managed_restart
            report = native.collect_native_progress(layout, controller_state="active")
            chain = next(item for item in report["components"] if item["id"] == "usdb_chain")
            self.assertEqual(chain["state"], "STARTING")
            self.assertIn("managed service startup; Waiting for Bitcoin foreground: 8 blocks remaining", chain["detail"])

    def test_chain_wait_tracks_raw_readiness_without_gating_on_background_validation(self):
        core = dict(bootstrap_ready=True, tip_ready=True, active_height=967961, headers=967961, history_validated=False)
        loader = dict(state="exited", exit_code=0)
        readiness = {name: (dict(consensus_ready=True), None) for name in ("balance-history", "usdb-indexer")}
        cases = [
            ({**core, "error": "RPC timeout", "rpc_available": False}, loader, readiness, "Bitcoin readiness: RPC timeout"),
            ({**core, "bootstrap_ready": False}, loader, readiness, "snapshot baseline activation"),
            (core, dict(state="running"), readiness, "snapshot preparation"),
            ({**core, "tip_ready": False, "active_height": 967960}, loader, readiness, "1 block remaining (967960/967961)"),
            ({**core, "tip_ready": False}, loader, readiness, "tip freshness and peer connections"),
            (core, loader, {**readiness, "balance-history": (None, "RPC timeout")}, "balance-history readiness: RPC timeout"),
            (core, loader, {**readiness, "balance-history": (dict(consensus_ready=False, blockers=["Indexing"]), None)}, "balance-history readiness: Indexing"),
            (core, loader, {**readiness, "usdb-indexer": (dict(consensus_ready=False, message="backfilling history"), None)}, "usdb-indexer readiness: backfilling history"),
            (core, loader, readiness, "Upstream ready; waiting for controller"),
        ]
        for observed_core, observed_loader, observed_readiness, expected in cases:
            with self.subTest(expected=expected):
                self.assertIn(expected, native._chain_wait_detail(observed_core, observed_loader, observed_readiness))

    def test_installer_setup_and_doctor_accept_native_release_before_download(self):
        layout = native_kit(self.root)
        assets = self.root / "assets"
        assets.mkdir()
        for name in ("usdb-release-manifest.json", "usdb-release-manifest.json.sha256"):
            shutil.copy2(layout.kit_root / "release" / name, assets / name)
        archive = assets / (layout.release_id + "-node-kit.tar.gz")
        with tarfile.open(archive, "w:gz") as output:
            output.add(layout.kit_root, arcname="usdb-node-kit")
        archive.with_name(archive.name + ".sha256").write_text(hashlib.sha256(archive.read_bytes()).hexdigest() + "  " + archive.name + "\n")
        result = subprocess.run(["bash", str(ROOT / "docker/scripts/tools/install_usdb_node.sh"),
            "--release-id", layout.release_id, "--release-base-url", assets.as_uri(),
            "--install-root", str(self.root / "installed"), "--bin-dir", str(self.root / "bin")],
            capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        installed = node.load_release_layout(self.root / "installed" / layout.release_id, node_env=layout.node_env)
        template_path = installed.bundle_dir / "node.env.example"
        original_template = template_path.read_bytes()
        p2p_options = p2p.options(node.build_parser().parse_args(["setup"]))
        prompts, output = [], io.StringIO()

        def answer(prompt):
            prompts.append(prompt)
            return str(self.root / "data") if prompt.startswith("Host data root") else ""

        with mock.patch.object(node, "effective_memory_bytes", return_value=64 * policy.GIB), \
             mock.patch.object(node, "_validate_data_root_capacity", return_value=node.DataRootCapacity(self.root, 4 * 1024**4, 3 * 1024**4)), \
             mock.patch.object(node, "detect_ssh_server_port", return_value=22), \
             mock.patch.object(p2p, "host_capabilities", return_value=HOST), \
             mock.patch.object(p2p, "engine_capabilities", return_value={"engine": "28.0.0", "compose": "2.33.1"}):
            setup = node.setup_node(installed, input_fn=answer, output=output, resource_management="auto",
                                    p2p_options=p2p_options)
            self.assertFalse(setup.install_snapshot)
            self.assertFalse(any("Use this release-approved snapshot" in prompt for prompt in prompts))
            self.assertIn("Native AssumeUTXO bootstrap", output.getvalue())
            import usdb_p2p
            with mock.patch.object(node, "run_host_action"), mock.patch.object(usdb_p2p, "check_host"), \
                 mock.patch.object(node, "run_helper") as helper, redirect_stdout(output):
                node.doctor(installed, allow_pending_snapshot=True)
            self.assertEqual([call.args[2] for call in helper.call_args_list], [["validate-node"]])
        self.assertIn("INFO AssumeUTXO", output.getvalue())
        self.assertNotIn("PENDING Snapshot:", output.getvalue())
        env = node.read_env(installed.node_env)
        self.assertEqual({key: env[key] for key in p2p.KEYS}, {
            "USDB_P2P_IP_FAMILY": "dual", "USDB_P2P_REQUESTED_FAMILY": "auto",
            "USDB_P2P_ADVERTISE_IPV4": "", "USDB_P2P_ADVERTISE_IPV6": "auto",
            "USDB_P2P_ADVERTISE_PORT": "31303", "USDB_P2P_ADVERTISE_DISCOVERY_PORT": "31303",
        })
        self.assertEqual(template_path.read_bytes(), original_template)
        self.assertEqual(list(Path(env["BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR"]).iterdir()), [])

    def test_watch_separates_download_import_replay_and_background_validation(self):
        layout = native_kit(self.root)
        self.configure(layout)
        env = node.read_env(layout.node_env)
        download_path = Path(env["BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR"]) / "mainnet-935000-utxos.dat.download/progress.json"
        download_path.parent.mkdir()
        activation_path = Path(env["BTC_ASSUMEUTXO_STATE_HOST_DIR"]) / "activation.json"
        bh_path = Path(env["BH_DATA_HOST_DIR"]) / "bootstrap-progress.json"
        started = "2026-09-14T08:30:00+00:00"
        services = {"btc-node": dict(state="running", started_at=started), "btc-snapshot-bootstrap": dict(state="running"),
                    "balance-history": dict(state="running")}
        core = dict(bootstrap_ready=False, tip_ready=False, history_validated=False,
                    active_height=1, headers=966674, background_height=None)
        with mock.patch.object(node, "_collect_compose_services", return_value=services), \
             mock.patch.object(native, "core_progress", return_value=core), \
             mock.patch.object(node, "_read_service_readiness", return_value=(None, "RPC unavailable")), \
             mock.patch.object(node, "_chain_component", return_value=node._component_progress("usdb_chain", "WAITING", "not started")), \
             mock.patch.object(node, "_resource_progress", return_value=({}, False)), \
             mock.patch.object(node, "controller_observed_state", return_value="STARTING"):
            download_path.write_text(json.dumps(dict(phase="downloading", updated_at=1, details=dict(bytes=50, total_bytes=100))))
            report = node.collect_node_progress(layout)
            self.assertEqual(report["components"][0]["progress_percent"], 50)
            # The file milestone survives a new observer and later activation journals.
            artifact = download_path.parent.with_name("mainnet-935000-utxos.dat")
            with artifact.open("wb") as output:
                output.truncate(artifacts.UTXO_SIZE)
            download = dict(schema_version="usdb-bitcoin-assumeutxo:v1", phase="verifying_file", updated_at=2,
                identity=dict(snapshot={**layout.snapshot["contract"]["snapshot"], "size_bytes": artifacts.UTXO_SIZE}),
                details=dict(bytes=0, total_bytes=artifacts.UTXO_SIZE))
            download_path.write_text(json.dumps(download))
            report = node.collect_node_progress(layout)
            self.assertEqual(report["components"][0]["progress_percent"], 0)
            rendered = node.render_node_progress(report, width=80)
            self.assertIn("File: download complete", rendered)
            self.assertIn("File SHA-256: verification in progress", rendered)
            services["btc-snapshot-bootstrap"].update(state="exited", exit_code=1)
            failed = node.collect_node_progress(layout)
            self.assertEqual(failed["overall_state"], "FAILED")
            self.assertIn("File SHA-256: not confirmed", node.render_node_progress(failed, width=80))
            services["btc-snapshot-bootstrap"].update(state="running", exit_code=0)
            download.update(phase="file_published", details=dict(bytes=artifacts.UTXO_SIZE, total_bytes=artifacts.UTXO_SIZE))
            download_path.write_text(json.dumps(download))
            activation_path.write_text(json.dumps(dict(phase="waiting_for_headers", updated_at=3)))
            report = node.NodeProgressHistory().apply(node.collect_node_progress(layout))
            self.assertEqual(report["components"][0]["state"], "WAITING")
            self.assertIsNone(report["components"][0]["progress_percent"])
            rendered = node.render_node_progress(report, width=80)
            self.assertIn("[WAIT] UTXO snapshot", rendered)
            self.assertIn("File: download complete", rendered)
            self.assertIn("File SHA-256: verified", rendered)
            self.assertIn("Next: Core import after baseline block header 935000 is available", rendered)
            self.assertNotIn("0.00%", next(line for line in rendered.splitlines() if "UTXO snapshot" in line))
            self.assertNotEqual(report["overall_state"], "READY")
            for failed_phase, failed_state in (("load_failed", "FAILED"), ("load_uncertain", "BLOCKED")):
                activation_path.write_text(json.dumps(dict(phase=failed_phase, updated_at=3)))
                failed = node.collect_node_progress(layout)
                self.assertEqual(failed["overall_state"], failed_state)
                self.assertIn("File SHA-256: verified", node.render_node_progress(failed, width=80))
                self.assertNotIn("Next: Core import after", node.render_node_progress(failed, width=80))
            activation_path.write_text(json.dumps(dict(phase="loading", updated_at=4, phase_started_at=native.timestamp(started))))
            report = node.collect_node_progress(layout)
            self.assertEqual(report["components"][0]["state"], "IMPORTING")
            self.assertIsNone(report["components"][0]["current"])
            self.assertIsNone(report["components"][0]["progress_percent"])
            self.assertIn("[RUN] UTXO snapshot", " ".join(node.render_node_progress(report, width=80).split()))
            self.assertEqual(report["components"][2]["label"], "Bitcoin (IBD)")
            self.assertNotIn("foreground=", report["components"][2]["detail"])
            core["rpc_available"] = False
            self.assertEqual(node.collect_node_progress(layout)["components"][2]["label"], "Bitcoin")
            del core["rpc_available"]
            log_path = Path(env["BTC_NODE_DATA_HOST_DIR"]) / "debug.log"
            base = layout.snapshot["contract"]["snapshot"]["base_hash"]
            log_path.write_text(f"2026-09-14T08:30:01Z [snapshot] loading 2000000 coins from snapshot {base}\n")
            for message, state, percent, phase in (
                ("[snapshot] 1000000 coins loaded (50.00%, 120 MB)", "IMPORTING", 50, "reading"),
                ("FlushSnapshotToDisk: flushing coins cache (120 MB) started", "IMPORTING", None, "flushing_cache"),
                (f"[snapshot] loaded 2000000 (120 MB) coins from snapshot {base}", "IMPORTING", None, "flushing"),
                ("FlushSnapshotToDisk: saving snapshot chainstate (120 MB) completed (100ms)", "VERIFYING", None, "verifying"),
                (f"[snapshot] successfully activated snapshot {base}", "VERIFYING", None, "activating"),
            ):
                with self.subTest(phase=phase):
                    with log_path.open("a") as output:
                        output.write(f"2026-09-14T08:30:02Z {message}\n")
                    report = node.collect_node_progress(layout)
                    snapshot = report["components"][0]
                    self.assertEqual(snapshot["state"], state)
                    self.assertEqual(snapshot["progress_percent"], percent)
                    self.assertEqual(snapshot["progress_phase"], "core_" + phase)
                    self.assertEqual(snapshot["progress_source"], "core_log")
                    rendered = node.render_node_progress(report, width=80)
                    self.assertIn("Stage elapsed=", rendered)
                    self.assertIn("File SHA-256: verified", rendered)
                    self.assertIn("Progress above: Core import stage; file download is complete", rendered)
                    self.assertNotEqual(report["overall_state"], "READY")
            # Neither activation logs nor even RPC readiness bypass the preparation exit gate.
            core.update(bootstrap_ready=True)
            self.assertEqual(node.collect_node_progress(layout)["components"][0]["state"], "STARTING")
            core.update(bootstrap_ready=True, tip_ready=True, active_height=966674, background_height=105439)
            services["btc-snapshot-bootstrap"].update(state="exited", exit_code=0)
            report = node.collect_node_progress(layout)
            self.assertEqual(report["components"][0]["state"], "READY")
            self.assertEqual(report["components"][2]["label"], "Bitcoin")
            core.update(bootstrap_ready=False, tip_ready=False, snapshot_active=True, phase="chain_changed_during_probe")
            report = node.collect_node_progress(layout)
            self.assertEqual(report["components"][0]["state"], "READY")
            self.assertEqual(report["components"][2]["label"], "Bitcoin")
            self.assertEqual(report["components"][2]["progress_phase"], "foreground")
            self.assertNotEqual(report["overall_state"], "READY")
            core.update(bootstrap_ready=True, tip_ready=True)
            for phase, fields, state in (("importing", dict(imported_coins=123456), "IMPORTING"),
                                         ("replaying", dict(height=949400, target=963800), "SYNCING"),
                                         ("waiting_for_blocks", dict(height=949400, target=963800), "SYNCING"),
                                         ("verifying", dict(height=963800), "VERIFYING"),
                                         ("sealed", dict(height=963800), "STARTING")):
                with self.subTest(phase=phase):
                    bh_path.write_text(json.dumps(dict(phase=phase, **fields)))
                    report = node.collect_node_progress(layout)
                    components = {item["id"]: item for item in report["components"]}
                    self.assertEqual(components["balance_history"]["state"], state)
                    self.assertEqual(components["bitcoin"]["state"], "READY")
                    self.assertNotEqual(report["overall_state"], "READY")
                    rendered = node.render_node_progress(report, width=80)
                    self.assertIn("Core background history: SYNCING 105439/935000", rendered)
                    if phase == "importing":
                        self.assertIn("123,456 UTXOs", rendered)
                        self.assertIsNone(components["balance_history"]["progress_percent"])
                    elif phase in {"replaying", "waiting_for_blocks"}:
                        lag = node.btc_registry_stable_lag_blocks(layout.network_identity["btc_activation_registry_id"])
                        self.assertEqual(components["balance_history"]["total"], core["headers"] - lag)
                        self.assertAlmostEqual(components["balance_history"]["progress_percent"],
                                               (949400 - 935000) * 100 / (core["headers"] - lag - 935000))
                        self.assertIn("Genesis 963800: replaying", rendered)
                    elif phase == "sealed":
                        self.assertIsNone(components["balance_history"]["progress_percent"])
                    else:
                        self.assertLess(components["balance_history"]["progress_percent"], 100)
            core.update(history_validated=True, background_height=None)
            self.assertIn("VALIDATED through baseline 935000", node.render_node_progress(node.collect_node_progress(layout), width=80, details=True))

    def test_file_milestone_requires_matching_completion_record_and_file(self):
        artifact = self.root / "snapshot.dat"
        artifact.write_bytes(b"x" * 20)
        expected = dict(base_height=935000, base_hash="a" * 64, file_sha256="b" * 64)
        record = dict(schema_version="usdb-bitcoin-assumeutxo:v1", phase="file_published",
                      identity=dict(snapshot={**expected, "size_bytes": 20}), details=dict(bytes=20, total_bytes=20))
        with mock.patch.object(native, "UTXO_SIZE", 20):
            self.assertEqual(native._snapshot_file_milestone(record, artifact, expected)["state"], "VERIFIED")
            for changes in (dict(phase="downloading"), dict(schema_version="old"), dict(identity=[]),
                            dict(identity=dict(snapshot={**expected, "file_sha256": "c" * 64, "size_bytes": 20})),
                            dict(details=dict(bytes=19, total_bytes=20)), dict(details=[])):
                with self.subTest(changes=changes):
                    self.assertIsNone(native._snapshot_file_milestone({**record, **changes}, artifact, expected))
            artifact.write_bytes(b"short")
            self.assertIsNone(native._snapshot_file_milestone(record, artifact, expected))
            artifact.unlink()
            self.assertIsNone(native._snapshot_file_milestone(record, artifact, expected))
            part = artifact.with_name(artifact.name + ".download") / "snapshot.part"
            part.parent.mkdir()
            part.write_bytes(b"x" * 20)
            self.assertIsNone(native._snapshot_file_milestone(record, artifact, expected))
            verifying = {**record, "phase": "verifying_file", "details": dict(bytes=0, total_bytes=20)}
            self.assertEqual(native._snapshot_file_milestone(verifying, artifact, expected)["state"], "VERIFYING")
            part.unlink()
            target = self.root / "other-file"
            target.write_bytes(b"x" * 20)
            artifact.symlink_to(target)
            self.assertIsNone(native._snapshot_file_milestone(record, artifact, expected))

    def test_failed_or_uncertain_core_import_is_visible_in_progress(self):
        layout = native_kit(self.root)
        self.configure(layout)
        env = node.read_env(layout.node_env)
        activation = Path(env["BTC_ASSUMEUTXO_STATE_HOST_DIR"]) / "activation.json"
        services = {"btc-node": dict(state="running"), "btc-snapshot-bootstrap": dict(state="running")}
        core = {}
        with mock.patch.object(node, "_collect_compose_services", return_value=services), \
             mock.patch.object(native, "core_progress", return_value=core), \
             mock.patch.object(node, "_read_service_readiness", return_value=(None, "RPC unavailable")), \
             mock.patch.object(node, "_chain_component", return_value=node._component_progress("usdb_chain", "WAITING", "not started")), \
             mock.patch.object(node, "_resource_progress", return_value=({}, False)), \
             mock.patch.object(node, "controller_observed_state", return_value="FAILED"):
            for phase, state in (("load_uncertain", "BLOCKED"), ("load_failed", "FAILED")):
                activation.write_text(json.dumps(dict(phase=phase, updated_at=1)))
                self.assertEqual(node.collect_node_progress(layout)["overall_state"], state)
            activation.write_text(json.dumps(dict(phase="snapshot_active", updated_at=2)))
            services["btc-snapshot-bootstrap"].update(state="exited", exit_code=0)
            core.update(error="Core RPC timed out", error_kind="rpc_unavailable", rpc_available=False)
            snapshot = node.collect_node_progress(layout)["components"][0]
            self.assertEqual(snapshot["state"], "STARTING")
            self.assertEqual(snapshot["display_state"], "UNAVAILABLE")
            self.assertTrue(snapshot["observation_unavailable"])
            self.assertIsNone(snapshot["progress_percent"])
            core.update(error="Core baseline mismatch", error_kind="identity_or_configuration")
            self.assertEqual(node.collect_node_progress(layout)["components"][0]["state"], "BLOCKED")

    def test_completed_preparation_survives_rpc_outage_without_claiming_core_readiness(self):
        layout = native_kit(self.root)
        self.configure(layout)
        env = node.read_env(layout.node_env)
        activation = Path(env["BTC_ASSUMEUTXO_STATE_HOST_DIR"]) / "activation.json"
        record = dict(schema_version="usdb-bitcoin-assumeutxo:v1", phase="snapshot_active", updated_at=2,
                      identity=dict(snapshot=layout.snapshot["contract"]["snapshot"]),
                      details=dict(report=dict(bootstrap_ready=True)))
        activation.write_text(json.dumps(record))
        services = {"btc-snapshot-bootstrap": dict(state="exited", exit_code=0),
                    "btc-node": dict(state="running", started_at="2026-09-15T04:46:47Z")}
        core = dict(error="Core RPC timed out", error_kind="rpc_unavailable", rpc_available=False)
        with ControllerStatusFixture() as controller, \
             mock.patch.object(node, "_collect_compose_services", return_value=services), \
             mock.patch.object(native, "core_progress", return_value=core) as probe, \
             mock.patch.object(node, "_read_service_readiness", return_value=(None, "RPC unavailable")), \
             mock.patch.object(node, "_chain_component", return_value=node._component_progress("usdb_chain", "WAITING", "not started")), \
             mock.patch.object(node, "_resource_progress", return_value=({}, False)):
            controller.layout = layout
            controller.write_unit()
            controller.properties.update(ActiveState="failed", Result="exit-code", ExecMainCode="1", ExecMainStatus="1")
            # A newly opened watch has no memory cache; the completed job is still observable.
            for failure in (None, native.CoreProbeError("Incompatible Core response")):
                probe.side_effect = failure
                report = node.collect_node_progress(layout)
                snapshot, bitcoin = report["components"][0], report["components"][2]
                self.assertEqual(snapshot["state"], "READY")
                self.assertEqual(snapshot["progress_percent"], 100)
                self.assertEqual(snapshot["completion_source"], "bootstrap_job")
                self.assertEqual(bitcoin["state"], "STARTING")
                self.assertEqual(bitcoin["display_state"], "UNAVAILABLE")
                self.assertNotEqual(report["overall_state"], "READY")
                self.assertFalse(report["native_bootstrap"]["core"].get("bootstrap_ready", False))
                self.assertIn("controller=failed", node.render_node_progress(report))
            probe.side_effect = None
            for state, exit_code in (("running", None), ("created", None), ("exited", 1)):
                services["btc-snapshot-bootstrap"].update(state=state, exit_code=exit_code)
                self.assertNotEqual(node.collect_node_progress(layout)["components"][0]["state"], "READY")
            services["btc-snapshot-bootstrap"].update(state="exited", exit_code=0)
            wrong_snapshot = {**record["identity"]["snapshot"], "base_hash": "f" * 64}
            for fields in (dict(identity={}), dict(identity=dict(snapshot=wrong_snapshot)),
                           dict(phase="loading"), dict(details={}), dict(schema_version="old")):
                activation.write_text(json.dumps({**record, **fields}))
                self.assertNotEqual(node.collect_node_progress(layout)["components"][0]["state"], "READY")
            activation.write_text(json.dumps(record))
            probe.return_value = dict(rpc_available=True, bootstrap_ready=False, snapshot_active=False,
                                      tip_ready=False, active_height=100, headers=967114)
            self.assertNotEqual(node.collect_node_progress(layout)["components"][0]["state"], "READY")
            probe.return_value = core
            core.update(error="Core baseline mismatch", error_kind="identity_or_configuration")
            self.assertEqual(node.collect_node_progress(layout)["components"][0]["state"], "BLOCKED")
            self.assertEqual(node.collect_node_progress(layout)["components"][2]["state"], "BLOCKED")
            core.update(error="Core RPC timed out", error_kind="rpc_unavailable")
            services["btc-node"].update(state="exited")
            bitcoin = node.collect_node_progress(layout)["components"][2]
            self.assertEqual(bitcoin["state"], "FAILED")
            self.assertNotIn("display_state", bitcoin)

    def test_balance_history_keeps_one_range_through_genesis_and_live_catchup(self):
        core = dict(headers=966950)
        percentages = []
        for phase, height, readiness, state, milestone in (
            ("replaying", 963799, None, "SYNCING", "replaying"),
            ("verifying", 963800, None, "VERIFYING", "verifying"),
            ("Loading", 963800, dict(stable_height=963800, phase="Loading", query_ready=False), "SYNCING", "initializing"),
            ("Indexing", 963820, dict(stable_height=963820, phase="Indexing", query_ready=True), "SYNCING", "available"),
            ("Synced", 966940, dict(stable_height=966940, phase="Synced", query_ready=True), "READY", "available"),
        ):
            with self.subTest(phase=phase):
                original = node._component_progress("balance_history", state, "RPC progress")
                item = native._balance_history_progress(original, readiness, dict(phase=phase, height=height), core, {},
                                                        base=935000, origin=963800, stable_lag=10)
                self.assertEqual(item["current"], height)
                self.assertEqual(item["total"], 966940)
                self.assertEqual(item["genesis_milestone"]["state"], milestone)
                self.assertEqual(item["state"], state)
                percentages.append(item["progress_percent"])
        self.assertEqual(percentages, sorted(percentages))
        self.assertLess(percentages[1], 100)
        self.assertEqual(percentages[-1], 100)

        # The completed journal is a baseline checkpoint, not a live height.
        report = dict(release_id="test", observed_at="now", overall_state="SYNCING", components=[item])
        history = node.NodeProgressHistory()
        history.apply(report, observed_monotonic=0)
        unavailable = native._balance_history_progress(node._component_progress("balance_history", "STARTING", "RPC unavailable"),
            None, dict(phase="sealed", height=963800), core, {}, base=935000, origin=963800, stable_lag=10)
        self.assertIsNone(unavailable["current"])
        self.assertIn("sealed", unavailable["genesis_milestone"]["state"])
        retained = history.apply({**report, "components": [unavailable]}, observed_monotonic=5)["components"][0]
        self.assertEqual(retained["current"], 966940)
        self.assertEqual(retained["progress_percent"], 100)
        self.assertEqual(retained["state"], "STARTING")
        self.assertIn("STALE", retained["detail"])
        self.assertIn("last observed", retained["genesis_milestone"]["state"])

    def test_balance_history_labels_unknown_targets_and_waits_for_bitcoin_when_ahead_of_available_blocks(self):
        for core, activation, target, source in (
            ({}, {}, None, "unavailable"),
            ({}, dict(details=dict(report=dict(headers=966950))), 966940, "last_observed_bitcoin_headers"),
        ):
            item = native._balance_history_progress(node._component_progress("balance_history", "SYNCING", "replay"), None,
                dict(phase="replaying", height=943860), core, activation, base=935000, origin=963800, stable_lag=10)
            self.assertEqual(item["total"], target)
            self.assertEqual(item["sync_target_source"], source)
            self.assertEqual(item["genesis_milestone"]["remaining_blocks"], 19940)
            if target is None:
                self.assertIsNone(item["progress_percent"])
        item = native._balance_history_progress(node._component_progress("balance_history", "READY", "ready"),
            dict(stable_height=964000, phase="Synced", query_ready=True, total=964000), {}, dict(headers=966950), {},
            base=935000, origin=963800, stable_lag=10)
        self.assertEqual(item["state"], "WAITING")
        self.assertIn("waiting for Bitcoin", item["detail"])
        capped = native._balance_history_progress(node._component_progress("balance_history", "READY", "ready"),
            dict(stable_height=964000, phase="Synced", query_ready=True, total=964000), {}, dict(headers=966950), {},
            base=935000, origin=963800, stable_lag=10, max_height=964000)
        self.assertEqual(capped["state"], "READY")
        self.assertEqual(capped["progress_percent"], 100)
        self.assertEqual(capped["total"], 964000)

    def test_signed_bundle_retains_public_trust_and_rejects_tampering(self):
        keys = self.root / "keys"
        signing.keygen(keys, "native-test", "bitcoin-assumeutxo")
        identity = artifacts.utxo_identity()
        value = dict(schema_version=signing.SCHEMA, artifact_type="bitcoin-assumeutxo", identity=identity,
                     file=dict(name=artifacts.UTXO_FILE, sha256=identity["file_sha256"], size_bytes=artifacts.UTXO_SIZE),
                     upstream_verification=None, signature_scheme="ed25519", signing_key_id="native-test")
        trust = keys / "trusted-keys.json"
        manifest = signing.write_release(self.root / "signed", value, signing.sign(value, keys / "signing-key.json", trust))
        layout = native_kit(self.root, manifest=manifest, trusted=trust)
        self.configure(layout)
        env = node.read_env(layout.node_env)
        self.assertEqual(env["BTC_ASSUMEUTXO_DISTRIBUTION_MODE"], "usdb-signed")
        self.assertEqual(env["BTC_ASSUMEUTXO_MANIFEST_FILE"], "/network/assumeutxo-distribution.json")
        self.assertFalse(list(layout.kit_root.rglob("signing-key.json")))
        packaged = layout.bundle_dir / "artifacts/assumeutxo-distribution.json.sig"
        packaged.write_bytes(b"x" * 64)
        with self.assertRaisesRegex(ValueError, "mismatch"):
            node.load_release_layout(layout.kit_root, node_env=layout.node_env)

    @unittest.skipUnless(shutil.which("docker"), "Docker Compose is required for graph validation")
    def test_rendered_compose_graph_has_native_mounts_and_no_legacy_dependencies(self):
        layout = native_kit(self.root)
        self.configure(layout)
        docker = layout.kit_root / "docker"
        base = ["docker", "compose", "--env-file", str(layout.bundle_dir / "network.env"), "--env-file", str(layout.node_env)]
        env = {**os.environ, "USDB_NETWORK_ARTIFACTS_DIR": str(layout.bundle_dir / "artifacts"),
               "BH_SNAPSHOT_TRUST_HOST_DIR": str(layout.bundle_dir / "trust"),
               "BTC_BOOTSTRAP_BUNDLE_ARTIFACTS_DIR": str(layout.bundle_dir / "artifacts"),
               "BTC_BOOTSTRAP_TRUST_DIR": str(layout.bundle_dir / "trust")}
        for files in ((docker / "compose.runtime.yml", layout.bundle_dir / "compose.network.yml", docker / "compose.runtime-assumeutxo.yml"),
                      (docker / "compose.bitcoin.yml", docker / "compose.bitcoin-assumeutxo.yml")):
            command = base + [arg for path in files for arg in ("-f", str(path))] + ["config", "--format", "json"]
            result = subprocess.run(command, capture_output=True, text=True, env=env, timeout=30)
            self.assertEqual(result.returncode, 0, result.stderr)
            services = json.loads(result.stdout)["services"]
            if "balance-history" in services:
                for name in ("snapshot-loader", "script-registry-installer", "paired-checkpoint-recovery"):
                    self.assertNotIn(name, services)
                for name in ("balance-history", "usdb-indexer"):
                    self.assertFalse(services[name].get("depends_on"))
                volumes = {item["target"]: item for item in services["balance-history"]["volumes"]}
                self.assertEqual(set(volumes), {"/data/balance-history", "/data/bitcoin", "/data/assumeutxo"})
                self.assertTrue(volumes["/data/bitcoin"]["read_only"])
                self.assertTrue(volumes["/data/assumeutxo"]["read_only"])
                self.assertEqual(set(services["usdb-chain"]["depends_on"]), {"usdb-chain-init", "usdb-indexer"})
                console = services["usdb-control-plane"]
                self.assertFalse(console.get("depends_on"))
                console_mounts = {item["target"]: item for item in console["volumes"]}
                self.assertTrue(console_mounts["/run/usdb-console"]["read_only"])
                self.assertNotIn("/var/run/docker.sock", console_mounts)
                self.assertEqual(console["ports"][0]["host_ip"], "127.0.0.1")
            else:
                self.assertEqual(services["btc-node"]["environment"]["BTC_TXINDEX"], "0")
                self.assertEqual(int(services["btc-snapshot-bootstrap"]["mem_limit"]), 128 * policy.MIB)
                self.assertIn("--ensure-snapshot-file", services["btc-snapshot-bootstrap"]["command"])
                self.assertIn("--reuse-active-snapshot-file", services["btc-snapshot-bootstrap"]["command"])


class NativeControllerTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory(prefix="usdb-native-controller-")
        self.addCleanup(temporary.cleanup)
        self.runtime = NativeRuntime(Path(temporary.name))
        r = self.runtime
        for patcher in (mock.patch.object(node, "effective_memory_bytes", return_value=r.memory),
                        mock.patch.object(node, "_resource_containers", side_effect=r.observed),
                        mock.patch.object(node, "run_helper", side_effect=r.helper),
                        mock.patch.object(node, "_read_service_readiness", side_effect=r.readiness),
                        mock.patch.object(node, "_runtime_lifecycle_status", return_value=dict(state="ready")),
                        mock.patch.object(node, "_print_startup_phase"),
                        mock.patch.object(native.time, "monotonic", side_effect=lambda: r.tick),
                        mock.patch.object(native.time, "sleep", side_effect=r.sleep)):
            patcher.start()
            self.addCleanup(patcher.stop)

    def start(self, timeout=30):
        native.start_native_node(self.runtime.layout, sync_timeout_secs=timeout, output_to_stderr=False,
                                 progress_monitor=SimpleNamespace(set_phase=lambda _: None))

    def complete_after_data(self):
        r = self.runtime
        if "balance-history" in r.containers:
            r.core["tip_ready"], r.ready = True, True

    def test_starts_data_before_origin_or_tip_and_background_never_gates_chain(self):
        r = self.runtime
        with self.assertRaisesRegex(ValueError, "timed out"):
            self.start()
        self.assertIn(("native-start-data", "overlap"), r.events)
        self.assertNotIn("usdb-chain", r.containers)
        self.assertEqual(r.core["active_height"], 935000)
        r.core["tip_ready"], r.ready = True, True
        self.start()
        self.assertIn(("up-chain", "steady"), r.events)
        self.assertFalse(r.core["history_validated"])
        self.assertEqual(node._read_resource_state(r.layout)["recover_services"], [])

    def test_resource_handoff_recovers_after_core_stop(self):
        r = self.runtime
        r.crash = "down"
        with self.assertRaisesRegex(RuntimeError, "after down"):
            self.start()
        self.assertTrue(node._read_resource_state(r.layout)["pending"])
        r.advance = self.complete_after_data
        self.start(timeout=60)
        self.assertEqual(node.read_env(r.layout.node_env)["USDB_RESOURCE_PHASE"], "steady")
        self.assertFalse(node._read_resource_state(r.layout)["pending"])

    def test_small_native_node_keeps_core_headroom_while_chain_starts(self):
        r = self.runtime
        r.memory = 32 * policy.GIB
        env = node.read_env(r.layout.node_env)
        env.update(policy.build_resource_plan(r.memory, "bitcoin", env).environment())
        r.layout.node_env.write_text(node.upsert_env("", env))
        r.advance = self.complete_after_data
        with mock.patch.object(node, "effective_memory_bytes", return_value=r.memory):
            self.start(timeout=60)
            configured = node.read_env(r.layout.node_env)
            node._check_running_resource_budget(configured, r.containers)
        self.assertFalse(r.core["history_validated"])
        self.assertEqual(r.containers["usdb-chain"]["state"], "running")
        self.assertEqual(r.containers["btc-node"]["memory"], 16 * policy.GIB)
        self.assertEqual(r.containers["btc-node"]["environment"]["BTC_DBCACHE_MB"], "2048")
        self.assertEqual(r.containers["balance-history"]["memory"], 4 * policy.GIB)
        self.assertTrue(node._resource_container_matches(r.containers["balance-history"], configured, "balance-history"))

    def test_native_defaults_are_persisted_without_overriding_operator_caps(self):
        native_caps = node._resource_policy_updates("auto", {"SNAPSHOT_MODE": "assumeutxo"})
        self.assertEqual(native_caps["USDB_BTC_STEADY_MEMORY_CAP"], "16g")
        legacy_caps = node._resource_policy_updates("auto", {})
        self.assertEqual(legacy_caps["USDB_BTC_STEADY_MEMORY_CAP"], "8g")
        custom_caps = node._resource_policy_updates("auto", {"SNAPSHOT_MODE": "assumeutxo", "USDB_BTC_STEADY_MEMORY_CAP": "8g"})
        self.assertEqual(custom_caps["USDB_BTC_STEADY_MEMORY_CAP"], "8g")

    def test_partial_data_start_is_adopted_after_controller_restart(self):
        r = self.runtime
        r.crash = "native-start-data"
        with self.assertRaisesRegex(RuntimeError, "after native-start-data"):
            self.start()
        r.advance = self.complete_after_data
        self.start(timeout=60)
        self.assertEqual(r.events.count(("native-start-data", "overlap")), 1)

    def test_observer_failure_and_wrong_core_identity_never_start_data(self):
        r = self.runtime
        r.containers["btc-node"] = r.container("btc-node")
        r.containers["btc-snapshot-bootstrap"] = {**r.container("btc-snapshot-bootstrap"), "state": "exited", "exit_code": 1}
        with self.assertRaisesRegex(ValueError, "preparation failed"):
            self.start()
        r.containers["btc-snapshot-bootstrap"]["exit_code"] = 0
        r.core["error"] = "Core canonical baseline hash mismatch"
        with self.assertRaisesRegex(ValueError, "canonical baseline"):
            self.start()
        self.assertFalse(any(action == "native-start-data" for action, _ in r.events))

    def test_no_baseline_keeps_services_stopped_and_observer_has_a_budget(self):
        r = self.runtime
        r.core["bootstrap_ready"] = False
        with self.assertRaisesRegex(ValueError, "timed out"):
            self.start()
        self.assertNotIn("balance-history", r.containers)
        env = node.read_env(r.layout.node_env)
        r.containers["btc-snapshot-bootstrap"] = r.container("btc-snapshot-bootstrap")
        node._check_running_resource_budget(env, r.containers)
        r.containers["btc-snapshot-bootstrap"]["memory"] *= 2
        with self.assertRaisesRegex(ValueError, "stale"):
            node._check_running_resource_budget(env, r.containers)

    def test_core_failure_during_import_is_not_hidden_as_rpc_warmup(self):
        r = self.runtime
        r.core["bootstrap_ready"] = False
        r.advance = lambda: r.containers["btc-node"].update(state="exited", exit_code=1)
        with self.assertRaisesRegex(ValueError, "Core stopped"):
            self.start()


if __name__ == "__main__":
    unittest.main()
