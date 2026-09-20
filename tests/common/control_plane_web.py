"""Isolated control-plane browser fixture; no node data or external RPC access."""
from contextlib import contextmanager
import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[2]


def default_report():
    return dict(schema_version="usdb-console-monitor:v1", observed_at_ms=int(time.time() * 1000),
                  observation_available=True, overall_state="SYNCING", node_role="full", release_id="fixture-release",
                  network=dict(name="fixture-network", chain_id=123),
                  components=[dict(id="bitcoin", state="SYNCING", current=935100, total=960000,
                                   progress_phase="foreground", file_preparation=dict(state="VERIFIED"),
                                   background_validation=dict(height=100, target=935000, validated=False, available=True))])


@contextmanager
def console_server(report=None):
    """Yield the private origin, ephemeral token and observer file of a disposable server."""
    with tempfile.TemporaryDirectory(prefix="usdb-private-console-browser-") as temporary:
        root = Path(temporary)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        roots = [ROOT / "web" / name / "dist" for name in ("usdb-console-app", "balance-history-browser", "usdb-indexer-browser")]
        assert all(path.is_dir() for path in roots), "Build all console web apps first"
        config = f'''root_dir = {json.dumps(str(root))}
[server]
host = "127.0.0.1"
port = {port}
[bitcoin]
url = "http://127.0.0.1:1"
[rpc]
balance_history_url = "http://127.0.0.1:1"
usdb_indexer_url = "http://127.0.0.1:1"
usdb_chain_url = "http://127.0.0.1:1"
ord_url = "http://127.0.0.1:1"
[web]
console_root = {json.dumps(str(roots[0]))}
balance_history_explorer_root = {json.dumps(str(roots[1]))}
usdb_indexer_explorer_root = {json.dumps(str(roots[2]))}
'''
        (root / "config.toml").write_text(config)
        report = report or default_report()
        snapshot = root / "node-progress.json"
        snapshot.write_text(json.dumps(report))
        server = subprocess.Popen([str(ROOT / "src/btc/target/debug/usdb-control-plane"), "--root-dir", str(root), "--skip-process-lock"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            origin = f"http://127.0.0.1:{port}"
            for _ in range(100):
                if server.poll() is not None:
                    raise AssertionError(server.stderr.read().decode())
                try:
                    with urlopen(origin + "/healthz", timeout=1) as response:
                        if response.status == 200:
                            break
                except OSError:
                    time.sleep(0.1)
            else:
                raise AssertionError("Console did not start")
            token = (root / "access-token").read_text().strip()
            yield origin, token, snapshot, report
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)

