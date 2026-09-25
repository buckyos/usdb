"""Isolated monitor evidence, database and CLI fixtures; no live node access."""
from datetime import datetime, timezone
from pathlib import Path
from types import SimpleNamespace
import tempfile

import node_monitor as monitor
import node_monitor_rules as rules
import node_observation

BASE = 1800000000000


def report(at=BASE, *, ready=True, incidents=None, available=True, phase="READY", current=10, target=10, restarts=0):
    timestamp = datetime.fromtimestamp(at / 1000, timezone.utc).isoformat()
    services = {service: dict(probe_status="available", readiness=node_observation.readiness(dict(
        rpc_alive=True, query_ready=True, consensus_ready=ready, blockers=[] if ready else ["CatchingUp"],
        current=current, total=target), observed_at=timestamp), runtime=dict(state="running", health="healthy",
        details_available=True, container_id="a" * 64, restart_count=restarts, oom_killed=False))
        for service in node_observation.SERVICES}
    return dict(schema_version="usdb-console-monitor:v1", observation_available=available, observed_at_ms=at,
                overall_state=phase, components=[dict(id=k, state=phase) for k in services],
                observations=dict(schema_version=node_observation.SCHEMA, status="available", observed_at=timestamp,
                                  services=services, incidents=incidents or dict(status="available", events=[])))


def incident(identity="b" * 32):
    return dict(status="available", events=[dict(code="DEEP_REORG_HALTED", event_id=identity,
                latched=True, evidence_status="available", detected_at="2026-09-24T00:00:00+00:00")])


class MonitorFixture:
    def __enter__(self):
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.layout = SimpleNamespace(node_env=self.root / "node.env", release_id="r1", bundle_id="test",
                                      network_identity={"chain_id": 12, "genesis_block_hash": "a" * 64}, kit_root=self.root)
        self.layout.node_env.write_text("USDB_MONITOR_ENABLED=1\n")
        self.layout.node_env.chmod(0o600)
        self.settings = {**rules.DEFAULTS, "startup_grace_secs": 0, "warning_after_secs": 2,
                         "critical_after_secs": 4, "recovery_after_secs": 2, "stall_after_secs": 3}
        self.store = monitor.database(self.layout, writable=True)
        self.store.begin(BASE, "r1")
        return self

    def tick(self, seconds, **options):
        at = BASE + int(seconds * 1000)
        value = report(at, **options)
        rules.evaluate(self.store, value, at, self.settings)
        return value

    def codes(self):
        return [v["code"] for v in self.store.events(limit=1000)]

    def __exit__(self, *_):
        self.store.close()
        self.temp.cleanup()
