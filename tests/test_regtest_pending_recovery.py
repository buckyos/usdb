#!/usr/bin/env python3
"""Check recovery handshakes and evidence collection without starting services."""

import os
from pathlib import Path
import shlex
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
SCRIPTS = ROOT / 'src/btc/usdb-indexer/scripts'


class PendingRecoveryHarnessTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='usdb-recovery-harness-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.env = dict(os.environ, WORK_DIR=str(self.root), REPO_ROOT=str(ROOT),
                        USDB_INDEXER_ROOT=str(self.root / 'indexer'))

    def run_shell(self, body, library='regtest_reorg_lib.sh'):
        return subprocess.run(
            ['bash', '-c', f'set -euo pipefail\nsource {shlex.quote(str(SCRIPTS / library))}\n{body}'],
            env=self.env, text=True, capture_output=True, timeout=10,
        )

    def test_release_is_explicit_and_rearming_never_reuses_a_released_gate(self):
        result = self.run_shell('''
for phase in energy transfer-reload; do
  regtest_arm_recovery_fault "$phase"
  old_gate="$REGTEST_RECOVERY_RELEASE_FILE"
  [[ ! -e "$old_gate" ]]
  regtest_release_recovery_fault
  [[ -f "$old_gate" ]]
  regtest_arm_recovery_fault "$phase"
  [[ "$old_gate" != "$REGTEST_RECOVERY_RELEASE_FILE" ]]
  [[ ! -e "$REGTEST_RECOVERY_RELEASE_FILE" ]]
  [[ -f "$old_gate" ]]
done
''')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def test_indexer_child_receives_gate_and_zero_finite_failure_budget(self):
        fake_bin = self.root / 'bin'
        fake_bin.mkdir()
        cargo = fake_bin / 'cargo'
        cargo.write_text('''#!/usr/bin/env bash
printf 'energy=%s budget=%s\\n' "$USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_RELEASE_FILE" "$USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES"
printf 'transfer=%s budget=%s\\n' "$USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_RELEASE_FILE" "$USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES"
''')
        cargo.chmod(0o755)
        self.env['PATH'] = str(fake_bin) + os.pathsep + self.env['PATH']
        result = self.run_shell('''
regtest_arm_recovery_fault energy
regtest_arm_recovery_fault transfer-reload
regtest_start_usdb_indexer
wait "$USDB_INDEXER_PID"
cat "$USDB_INDEXER_LOG_FILE"
''')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        for phase in ('energy', 'transfer'):
            self.assertRegex(result.stdout, rf'{phase}={self.root}/recovery-\S+/release budget=0')

    def test_failure_diagnostics_include_indexer_runtime_log(self):
        logs = self.root / 'indexer/logs'
        logs.mkdir(parents=True)
        (logs / 'usdb-indexer_rCURRENT.log').write_text('durable-recovery-evidence\n')
        result = self.run_shell('regtest_print_failure_diagnostics 7')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn('usdb-indexer service log', result.stdout)
        self.assertIn('durable-recovery-evidence', result.stdout)

    def test_suite_allocates_distinct_case_directories_under_artifact_root(self):
        self.env['REGTEST_WORK_ROOT'] = str(self.root / 'artifact work')
        result = self.run_shell('''
run_cmd() { printf '%s\\n' "$@"; }
run_case 20 regtest_pending_recovery_energy_failure.sh
run_case 21 regtest_pending_recovery_transfer_reload_restart.sh
run_case 20 regtest_pending_recovery_energy_failure.sh
''', library='run_reorg_regression.sh')
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        paths = [Path(line.removeprefix('WORK_DIR=')) for line in result.stdout.splitlines()
                 if line.startswith('WORK_DIR=')]
        self.assertEqual(len(paths), 3, result.stdout)
        self.assertEqual(len(set(paths)), 3)
        for path in paths:
            self.assertEqual(path.parent, Path(self.env['REGTEST_WORK_ROOT']))
            self.assertTrue(path.is_dir())


if __name__ == '__main__':
    unittest.main()
