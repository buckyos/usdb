#!/usr/bin/env python3
"""Run the shell shutdown path with a stateful Docker substitute and no live data."""

import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


RUNNER = Path(__file__).with_name("run_testnet_runtime.sh")
DOCKER = '''#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
state_path = Path(os.environ['RESOURCE_TEST_STATE'])
states = json.loads(state_path.read_text())
args = sys.argv[1:]
with open(os.environ['RESOURCE_TEST_CALLS'], 'a') as log:
    log.write(json.dumps(args) + '\\n')
if args[0] == 'compose':
    if 'exec' in args:
        assert 'btc-node' in args
    elif 'ps' in args:
        service = args[-1]
        if service in states:
            print(service)
    elif 'up' in args:
        assert '--no-deps' in args
        services = args[args.index('--no-deps') + 1:]
        assert set(services) <= {'usdb-chain-init', 'paired-checkpoint-recovery', 'usdb-chain', 'usdb-control-plane'}
        if 'usdb-chain-init' in services:
            assert states.get('usdb-chain', {}).get('state') != 'running'
        if 'usdb-chain' in services:
            for job in ('usdb-chain-init', 'paired-checkpoint-recovery'):
                assert states[job]['state'] == 'exited' and states[job]['exit_code'] == 0
        for service in services:
            states[service] = {'state': 'running'}
    else:
        raise SystemExit('unexpected Compose mutation')
elif args[0] == 'inspect':
    service = args[-1]
    if states[service].get('flush_checks', 0):
        states[service]['flush_checks'] -= 1
        if states[service]['flush_checks'] == 0:
            states[service]['state'] = 'exited'
    print(states[service]['state'])
elif args[0] == 'update':
    if os.environ.get('RESOURCE_TEST_FAIL_UPDATE') == '1':
        raise SystemExit(1)
    states[args[-1]]['restart'] = args[1].split('=', 1)[1]
elif args[0] == 'kill':
    assert args[1] == '--signal=SIGTERM', args
    assert states[args[-1]].get('restart') == 'no'
    states[args[-1]]['flush_checks'] = 2
elif args[0] == 'wait':
    code = int(os.environ.get('RESOURCE_TEST_FAIL_JOB') == args[-1])
    states[args[-1]].update(state='exited', exit_code=code)
    print(code)
else:
    raise SystemExit('unexpected Docker command')
state_path.write_text(json.dumps(states))
'''


class ResourceRunnerTests(unittest.TestCase):
    def run_quiesce(self, states, fail_update=False, action="quiesce-data", fail_job=""):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            (root / "node.env").write_text("USDB_RESOURCE_MODE=auto\n")
            state_path = root / "states.json"
            state_path.write_text(json.dumps(states))
            calls = root / "calls.jsonl"
            executable = root / "docker"
            executable.write_text(DOCKER.replace("#!/usr/bin/env python3", f"#!{sys.executable}", 1))
            executable.chmod(0o755)
            sleep = root / "sleep"
            sleep.write_text("#!/bin/sh\nexit 0\n")
            sleep.chmod(0o755)
            # Readiness/validation responses are fixtures; the shell orchestration
            # and Docker sequencing still execute through the real runner.
            python = root / "python3"
            python.write_text("#!/bin/sh\nexit 0\n")
            python.chmod(0o755)
            env = {**os.environ, "PATH": f"{root}:{os.environ['PATH']}",
                   "USDB_TESTNET_BUNDLE_DIR": str(root), "USDB_TESTNET_NODE_ENV": str(root / "node.env"),
                   "RESOURCE_TEST_STATE": str(state_path), "RESOURCE_TEST_CALLS": str(calls),
                   "RESOURCE_TEST_FAIL_UPDATE": "1" if fail_update else "0",
                   "RESOURCE_TEST_FAIL_JOB": fail_job}
            result = subprocess.run([str(RUNNER), action], env=env, capture_output=True, text=True, timeout=10)
            return result, json.loads(state_path.read_text()), [json.loads(line) for line in calls.read_text().splitlines()]

    def test_consumers_stop_before_sources_and_loader_is_preserved(self):
        names = ["balance-history", "usdb-indexer", "usdb-chain", "usdb-control-plane", "snapshot-loader"]
        result, states, calls = self.run_quiesce({name: {"state": "running"} for name in names})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual([call[-1] for call in calls if call[0] == "kill"],
                         ["usdb-control-plane", "usdb-chain", "usdb-indexer", "balance-history"])
        self.assertEqual(states["snapshot-loader"], {"state": "running"})
        for name in names[:-1]:
            self.assertEqual(states[name]["state"], "exited")
            self.assertEqual(states[name]["restart"], "no")
        self.assertFalse(any("SIGKILL" in " ".join(call) for call in calls))

    def test_disabled_restart_is_required_before_any_shutdown_signal(self):
        result, states, calls = self.run_quiesce({"balance-history": {"state": "running"}}, fail_update=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(states["balance-history"]["state"], "running")
        self.assertFalse(any(call[0] == "kill" for call in calls))

    def test_stopped_services_are_idempotent_and_paused_service_is_rejected(self):
        result, _, calls = self.run_quiesce({"balance-history": {"state": "exited"}})
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(any(call[0] in {"kill", "update"} for call in calls))
        result, states, calls = self.run_quiesce({"balance-history": {"state": "paused"}})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("paused", result.stderr)
        self.assertEqual(states["balance-history"]["state"], "paused")
        self.assertFalse(any(call[0] == "kill" for call in calls))

    def test_partial_chain_restart_releases_allocation_before_bootstrap_jobs(self):
        result, states, calls = self.run_quiesce(
            {"usdb-chain": {"state": "running"}, "balance-history": {"state": "running"},
             "snapshot-loader": {"state": "exited"}}, action="up-chain")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(states["balance-history"], {"state": "running"})
        self.assertEqual(states["snapshot-loader"], {"state": "exited"})
        self.assertEqual([call[-1] for call in calls if call[0] == "kill"], ["usdb-chain"])
        self.assertEqual([call[-1] for call in calls if call[0] == "wait"],
                         ["usdb-chain-init", "paired-checkpoint-recovery"])
        for name in ("usdb-chain", "usdb-control-plane"):
            self.assertEqual(states[name], {"state": "running", "restart": "unless-stopped"})

    def test_failed_checkpoint_gate_never_starts_chain(self):
        result, states, _ = self.run_quiesce({}, action="up-chain", fail_job="paired-checkpoint-recovery")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("paired-checkpoint-recovery", result.stderr)
        self.assertNotIn("usdb-chain", states)


if __name__ == "__main__":
    unittest.main()
