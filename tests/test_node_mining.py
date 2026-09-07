#!/usr/bin/env python3
"""Accept mining preflight, durable interruption recovery, and chain-only changes."""
from copy import deepcopy
import json
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import usdb_node as NODE
import usdb_mining as MINING
from common.mining import ADDRESS, PASS_ID, SEED, MiningFixture, RuntimeScriptFixture, RuntimeHelperFixture


class Interrupted(BaseException):
    """Model process death without invoking normal failure rollback."""


class MiningTests(unittest.TestCase):
    def test_private_geth_files_preserve_binding_across_chain_stop_and_restart(self):
        with MiningFixture() as f:
            expected = MINING.binding(f.layout, f.env)
            with f.protected_files() as probes:
                self.assertEqual(MINING.binding(f.layout, f.env), expected)
                f.enable()
                self.assertEqual(f.run(), 0)
                self.assertEqual(MINING.read_state(f.layout)["phase"], "APPLIED")
                self.assertEqual(MINING.binding(f.layout, f.env), expected)
                f.fail_rpc = "get_readiness"
                MINING.submit(f.layout, disable=True, yes=True)
                self.assertEqual(f.run(), 0)
            self.assertEqual(f.container_number, 2)
            self.assertTrue(any(action == "binding" and state == "exited" for action, state, _ in probes))
            self.assertNotIn("test-node-key", json.dumps(probes))
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_NODE_ROLE"], "full")

    def test_readable_directory_with_private_nodekey_uses_same_binding(self):
        with MiningFixture() as f:
            expected = MINING.binding(f.layout, f.env)
            with f.protected_files(key_only=True) as probes:
                self.assertEqual(MINING.binding(f.layout, f.env), expected)
            self.assertEqual(len(probes), 1)

    def test_private_data_replacement_after_stop_still_refuses_restart(self):
        with MiningFixture() as f:
            key = Path(f.env["USDB_CHAIN_DATA_HOST_DIR"], "geth/nodekey")
            def replace(action):
                if action == "stop-chain":
                    # Simulate another actor replacing the file outside the denied host reader.
                    with open(key, "w") as output:
                        output.write("replacement")
            f.after_helper = replace
            with f.protected_files():
                f.enable()
                self.assertEqual(f.run(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.container_number, 0)
            self.assertEqual(MINING.read_state(f.layout)["rollback"], "refused_identity_or_config_change")

    def test_private_missing_database_is_not_treated_as_permission_success(self):
        with MiningFixture() as f:
            Path(f.env["USDB_CHAIN_DATA_HOST_DIR"], "geth/chaindata/CURRENT").unlink()
            before = f.layout.node_env.read_bytes()
            with f.protected_files(), self.assertRaisesRegex(ValueError, "CHAIN_NOT_INITIALIZED"):
                f.enable()
            self.assertEqual(f.layout.node_env.read_bytes(), before)
            self.assertFalse(MINING.state_path(f.layout).exists())
            self.assertEqual(f.calls, [])

    def test_private_recovery_markers_preserve_halt_and_epoch_checks(self):
        for baseline, halted, error in ((0, False, None), (1, False, "DEEP_REORG_EPOCH_CHANGED"),
                                       (0, True, "DEEP_REORG_HALTED"),
                                       ("broken", False, "CHAIN_DATA_INSPECTION_FAILED")):
            with self.subTest(baseline=baseline, halted=halted), MiningFixture() as f:
                guard = Path(f.env["USDB_CHAIN_DATA_HOST_DIR"], "recovery/deep-btc-reorg")
                guard.mkdir(parents=True)
                (guard / "baseline.json").write_text(json.dumps({"upstream_reorg_epoch": baseline}))
                if halted:
                    (guard / "halted.json").write_text("{}")
                before = {p.name: p.read_bytes() for p in guard.iterdir()}
                with f.protected_files():
                    if error:
                        with self.assertRaisesRegex(ValueError, error):
                            MINING.preflight(f.layout, ADDRESS, first_node=True)
                    else:
                        MINING.preflight(f.layout, ADDRESS, first_node=True)
                self.assertEqual({p.name: p.read_bytes() for p in guard.iterdir()}, before)
                self.assertEqual(f.calls, [])

    def test_failed_private_probe_cannot_submit_or_change_config(self):
        failures = [FileNotFoundError("docker unavailable"), subprocess.TimeoutExpired("docker", 30),
                    subprocess.CompletedProcess([], 1, "", "cached image unavailable"),
                    subprocess.CompletedProcess([], 0, "{}", "")]
        for failure in failures:
            with self.subTest(failure=failure), MiningFixture() as f:
                before = f.layout.node_env.read_bytes()
                response = {"side_effect": failure} if isinstance(failure, Exception) else {"return_value": failure}
                with f.protected_files(), mock.patch.object(MINING.subprocess, "run", **response), \
                        self.assertRaisesRegex(ValueError, "CHAIN_DATA_INSPECTION_FAILED"):
                    f.enable()
                self.assertEqual(f.layout.node_env.read_bytes(), before)
                self.assertFalse(MINING.state_path(f.layout).exists())
                self.assertEqual(f.calls, [])

    def test_zero_energy_without_registry_is_eligible_and_selector_is_pinned(self):
        with MiningFixture() as f:
            plan = MINING.preflight(f.layout, ADDRESS, first_node=True)
            self.assertEqual(plan["candidate"]["pass"]["effective_energy"], "0")
            self.assertEqual(plan["threads"], 1)
            self.assertEqual(f.calls, [])
            query = next(params for method, params in f.rpc_calls if method == "resolve_miner_candidate")[0]
            self.assertEqual(query["context"]["expected_state"]["snapshot_id"], f.ready["upstream_snapshot_id"])
            self.assertEqual(query["context"]["requested_height"], 100)

    def test_preflight_rejections_leave_config_and_containers_unchanged(self):
        changes = [lambda f: f.candidate["pass"].update(state="consumed"),
                   lambda f: f.candidate["pass"].update(pass_kind="collab"),
                   lambda f: f.candidate["pass"].update(usdb_main="0x" + "11" * 20),
                   lambda f: f.candidate["external_state"].update(snapshot_id="aa" * 32),
                   lambda f: f.candidate.update(selection_rule="wrong-rule"),
                   lambda f: f.ready.update(consensus_ready=False),
                   lambda f: setattr(f, "fail_rpc", "resolve_miner_candidate")]
        for change in changes:
            with self.subTest(change=change), MiningFixture() as f:
                before = f.layout.node_env.read_bytes()
                change(f)
                with self.assertRaises(ValueError):
                    f.enable()
                self.assertEqual(f.layout.node_env.read_bytes(), before)
                self.assertEqual(f.calls, [])
                self.assertFalse(MINING.state_path(f.layout).exists())

    def test_invalid_addresses_and_zero_threads_rejected(self):
        for address in ("0x0", "0x" + "00" * 20, "0x" + "gg" * 20):
            with MiningFixture() as f, self.assertRaisesRegex(ValueError, "INVALID_ADDRESS"):
                MINING.preflight(f.layout, address, first_node=True)
        with MiningFixture() as f, self.assertRaisesRegex(ValueError, "INVALID_THREADS"):
            MINING.preflight(f.layout, ADDRESS, first_node=True, threads=0)

    def test_peer_source_required_and_joiner_never_falls_back(self):
        with MiningFixture() as f:
            with self.assertRaisesRegex(ValueError, "PEER_SOURCE_REQUIRED"):
                MINING.preflight(f.layout, ADDRESS)
            f.update_env(USDB_BOOTNODES=SEED)
            with self.assertRaisesRegex(ValueError, "FIRST_NODE_CONFLICT"):
                MINING.preflight(f.layout, ADDRESS, first_node=True)
            with self.assertRaisesRegex(ValueError, "PEERS_UNREACHABLE"):
                MINING.preflight(f.layout, ADDRESS)
            f.chain["peers"] = 1
            self.assertEqual(MINING.preflight(f.layout, ADDRESS)["peer"]["mode"], "join")
            f.chain["syncing"] = {"currentBlock": "0x0"}
            with self.assertRaisesRegex(ValueError, "CHAIN_SYNCING"):
                MINING.preflight(f.layout, ADDRESS)

    def test_first_node_cannot_be_newly_declared_after_genesis(self):
        with MiningFixture() as f:
            f.chain["height"] = 1
            with self.assertRaisesRegex(ValueError, "FIRST_NODE_CONFLICT"):
                f.enable()

    def test_enable_recreates_only_chain_and_repeat_is_noop_after_blocks(self):
        with MiningFixture() as f:
            f.enable()
            self.assertEqual(f.run(), 0)
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_NODE_ROLE"], "miner")
            self.assertEqual(f.calls, [("run_testnet_runtime.sh", ("stop-chain",)),
                                      ("run_testnet_runtime.sh", ("recreate-chain",))])
            self.assertNotIn("fixture-secret", MINING.state_path(f.layout).read_text())
            f.chain["height"] = 4
            self.assertEqual(MINING.submit(f.layout, address=ADDRESS, yes=True)["outcome"], "already_applied")
            self.assertEqual(f.container_number, 1)
            self.assertEqual(MINING.observe(f.layout)["state"], "ACTIVE")

    def test_disable_is_persistent_with_unavailable_upstream_and_pass(self):
        with MiningFixture() as f:
            f.enable()
            f.run()
            f.fail_rpc = "get_readiness"
            MINING.submit(f.layout, disable=True, yes=True)
            self.assertEqual(f.run(), 0)
            self.assertEqual(MINING.observe(f.layout)["state"], "DISABLED")
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_NODE_ROLE"], "full")

    def test_disable_cancels_pending_enable_and_preserves_halt_latch(self):
        with MiningFixture() as f:
            f.enable()
            previous = MINING.read_state(f.layout)["operation_id"]
            halted = Path(f.env["USDB_CHAIN_DATA_HOST_DIR"]) / "recovery/deep-btc-reorg/halted.json"
            halted.parent.mkdir(parents=True)
            halted.write_text('{"reason":"reorg"}')
            MINING.submit(f.layout, disable=True, yes=True)
            self.assertEqual(f.run(), 0)
            state = MINING.read_state(f.layout)
            self.assertEqual(state["cancelled_operation_id"], previous)
            self.assertEqual(state["result"], "disabled_chain_halted")
            self.assertTrue(halted.exists())
            self.assertEqual(f.container_number, 0)
            self.assertEqual(f.runtime["state"], "exited")

    def test_every_durable_phase_resumes_after_process_death(self):
        for crash_phase in ("STOPPING", "STOPPED", "CONFIGURED", "STARTING", "APPLIED"):
            with self.subTest(phase=crash_phase), MiningFixture() as f:
                f.enable()
                original = MINING._phase
                def phase(layout, operation, value, **fields):
                    original(layout, operation, value, **fields)
                    if value == crash_phase:
                        raise Interrupted()
                with mock.patch.object(MINING, "_phase", side_effect=phase), self.assertRaises(Interrupted):
                    f.run()
                self.assertEqual(f.run(), 0)
                self.assertEqual(MINING.read_state(f.layout)["phase"], "APPLIED")
                self.assertEqual(f.container_number, 1)

    def test_death_after_docker_create_adopts_same_container(self):
        with MiningFixture() as f:
            f.enable()
            def die(action):
                if action == "recreate-chain":
                    raise Interrupted()
            f.after_helper = die
            with self.assertRaises(Interrupted):
                f.run()
            f.after_helper = None
            self.assertEqual(f.run(), 0)
            self.assertEqual(f.container_number, 1)

    def test_death_after_role_write_before_journal_resumes(self):
        with MiningFixture() as f:
            f.enable()
            original = MINING._write_role
            def write(*args):
                original(*args)
                raise Interrupted()
            with mock.patch.object(MINING, "_write_role", side_effect=write), self.assertRaises(Interrupted):
                f.run()
            self.assertEqual(MINING.read_state(f.layout)["phase"], "STOPPED")
            self.assertEqual(f.run(), 0)

    def test_start_failure_rolls_back_and_reports_actual_failure(self):
        with MiningFixture() as f:
            f.enable()
            f.fail_helper = "recreate-chain"
            self.assertEqual(f.run(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(MINING.read_state(f.layout)["phase"], "FAILED")
            self.assertEqual(MINING.read_state(f.layout)["rollback"], "failed")
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_NODE_ROLE"], "full")
            self.assertEqual(MINING.observe(f.layout)["state"], "FAILED")

    def test_work_warming_up_is_not_failure_and_chain_height_is_not_local_seal(self):
        with MiningFixture() as f:
            f.enable()
            f.run()
            f.work_available = False
            f.chain["height"] = 99
            report = MINING.observe(f.layout)
            self.assertEqual(report["state"], "WARMING_UP")
            self.assertTrue(report["applied"])
            self.assertNotIn("last_local_seal", report)
            self.assertEqual(f.container_number, 1)

    def test_rpc_start_timeout_is_pending_without_recreate_loop(self):
        with MiningFixture() as f:
            f.enable()
            f.fail_rpc = "eth_mining"
            self.assertEqual(f.run(), 1)
            self.assertEqual(f.run(), 1)
            self.assertEqual(f.container_number, 1)
            self.assertEqual(MINING.read_state(f.layout)["phase"], "STARTING")
            f.fail_rpc = None
            self.assertEqual(f.run(), 0)

    def test_role_drift_and_old_set_role_do_not_bypass_gate(self):
        with MiningFixture() as f:
            f.update_env(USDB_NODE_ROLE="miner", USDB_MINER_ADDRESS=ADDRESS)
            self.assertTrue(MINING.observe(f.layout)["drift"])
            with self.assertRaisesRegex(ValueError, "MINING_AUTHORIZATION_REQUIRED"):
                MINING.validate_start(f.layout)
            with self.assertRaisesRegex(ValueError, "mining enable"):
                NODE.set_role(f.layout, role="miner", miner_address=ADDRESS, miner_threads=1)

    def test_changed_data_identity_never_stops_or_mutates_new_database(self):
        with MiningFixture() as f:
            f.enable()
            Path(f.env["USDB_CHAIN_DATA_HOST_DIR"], "geth/nodekey").write_text("replacement-key")
            self.assertEqual(f.run(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.calls, [])
            self.assertEqual(NODE.read_env(f.layout.node_env)["USDB_NODE_ROLE"], "full")

    def test_reorg_after_plan_fails_before_stop(self):
        with MiningFixture() as f:
            f.enable()
            f.ready["upstream_reorg_epoch"] = 1
            self.assertEqual(f.run(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.calls, [])

    def test_reorg_during_candidate_query_rejected(self):
        with MiningFixture() as f:
            def reorg(method):
                if method == "resolve_miner_candidate":
                    f.ready["upstream_reorg_epoch"] += 1
            f.after_rpc = reorg
            with self.assertRaisesRegex(ValueError, "REORG_DURING_CHECK"):
                f.enable()

    def test_duplicate_enable_attaches_and_other_target_is_rejected(self):
        with MiningFixture() as f:
            first = f.enable()
            again = f.enable()
            self.assertEqual(first["operation_id"], again["operation_id"])
            with self.assertRaisesRegex(ValueError, "MINING_OPERATION_BUSY"):
                MINING.submit(f.layout, address="0x" + "11" * 20, first_node=True, yes=True)

    def test_expect_pass_is_assertion_and_cli_defaults_one_worker(self):
        with MiningFixture() as f:
            with self.assertRaisesRegex(ValueError, "UNEXPECTED_PASS"):
                MINING.preflight(f.layout, ADDRESS, first_node=True, expect_pass="00" * 32 + "i0")
        parsed = NODE.build_parser().parse_args(["mining", "enable", "--address", ADDRESS, "--first-node", "--yes"])
        self.assertEqual(parsed.threads, 1)
        self.assertTrue(parsed.first_node)


    def test_duplicate_enable_attaches_while_controller_holds_lock(self):
        with MiningFixture() as f:
            operation = f.enable()
            with NODE.node_operation_lock(f.layout, "mining"):
                attached = f.enable()
            self.assertEqual(attached["operation_id"], operation["operation_id"])
            self.assertEqual(f.calls, [])

    def test_mixed_case_checksum_is_checked_with_keccak_not_sha3(self):
        with MiningFixture() as f:
            address = "0x5aAeb6053F3E94C9b9A09f33669435E7Ef1BeAed"
            # Each alphabetic nibble's upper/lower bit is the EIP-55 condition.
            digest = "0x" + "".join("8" if c.isupper() else "0" for c in address[2:]) + "0" * 24
            with mock.patch.object(MINING, "rpc", return_value=digest) as rpc:
                self.assertEqual(MINING.address_check(f.layout, address), address.lower())
                self.assertEqual(rpc.call_args.args[1], "web3_sha3")
                with self.assertRaisesRegex(ValueError, "checksum"):
                    MINING.address_check(f.layout, address.replace("aAe", "aae"))

    def test_resource_quota_and_conflicting_arguments_fail_before_stop(self):
        with MiningFixture() as f:
            f.runtime["nano_cpus"] = 1500000000
            with self.assertRaisesRegex(ValueError, "INVALID_THREADS"):
                MINING.preflight(f.layout, ADDRESS, first_node=True, threads=2)
            f.update_env(USDB_CHAIN_EXTRA_ARGS="--miner.etherbase 0x" + "11" * 20)
            with self.assertRaisesRegex(ValueError, "CONFLICTING_EXTRA_ARGS"):
                f.enable()
            self.assertEqual(f.calls, [])

    def test_runtime_script_has_explicit_discovery_and_one_worker(self):
        with RuntimeScriptFixture() as f:
            result = f.run(USDB_NODE_ROLE="miner", USDB_MINER_ADDRESS=ADDRESS)
            self.assertEqual(result.returncode, 0, result.stderr)
            flags = MINING.flag_values(json.loads(f.argv_file.read_text()))
            self.assertEqual(flags["--bootnodes"], "")
            self.assertEqual(flags["--discovery.dns"], "")
            self.assertEqual(flags["--miner.threads"], "1")
            self.assertEqual(flags["--miner.etherbase"], ADDRESS)
            self.assertTrue(flags["--log.json"])

    def test_runtime_script_rejects_zero_workers_and_managed_flag_overrides(self):
        for env in ({"USDB_MINER_THREADS": "0"}, {"USDB_CHAIN_EXTRA_ARGS": "--bootnodes " + SEED},
                    {"USDB_CHAIN_EXTRA_ARGS": "--miner.threads 8"}, {"USDB_CHAIN_EXTRA_ARGS": "--fakepow"},
                    {"USDB_CHAIN_EXTRA_ARGS": "--override.terminaltotaldifficulty 0"}):
            with self.subTest(env=env), RuntimeScriptFixture() as f:
                result = f.run(USDB_NODE_ROLE="miner", USDB_MINER_ADDRESS=ADDRESS, **env)
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(f.argv_file.exists())

    def test_runtime_full_does_not_seal_and_persists_configured_seed(self):
        with RuntimeScriptFixture() as f:
            result = f.run(USDB_NODE_ROLE="full", USDB_BOOTNODES=SEED)
            self.assertEqual(result.returncode, 0, result.stderr)
            flags = MINING.flag_values(json.loads(f.argv_file.read_text()))
            self.assertNotIn("--mine", flags)
            self.assertEqual(flags["--bootnodes"], SEED)

    def test_local_seal_requires_full_hash_and_canonical_match(self):
        with MiningFixture() as f:
            full_hash = "0x" + "bb" * 32
            log = json.dumps({"msg": "Successfully sealed new block", "hash": full_hash})
            with mock.patch.object(MINING.subprocess, "run", return_value=mock.Mock(stdout=log, stderr="")), mock.patch.object(
                MINING, "rpc", side_effect=[{"number": "0x1", "hash": full_hash}, {"hash": full_hash}]
            ):
                self.assertTrue(MINING.local_seal(f.layout, f.runtime)["canonical"])
            with mock.patch.object(MINING.subprocess, "run", return_value=mock.Mock(stdout=log, stderr="")), mock.patch.object(
                MINING, "rpc", side_effect=[{"number": "0x1", "hash": full_hash}, {"hash": "0x" + "aa" * 32}]
            ):
                self.assertFalse(MINING.local_seal(f.layout, f.runtime)["canonical"])

    def test_actual_chain_identity_batch_rejects_wrong_genesis(self):
        with MiningFixture() as f:
            # Call the real boundary rather than the fixture's convenient chain view.
            import importlib.util
            spec = importlib.util.spec_from_file_location("mining_boundary", ROOT / "docker/scripts/tools/usdb_mining.py")
            module = importlib.util.module_from_spec(spec)
            spec.loader.exec_module(module)
            values = {"eth_chainId": "0x7b", "net_version": "123", "eth_getBlockByNumber": {"hash": "wrong"},
                      "eth_blockNumber": "0x0", "eth_syncing": False, "net_peerCount": "0x0", "admin_nodeInfo": {"id": "ee" * 32}}
            with mock.patch.object(NODE, "_json_rpc_batch", return_value=values), self.assertRaisesRegex(ValueError, "CHAIN_IDENTITY_MISMATCH"):
                module.chain_view(f.layout)

    def test_compatible_image_upgrade_preserves_applied_authorization(self):
        with MiningFixture() as f:
            f.enable()
            f.run()
            f.update_env(USDB_CHAIN_IMAGE="new-image-reference", USDB_CHAIN_MEMORY_LIMIT="6442450944")
            MINING.validate_start(f.layout)

    def test_node_up_reconciles_role_drift_instead_of_ready_noop(self):
        with MiningFixture() as f:
            f.update_env(USDB_NODE_ROLE="bootnode")
            reports = [{"overall_state": "STARTING", "checks": {"mining": {"drift": True}}},
                       {"overall_state": "STARTING", "checks": {"mining": {"drift": True}}},
                       {"overall_state": "READY", "checks": {}}]
            with mock.patch.object(NODE, "collect_node_status", side_effect=reports), mock.patch.object(NODE, "start_node") as start:
                result, code = NODE.up_node(f.layout, dry_run=False, allow_activation=False, sync_timeout_secs=1,
                                            pull=False, json_output=False)
            self.assertEqual(code, 0)
            self.assertEqual(result["completed_actions"], ["mining"])
            start.assert_not_called()
            self.assertEqual(f.container_number, 1)

    def test_data_change_during_stop_cannot_trigger_rollback_into_replacement(self):
        with MiningFixture() as f:
            f.enable()
            def replace(action):
                if action == "stop-chain":
                    Path(f.env["USDB_CHAIN_DATA_HOST_DIR"], "geth/nodekey").write_text("replacement")
            f.after_helper = replace
            self.assertEqual(f.run(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.container_number, 0)
            self.assertEqual(MINING.read_state(f.layout)["rollback"], "refused_identity_or_config_change")



    def test_real_compose_helper_stops_old_writer_before_chain_only_recreate(self):
        with RuntimeHelperFixture() as f:
            result = f.run("recreate-chain")
            self.assertEqual(result.returncode, 0, result.stderr)
            commands = [json.loads(line) for line in f.calls.read_text().splitlines()]
            up = next(i for i, cmd in enumerate(commands) if cmd[0] == "compose" and "up" in cmd)
            stop = next(i for i, cmd in enumerate(commands) if cmd[0] == "kill")
            self.assertLess(stop, up)
            self.assertEqual(f.state.read_text(), "running")
            for name in ("balance-history", "usdb-indexer", "btc-node", "script-registry-installer", "usdb-control-plane"):
                self.assertFalse(any(name in cmd for cmd in commands))

    def test_disable_recovers_legacy_zero_worker_configuration(self):
        with MiningFixture() as f:
            f.update_env(USDB_NODE_ROLE="miner", USDB_MINER_ADDRESS=ADDRESS, USDB_MINER_THREADS="0")
            f.adopt()
            MINING.submit(f.layout, disable=True, yes=True)
            self.assertEqual(f.run(), 0)
            self.assertEqual(MINING.observe(f.layout)["state"], "DISABLED")

    def test_expect_pass_is_rechecked_after_queue_and_remint_auto_selection_survives(self):
        with MiningFixture() as f:
            MINING.submit(f.layout, address=ADDRESS, first_node=True, yes=True, expect_pass=PASS_ID)
            f.candidate["pass"]["pass_id"] = "aa" * 32 + "i0"
            self.assertEqual(f.run(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.calls, [])
            MINING.submit(f.layout, address=ADDRESS, first_node=True, yes=True)
            self.assertEqual(f.run(), 0)
            self.assertEqual(MINING.read_state(f.layout)["plan"]["candidate"]["pass"]["pass_id"], "aa" * 32 + "i0")

    def test_optional_bootstrap_observation_never_blocks_eligibility(self):
        with MiningFixture() as f:
            plan = MINING.preflight(f.layout, ADDRESS, first_node=True)
            self.assertEqual(plan["state"], "READY")
            self.assertIsNone(plan["bootstrap"]["bootstrap_finalized"])
            self.assertIn("observation_error", plan["bootstrap"])

    def test_first_block_estimate_counts_genesis_allocation(self):
        with MiningFixture() as f:
            artifacts = f.layout.bundle_dir / "artifacts"
            artifacts.mkdir(parents=True)
            (artifacts / "usdb-genesis.json").write_text(json.dumps({"config": {"usdb": {"activations": [
                {"versions": {"pricePolicyVersion": 1, "coinbaseEmissionPolicyVersion": 1}}]}}}))
            with mock.patch.object(MINING, "rpc", return_value="0x" + format(10 * 10**18, "064x")):
                result = MINING.economics_status(f.layout, f.chain, f.candidate)
            self.assertEqual(result["issued_usdb_atoms"], "10000000000000000000")
            self.assertEqual(result["first_block_emission_atoms"], "56405377980720")



    def test_progress_surfaces_mining_failure_even_while_chain_rpc_waits(self):
        from common.node_progress import progress_fixture
        services = {name: {"state": "running", "exit_code": 0} for name in
                    ("btc-node", "balance-history", "usdb-indexer")}
        with progress_fixture(services) as layout:
            layout.node_env.write_text("USDB_RESOURCE_MODE=manual\n")
            (layout.node_env.parent / "node.mining.json").write_text("{}")
            with mock.patch.object(NODE, "_mining_status", return_value={"state": "FAILED", "detail": "OCI failure"}):
                report = NODE.collect_node_progress(layout)
            self.assertEqual(report["overall_state"], "FAILED")
            self.assertIn("OCI failure", NODE.render_node_progress(report))



    def test_repeat_enable_does_not_ignore_runtime_coinbase_drift(self):
        with MiningFixture() as f:
            f.enable()
            f.run()
            original = f.rpc
            def changed(layout, method, *args, **kwargs):
                return "0x" + "aa" * 20 if method == "eth_coinbase" else original(layout, method, *args, **kwargs)
            with mock.patch.object(MINING, "rpc", side_effect=changed):
                result = MINING.submit(f.layout, address=ADDRESS, yes=True)
            self.assertEqual(result["outcome"], "controller_submitted")
            self.assertEqual(MINING.read_state(f.layout)["phase"], "QUEUED")


if __name__ == "__main__":
    unittest.main()
