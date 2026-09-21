"""Private host observations: bounded scans, truthful failures, and resource semantics."""
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'docker/scripts/tools'))
import control_plane_resources as resources


class ResourceTests(unittest.TestCase):
    def test_cpu_counter_delta_excludes_guest_double_count_and_rejects_reset(self):
        with tempfile.TemporaryDirectory() as directory:
            proc = Path(directory)
            (proc / 'stat').write_text('cpu 10 20 30 100 40 5 5 10 999 999\n')
            self.assertEqual(resources.cpu_counters(proc), (220, 140))
        self.assertEqual(resources.cpu_percent((100, 50), (200, 75)), 75)
        self.assertIsNone(resources.cpu_percent((200, 100), (100, 20)))
        self.assertIsNone(resources.cpu_percent((100, 50), (100, 50)))

    def test_memory_uses_memavailable_instead_of_counting_cache_as_used(self):
        with tempfile.TemporaryDirectory() as directory:
            proc = Path(directory)
            (proc / 'meminfo').write_text('MemTotal: 1000 kB\nMemFree: 100 kB\nMemAvailable: 600 kB\nSwapTotal: 100 kB\nSwapFree: 40 kB\n')
            value = resources.host_memory(proc)
            self.assertEqual(value['memory_used_bytes'], 400 * 1024)
            self.assertEqual(value['swap_used_bytes'], 60 * 1024)
            self.assertEqual(value['memory_used_percent'], 40)

    def test_docker_units_and_invalid_numbers(self):
        self.assertEqual(resources.parse_size('1.5GiB'), int(1.5 * 1024**3))
        self.assertEqual(resources.parse_size('100MB'), 100_000_000)
        self.assertEqual(resources.parse_size('0B'), 0)
        for value in ('NaN%', '-1%', 'inf%'):
            with self.assertRaises(ValueError):
                resources.percent(value)
        with self.assertRaises(ValueError):
            resources.parse_size('unavailable')

    def test_container_inventory_is_scoped_and_stopped_is_not_zero_usage(self):
        outputs = ['a' * 12 + '\tusdb-chain\trunning\n' + 'b' * 12 + '\tord-server\texited\n' + 'd' * 12 + '\tunrelated\trunning\n',
                   'c' * 12 + '\tbtc-node\trunning\n',
                   '\n'.join(json.dumps(dict(ID=identifier * 12, CPUPerc='175.25%', MemUsage='1.5GiB / 4GiB', Password='SECRET')) for identifier in ('a', 'c'))]
        with mock.patch.object(resources, 'command', side_effect=outputs) as command:
            result = resources.container_stats('fixture')
        self.assertIn('label=com.docker.compose.project=fixture', command.call_args_list[0].args[0])
        self.assertIn('label=com.docker.compose.project=fixture-bitcoin', command.call_args_list[1].args[0])
        self.assertEqual(command.call_args_list[2].args[0][-2:], ['a' * 12, 'c' * 12])
        stopped = next(item for item in result['items'] if item['service'] == 'ord-server')
        self.assertEqual(stopped['status'], 'not_running')
        self.assertNotIn('memory_used_bytes', stopped)
        self.assertNotIn('SECRET', json.dumps(result))
        self.assertEqual(result['items'][0]['cpu_percent'], 175.25)

    def test_data_path_projection_and_disabled_ord(self):
        env = dict(BTC_NODE_DATA_HOST_DIR='/tmp/fixture-btc', USDB_CHAIN_DATA_HOST_DIR='/',
                   BH_DATA_HOST_DIR='relative', ORD_DATA_HOST_DIR='/tmp/fixture-ord', BTC_RPC_PASSWORD='SECRET')
        value = resources.directory_paths(env)
        self.assertEqual({item['service'] for item in value}, {'bitcoin', 'usdb-chain', 'balance-history'})
        self.assertEqual(value[1]['status'], 'invalid_path')
        self.assertNotIn('SECRET', json.dumps(value))
        self.assertEqual(len(resources.directory_paths(dict(env, USDB_MINTING_ENABLED='1'))), 4)

    def test_disk_scan_uses_allocated_blocks_and_does_not_follow_nested_symlinks(self):
        with tempfile.TemporaryDirectory() as directory, tempfile.TemporaryDirectory() as outside:
            root = Path(directory)
            with (root / 'sparse').open('wb') as stream:
                stream.truncate(1024**3)
            (root / 'link').symlink_to(outside, target_is_directory=True)
            (Path(outside) / 'payload').write_bytes(b'x' * 1024**2)
            value = resources.directory_size(str(root))
            self.assertEqual(value['status'], 'available')
            self.assertLess(value['used_bytes'], 1024**2)

    def test_partial_or_timed_out_walk_is_not_published_as_complete(self):
        with tempfile.TemporaryDirectory() as directory:
            for error, expected in [(subprocess.TimeoutExpired('du', 4), 'timeout'),
                                    (subprocess.CalledProcessError(1, 'du', '1024\tpath', 'SECRET'), 'unavailable')]:
                with mock.patch.object(resources, 'command', side_effect=error):
                    result = resources.directory_size(directory)
                self.assertEqual(result['status'], expected)
                self.assertNotIn('used_bytes', result)
                self.assertNotIn('SECRET', json.dumps(result))

    def test_filesystem_warning_uses_percent_and_absolute_capacity(self):
        self.assertEqual(resources.capacity_warning(2000 * resources.GIB, 1500 * resources.GIB), 'ok')
        self.assertEqual(resources.capacity_warning(2000 * resources.GIB, 150 * resources.GIB), 'warning')
        self.assertEqual(resources.capacity_warning(2000 * resources.GIB, 90 * resources.GIB), 'critical')
        self.assertEqual(resources.capacity_warning(200 * resources.GIB, 45 * resources.GIB), 'critical')

    def test_protected_directory_fallback_verifies_project_service_and_exact_mount(self):
        mount = dict(Type='bind', Source='/fixture/chain', Destination='/data/usdb-chain')
        with mock.patch.object(resources, 'command', side_effect=['a' * 12, json.dumps([mount]), '123\t/data/usdb-chain\n']) as command:
            result = resources.container_directory_size('/fixture/chain', 'usdb-chain', 'fixture')
        self.assertEqual(result['used_bytes'], 123 * 1024)
        self.assertIn('label=com.docker.compose.project=fixture', command.call_args_list[0].args[0])
        self.assertIn('label=com.docker.compose.service=usdb-chain', command.call_args_list[0].args[0])
        self.assertEqual(command.call_args_list[-1].args[0], ['docker', 'exec', 'a' * 12, 'timeout', '4', 'du', '-s', '-k', '-x', '--', '/data/usdb-chain'])
        mount['Source'] = '/other-node/chain'
        with mock.patch.object(resources, 'command', side_effect=['a' * 12, json.dumps([mount])]) as command:
            result = resources.container_directory_size('/fixture/chain', 'usdb-chain', 'fixture')
        self.assertEqual(result['status'], 'permission_denied')
        self.assertEqual(command.call_count, 2)

    def test_container_timeout_keeps_last_measurement_and_does_not_report_zero(self):
        collector = resources.ResourceCollector()
        collector.disk_cache['/fixture/chain'] = dict(used_bytes=123, observed_at_ms=10)
        with mock.patch.object(resources, 'directory_size', return_value=dict(status='permission_denied')), \
                mock.patch.object(resources, 'container_directory_size', return_value=dict(status='timeout', checked_at_ms=20)):
            collector._scan(['/fixture/chain'], 'fixture', {'/fixture/chain': 'usdb-chain'})
        self.assertEqual(collector.disk_cache['/fixture/chain'], dict(status='timeout', checked_at_ms=20, used_bytes=123, observed_at_ms=10))

    def test_background_scan_does_not_block_heartbeat_or_run_twice_per_interval(self):
        collector = resources.ResourceCollector()
        started, release = threading.Event(), threading.Event()
        def scan(_path):
            started.set()
            release.wait(3)
            return dict(status='available', used_bytes=100, observed_at_ms=resources.now_ms())
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(resources, 'container_stats', return_value=dict(status='available', items=[])), \
                mock.patch.object(resources, 'directory_size', side_effect=scan) as walk:
            env = dict(BTC_NODE_DATA_HOST_DIR=directory, BH_DATA_HOST_DIR=directory)
            try:
                first = collector.sample(env, 'fixture')
                self.assertTrue(started.wait(1))
                self.assertTrue(first['disk_scan_in_progress'])
                self.assertEqual(first['directories'][0]['status'], 'pending')
                self.assertEqual(len(first['filesystems']), 1)
                second = collector.sample(env, 'fixture')
                self.assertTrue(second['disk_scan_in_progress'])
                self.assertEqual(walk.call_count, 1)
            finally:
                release.set()
                collector.worker.join(3)
            final = collector.sample(env, 'fixture')
            self.assertEqual(final['directories'][0]['used_bytes'], 100)
            self.assertEqual(walk.call_count, 1)

    def test_failed_rescan_preserves_last_success_timestamp(self):
        collector = resources.ResourceCollector()
        collector.disk_cache['/fixture'] = dict(status='available', used_bytes=100, observed_at_ms=123)
        with mock.patch.object(resources, 'directory_size', return_value=dict(status='timeout', checked_at_ms=456)):
            collector._scan(['/fixture'])
        self.assertEqual(collector.disk_cache['/fixture'], dict(status='timeout', checked_at_ms=456, used_bytes=100, observed_at_ms=123))

    def test_docker_failure_does_not_erase_host_or_disk_sample(self):
        with tempfile.TemporaryDirectory() as directory, \
                mock.patch.object(resources, 'container_stats', side_effect=ValueError('SECRET')):
            result = resources.ResourceCollector().sample(dict(BTC_NODE_DATA_HOST_DIR=directory), 'fixture', wait_for_disk=True)
        self.assertEqual(result['containers']['status'], 'unavailable')
        self.assertEqual(result['host']['status'], 'available')
        self.assertEqual(result['directories'][0]['status'], 'available')
        self.assertEqual(len(result['filesystems']), 1)
        self.assertNotIn('SECRET', json.dumps(result))


if __name__ == '__main__':
    unittest.main()
