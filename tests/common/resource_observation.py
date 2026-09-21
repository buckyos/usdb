"""Sanitized resource samples for console browser coverage."""
import time


def resource_observation():
    now = int(time.time() * 1000)
    gib = 1024**3
    return dict(schema_version='usdb-console-resources:v1', observed_at_ms=now,
        host=dict(status='available', observed_at_ms=now, cpu_percent=23.5, cpu_count=16, cpu_interval_ms=10000,
                  memory_used_bytes=24 * gib, memory_available_bytes=40 * gib, memory_total_bytes=64 * gib,
                  swap_total_bytes=8 * gib, swap_used_bytes=0),
        containers=dict(status='available', observed_at_ms=now, items=[
            dict(service='btc-node', state='running', status='available', cpu_percent=210.5, memory_used_bytes=8 * gib, memory_limit_bytes=32 * gib),
            dict(service='ord-server', state='exited', status='not_running')]),
        filesystems=[dict(id='fixture-device', mount_path='/data', status='available', observed_at_ms=now,
                         total_bytes=4 * 1024 * gib, used_bytes=4 * 1024 * gib - 40 * gib, available_bytes=40 * gib, warning='critical')],
        directories=[dict(service='bitcoin', path='/data/.usdb/datasets/bitcoin/btc-mainnet', status='available', used_bytes=1500 * gib,
                          observed_at_ms=now, filesystem_id='fixture-device'),
                     dict(service='balance-history', path='/data/.usdb/datasets/balance-history/btc-mainnet/' + 'a' * 64,
                          status='timeout', used_bytes=400 * gib, observed_at_ms=now - 700000, checked_at_ms=now, filesystem_id='fixture-device')],
        disk_scan_interval_secs=300, disk_scan_in_progress=False)
