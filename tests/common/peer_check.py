"""Release-bound peer diagnostic fixtures with an isolated Docker runner."""
import hashlib
import json
from types import SimpleNamespace
from unittest import mock

import usdb_peer_check as CHECK
from common.peers import PeerFixture


class PeerCheckFixture(PeerFixture):
    def __enter__(self):
        super().__enter__()
        self.layout.bundle_dir.mkdir()
        raw = json.dumps({"config": {"chainId": 123}}).encode()
        (self.layout.bundle_dir / "genesis.json").write_bytes(raw)
        (self.layout.bundle_dir / "network.json").write_text(json.dumps({"artifacts": {"genesis": {"path": "genesis.json"}}}))
        self.layout.network_identity["genesis_sha256"] = hashlib.sha256(raw).hexdigest()
        self.probes = []
        self.mutate = lambda report: None
        self.runner = self.stack.enter_context(mock.patch.object(CHECK.subprocess, "run", side_effect=self.run_probe))
        return self

    def run_probe(self, args, **kwargs):
        self.probes.append((args, kwargs))
        request = json.loads(kwargs["input"])
        stage = lambda: {"state": "PASS"}
        report = {"schema_version": CHECK.SCHEMA, "enode": request["enode"], "checked_at": "2026-09-27T00:00:00Z",
                  "network_id": request["network_id"], "genesis_hash": request["genesis_hash"],
                  "state": "PASS", "usable": True, "syntax": stage(), "dns": stage(), "warnings": [],
                  "endpoints": [{"ip": "192.0.2.1", "family": "ipv4", "tcp_port": 31303, "udp_port": 31303,
                                 **{name: stage() for name in CHECK.STAGES}}]}
        self.mutate(report)
        return SimpleNamespace(returncode=0, stdout=json.dumps(report), stderr="")
