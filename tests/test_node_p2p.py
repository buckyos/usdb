#!/usr/bin/env python3
"""Accept family selection, container publication and recoverable P2P changes."""
from copy import deepcopy
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import unittest
from unittest import mock

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "docker/scripts/tools"))
import usdb_p2p as P2P
import usdb_node as NODE
import usdb_peers as PEERS
import usdb_mining as MINING
from common.p2p import P2PFixture, HOST, V4, V6, container
from common.enode import PUBLIC_KEY, V6 as SEED6
from common.mining import RuntimeHelperFixture


class P2PTests(unittest.TestCase):
    def test_ra_route_must_survive_docker_enabling_forwarding(self):
        with P2PFixture() as f:
            f.host["ipv6_ra_interfaces"] = [{"interface": "eth0", "accept_ra": "1"}]
            updates, reason = P2P.select("auto")
            self.assertEqual(updates["USDB_P2P_IP_FAMILY"], "ipv4")
            self.assertIn("P2P_IPV6_RA_REQUIRED", reason)
            with self.assertRaisesRegex(ValueError, "eth0.accept_ra=2"):
                f.configure("dual")
            self.assertEqual(f.calls, [])
            self.assertFalse(PEERS.state_path(f.layout).exists())
            f.host["ipv6_ra_interfaces"][0]["accept_ra"] = "2"
            self.assertEqual(P2P.select("auto")[0]["USDB_P2P_IP_FAMILY"], "dual")
            f.configure()
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)

    def test_engine_rejects_old_or_rootless_docker(self):
        for engine, compose, options, accepted in (
                ("28.0.0", "2.33.1", [], True),
                ("20.10.24+dfsg1", "2.33.1", [], False),
                ("28.0.0", "2.33.0", [], False),
                ("28.0.0", "2.33.1", ["name=rootless"], False)):
            with self.subTest(engine=engine, compose=compose, options=options), \
                    mock.patch.object(P2P, "command_json", side_effect=[
                        {"Version": engine}, {"OSType": "linux", "SecurityOptions": options}]), \
                    mock.patch.object(P2P.subprocess, "run", return_value=subprocess.CompletedProcess([], 0, compose)):
                if accepted:
                    self.assertEqual(P2P.engine_capabilities()["engine"], engine)
                else:
                    with self.assertRaisesRegex(ValueError, "P2P_IPV6_ENGINE_REQUIRED"):
                        P2P.engine_capabilities()

    def test_invalid_transport_is_rejected_without_host_io(self):
        with mock.patch.object(P2P, "command_json", side_effect=AssertionError("unexpected IO")):
            for env in ({"USDB_P2P_IP_FAMILY": "auto"}, {"USDB_P2P_IP_FAMILY": "ipv6"},
                        {"USDB_P2P_ADVERTISE_IPV4": "0.0.0.0"}, {"USDB_P2P_ADVERTISE_PORT": "65536"},
                        {"USDB_P2P_ADVERTISE_IPV6": V6}, {"USDB_P2P_ADVERTISE_IPV4": "host.example.org"}):
                with self.subTest(env=env), self.assertRaises(ValueError):
                    P2P.validate(env)

    def test_auto_freezes_dual_and_reports_ipv4_fallback(self):
        with P2PFixture() as f:
            updates, reason = P2P.select("auto")
            self.assertEqual(updates["USDB_P2P_IP_FAMILY"], "dual")
            self.assertEqual(updates["USDB_P2P_REQUESTED_FAMILY"], "auto")
            f.host["ipv6_default_route"] = False
            updates, reason = P2P.select("auto")
            self.assertEqual(updates["USDB_P2P_IP_FAMILY"], "ipv4")
            self.assertIn("route", reason)
            f.host["ipv6_default_route"] = True
            with mock.patch.object(P2P, "engine_capabilities", side_effect=ValueError("old Docker")):
                updates, reason = P2P.select("auto")
                self.assertEqual(updates["USDB_P2P_IP_FAMILY"], "ipv4")
                self.assertIn("old Docker", reason)
                with self.assertRaisesRegex(ValueError, "old Docker"):
                    P2P.select("ipv6")

    def test_explicit_ipv6_requires_assigned_address_and_route_before_mutation(self):
        with P2PFixture() as f:
            before = f.layout.node_env.read_bytes()
            for changes in ({"ipv6": []}, {"ipv6_default_route": False}):
                f.host = {**deepcopy(HOST), **changes}
                with self.assertRaisesRegex(ValueError, "P2P_IPV6_HOST_REQUIRED"):
                    f.configure("ipv6")
            self.assertFalse(PEERS.state_path(f.layout).exists())
            self.assertEqual(f.layout.node_env.read_bytes(), before)

    def test_host_inspection_excludes_unstable_and_docker_addresses(self):
        items = [{"ifname": "eth0", "addr_info": [
            {"family": "inet6", "scope": "global", "local": V6},
            *[{"family": "inet6", "scope": "global", "local": f"2001:4860::{index+2}", flag: True}
              for index, flag in enumerate(("temporary", "tentative", "deprecated", "dadfailed"))]]},
            {"ifname": "br-abc", "addr_info": [{"family": "inet6", "scope": "global", "local": "fd12::1"}]}]
        with mock.patch.object(P2P, "command_json", side_effect=[items, [{"dev": "eth0"}]]):
            report = P2P.host_capabilities()
        self.assertEqual(report["ipv6"], [V6])
        self.assertTrue(report["ipv6_default_route"])
        with mock.patch.object(P2P, "command_json", side_effect=[items, [{"dev": "eth0", "protocol": "ra"}]]), \
                mock.patch.object(Path, "read_text", return_value="1\n"):
            report = P2P.host_capabilities()
        self.assertEqual(report["ipv6_ra_interfaces"], [{"interface": "eth0", "accept_ra": "1"}])
        with mock.patch.object(P2P, "command_json", side_effect=[items, [{"dev": "eth0", "protocol": "ra"}]]), \
                mock.patch.object(Path, "read_text", side_effect=PermissionError("fixture unreadable sysctl")):
            report = P2P.host_capabilities()
        self.assertFalse(report["ipv6_default_route"])
        self.assertIn("unreadable sysctl", report["errors"][0])

    def test_ipv6_publication_does_not_accept_ipv4_proxy_or_missing_udp(self):
        env = {"USDB_P2P_IP_FAMILY": "dual"}
        self.assertTrue(P2P.transport_ready(env, container("dual")))
        self.assertFalse(P2P.transport_ready(env, container("ipv4")))
        for missing in ("udp", "route"):
            runtime = container("dual")
            if missing == "udp":
                runtime["ports"]["31303/udp"] = [{"HostIp": "0.0.0.0", "HostPort": "31303"}]
            else:
                runtime["networks"].pop("p2p")
            self.assertFalse(P2P.transport_ready(env, runtime))

    def test_configure_then_seed_add_preserves_queued_transport(self):
        with P2PFixture() as f:
            f.configure()
            f.edit(enode=SEED6)
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            env = NODE.read_env(f.layout.node_env)
            self.assertEqual(env["USDB_P2P_IP_FAMILY"], "dual")
            self.assertEqual(env["USDB_BOOTNODES"], SEED6)
            self.assertEqual(f.container_number, 1)
            self.assertEqual([args[0] for _, args in f.calls], ["stop-chain", "recreate-chain"])

    def test_pending_transport_does_not_share_old_address_as_applied(self):
        with P2PFixture() as f:
            f.configure("dual", advertise_ipv4=V4)
            report = P2P.endpoint_report(f.layout)
            self.assertEqual(report["state"], "WAITING")
            self.assertEqual(report["family"], "ipv4")
            self.assertEqual(report["desired_family"], "dual")
            self.assertEqual(report["endpoints"], [])
            f.run_peers()
            f.run_peers()
            self.assertEqual(P2P.endpoint_report(f.layout)["state"], "CONFIGURED")
            operation = PEERS.read_state(f.layout)
            operation["transport_updates"].pop("USDB_P2P_IP_FAMILY")
            PEERS.write_state(f.layout, operation)
            with self.assertRaisesRegex(ValueError, "PEER_JOURNAL_INVALID"):
                P2P.endpoint_report(f.layout)

    def test_switching_transport_rebinds_existing_miner_authorization(self):
        with P2PFixture() as f:
            f.enable()
            f.run()
            original = MINING.read_state(f.layout)
            f.configure()
            f.run_peers()
            self.assertEqual(f.run_peers(), 0)
            after = MINING.read_state(f.layout)
            self.assertEqual(after["first_node"], original["first_node"])
            self.assertEqual(after["authorization_config"]["USDB_P2P_IP_FAMILY"], "dual")
            MINING.validate_start(f.layout)
            f.update_env(USDB_P2P_IP_FAMILY="ipv6")
            with self.assertRaisesRegex(ValueError, "MINING_AUTHORIZATION_REQUIRED"):
                MINING.validate_start(f.layout)

    def test_disappearing_ipv6_can_be_corrected_without_losing_data_or_seed(self):
        with P2PFixture() as f:
            f.configure()
            # The initial file update succeeds, then the host route disappears.
            original = PEERS._phase
            def change_host(layout, operation, phase):
                original(layout, operation, phase)
                if phase == "STOPPING":
                    f.host["ipv6_default_route"] = False
            with mock.patch.object(PEERS, "_phase", side_effect=change_host):
                self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.calls, [])
            f.configure("ipv4")
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            self.assertEqual(f.runtime["environment"]["USDB_P2P_IP_FAMILY"], "ipv4")

    def test_managed_firewall_failure_preserves_running_chain_and_can_retry(self):
        with P2PFixture() as f, mock.patch.object(NODE, "run_firewall_action") as firewall:
            f.update_env(USDB_FIREWALL_MODE="managed")
            f.configure()
            firewall.side_effect = ValueError("missing IPv6 UFW allow rule")
            self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertEqual(f.calls, [])
            self.assertEqual(f.runtime["state"], "running")
            firewall.assert_called_once_with(f.layout, "check", output_to_stderr=True)
            firewall.side_effect = None
            PEERS.submit(f.layout, "apply")
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)

    def test_wrong_running_port_mapping_blocks_success_and_explicit_retry_recovers(self):
        with P2PFixture() as f:
            f.configure()
            self.assertEqual(f.run_peers(), 1)
            with mock.patch.object(P2P, "container_view", return_value=container("ipv4")):
                self.assertEqual(f.run_peers(), NODE.CONTROLLER_MANUAL_EXIT_CODE)
            self.assertIn("P2P_TRANSPORT_MISMATCH", PEERS.read_state(f.layout)["error"])
            self.assertEqual(f.container_number, 1)
            PEERS.submit(f.layout, "apply")
            self.assertEqual(f.run_peers(), 1)
            self.assertEqual(f.run_peers(), 0)
            self.assertEqual(f.container_number, 2)

    def test_shareable_candidates_keep_one_key_and_use_external_ports(self):
        with P2PFixture() as f:
            f.configure(advertise_ipv4=V4, advertise_port=41303, discovery_port=41304)
            f.run_peers()
            f.run_peers()
            report = P2P.endpoint_report(f.layout)
            self.assertEqual(report["state"], "CONFIGURED")
            self.assertEqual(report["reachability"], "unverified")
            self.assertEqual([entry["family"] for entry in report["endpoints"]], ["ipv4", "ipv6"])
            self.assertEqual([entry["enode"] for entry in report["endpoints"]], [
                f"enode://{PUBLIC_KEY}@{V4}:41303?discport=41304",
                f"enode://{PUBLIC_KEY}@[{V6}]:41303?discport=41304"])

    def test_private_container_ip_is_not_presented_as_public_enode(self):
        with P2PFixture() as f:
            report = P2P.endpoint_report(f.layout)
            self.assertEqual(report["endpoints"], [])
            f.chain["network"] = {}
            with mock.patch.object(MINING, "chain_view", side_effect=ValueError("CHAIN_IDENTITY_MISMATCH: wrong genesis")):
                self.assertEqual(P2P.endpoint_report(f.layout)["state"], "BLOCKED")

    def test_commands_execute_transport_selection_and_family_filter(self):
        with P2PFixture() as f, mock.patch("sys.stdout", new_callable=io.StringIO) as out:
            args = NODE.build_parser().parse_args(["peers", "configure", "--ip-family", "ipv6", "--json"])
            self.assertEqual(PEERS.execute(f.layout, args), 0)
            self.assertEqual(json.loads(out.getvalue())["outcome"], "controller_submitted")
            f.run_peers()
            f.run_peers()
            out.seek(0)
            out.truncate()
            args = NODE.build_parser().parse_args(["peers", "enode", "--family", "ipv6", "--json"])
            self.assertEqual(PEERS.execute(f.layout, args), 0)
            self.assertEqual(json.loads(out.getvalue())["endpoints"][0]["family"], "ipv6")
            out.seek(0)
            out.truncate()
            args = NODE.build_parser().parse_args(["peers", "enode", "--family", "ipv4", "--json"])
            self.assertEqual(PEERS.execute(f.layout, args), 1)
            self.assertEqual(json.loads(out.getvalue())["endpoints"], [])

    def test_runtime_helper_checks_transport_before_stopping_and_selects_overlays(self):
        for family in ("dual", "ipv6"):
            with self.subTest(family=family), RuntimeHelperFixture() as f:
                f.node_env.write_text(f"USDB_NODE_ROLE=full\nUSDB_P2P_IP_FAMILY={family}\n")
                check = f.script.parent / "usdb_p2p.py"
                check.write_text("raise SystemExit('P2P_IPV6_HOST_CHANGED: fixture missing route')\n")
                failed = f.run("recreate-chain")
                self.assertNotEqual(failed.returncode, 0)
                self.assertIn("P2P_IPV6_HOST_CHANGED", failed.stderr)
                self.assertFalse(f.calls.exists())
                self.assertEqual(f.state.read_text(), "running")
                check.write_text("# Restored host preflight boundary.\n")
                result = f.run("recreate-chain")
                self.assertEqual(result.returncode, 0, result.stderr)
                commands = [json.loads(line) for line in f.calls.read_text().splitlines()]
                for command in commands:
                    if command[0] != "compose":
                        continue
                    files = [Path(command[i+1]).name for i, arg in enumerate(command) if arg == "-f"]
                    self.assertEqual(files, ["compose.runtime.yml", "compose.network.yml", "compose.p2p-dual.yml"] +
                                     (["compose.p2p-ipv6.yml"] if family == "ipv6" else []))

    def test_compose_renders_both_protocols_without_expanding_rpc_exposure(self):
        # Real Compose parsing catches merge/override behavior that YAML string tests miss.
        environment = {**os.environ, "USDB_NETWORK_ARTIFACTS_DIR": "/tmp/usdb-p2p-test-artifacts",
                       "BH_SNAPSHOT_TRUST_HOST_DIR": "/tmp/usdb-p2p-test-trust",
                       "USDB_SERVICES_IMAGE": "fixture-services:test", "USDB_CHAIN_IMAGE": "fixture-chain:test"}
        for family in ("ipv4", "dual", "ipv6"):
            command = ["docker", "compose", "--env-file", str(ROOT / "docker/networks/testnet-v0/network.env"),
                       "--env-file", str(ROOT / "docker/networks/testnet-v0/node.env.example"),
                       "-f", str(ROOT / "docker/compose.runtime.yml"),
                       "-f", str(ROOT / "docker/networks/testnet-v0/compose.network.yml")]
            if family != "ipv4":
                command += ["-f", str(ROOT / "docker/compose.p2p-dual.yml")]
            if family == "ipv6":
                command += ["-f", str(ROOT / "docker/compose.p2p-ipv6.yml")]
            result = subprocess.run([*command, "config", "--format=json"], env=environment,
                                    capture_output=True, text=True, timeout=20)
            self.assertEqual(result.returncode, 0, result.stderr)
            config = json.loads(result.stdout)
            chain = config["services"]["usdb-chain"]
            expected = {"ipv4": {"0.0.0.0"}, "dual": {"0.0.0.0", "::"}, "ipv6": {"::"}}[family]
            for protocol in ("tcp", "udp"):
                self.assertEqual({port["host_ip"] for port in chain["ports"]
                                  if port["target"] == 31303 and port["protocol"] == protocol}, expected)
            for service in config["services"].values():
                for port in service.get("ports", []):
                    if port["target"] != 31303:
                        self.assertEqual(port["host_ip"], "127.0.0.1")
            self.assertEqual(set(config["services"]["usdb-indexer"]["networks"]), {"usdb-runtime"})
            if family != "ipv4":
                self.assertTrue(config["networks"]["usdb-p2p"]["enable_ipv6"])
                self.assertEqual(chain["networks"]["usdb-p2p"]["gw_priority"], 1)


if __name__ == "__main__":
    unittest.main()
