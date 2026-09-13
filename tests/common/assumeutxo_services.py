"""Isolated process, RPC and fault-injection helpers for service acceptance."""

import base64
from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import signal
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request


def free_port():
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class RpcError(RuntimeError):
    def __init__(self, error):
        self.error = error
        super().__init__(str(error))


class Rpc:
    def __init__(self, port, cookie=None):
        self.url = f"http://127.0.0.1:{port}"
        self.cookie = cookie

    def __call__(self, method, *params):
        headers = {"Content-Type": "application/json"}
        if self.cookie:
            headers["Authorization"] = "Basic " + base64.b64encode(self.cookie.read_bytes().strip()).decode()
        request = urllib.request.Request(self.url, json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)).encode(), headers)
        try:
            with urllib.request.urlopen(request, timeout=5) as response:
                result = json.load(response)
        except urllib.error.HTTPError as error:
            result = json.load(error)
        if result.get("error"):
            raise RpcError(result["error"])
        return result["result"]


class Processes:
    """Track only child processes created by this run; retain all logs and data."""

    def __init__(self, root):
        self.root = root
        self.children = {}
        self.peak_rss_kib = {}

    def start(self, name, args):
        assert name not in self.children or self.children[name].poll() is not None
        with (self.root / f"{name}.log").open("ab") as log:
            process = subprocess.Popen([str(arg) for arg in args], stdout=log, stderr=subprocess.STDOUT)
        self.children[name] = process
        return process

    def sample(self):
        for name, process in self.children.items():
            if process.poll() is not None:
                raise RuntimeError(f"{name} exited with {process.returncode}; inspect {self.root}")
            try:
                for line in Path(f"/proc/{process.pid}/status").read_text().splitlines():
                    if line.startswith("VmHWM:"):
                        self.peak_rss_kib[name] = max(self.peak_rss_kib.get(name, 0), int(line.split()[1]))
            except FileNotFoundError:
                pass

    def wait(self, predicate, label, timeout=90):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            self.sample()
            try:
                value = predicate()
                if value:
                    return value
                last = value
            except (OSError, RuntimeError, ValueError) as error:
                last = str(error)
            time.sleep(0.1)
        raise RuntimeError(f"Timed out waiting for {label}: {last}; inspect {self.root}")

    def stop(self, name, crash=False):
        process = self.children.pop(name, None)
        if process is None or process.poll() is not None:
            return
        process.send_signal(signal.SIGKILL if crash else signal.SIGTERM)
        try:
            process.wait(timeout=30)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            raise RuntimeError(f"{name} graceful shutdown timed out")
        if not crash and process.returncode != 0:
            raise RuntimeError(f"{name} shutdown returned {process.returncode}")

    def close(self):
        errors = []
        for name in list(self.children)[::-1]:
            try:
                self.stop(name)
            except RuntimeError as error:
                errors.append(str(error))
        if errors:
            raise RuntimeError(errors)


class CoreProxy:
    """Inject missing block/undo responses for one indexer without mutating Core data."""

    def __init__(self, upstream):
        self.upstream = upstream
        self.missing_undo = None
        self.missing_block = None
        self.calls = Counter()
        self.faults = Counter()
        self.lock = threading.Lock()
        proxy = self

        class Handler(BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def log_message(self, *_args):
                pass

            def do_POST(self):
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                method, params = request["method"], request.get("params", [])
                key = method + (f":{params[1]}" if method == "getblock" and len(params) > 1 else "")
                with proxy.lock:
                    proxy.calls[key] += 1
                result, error = None, None
                try:
                    if method == "getrawtransaction":
                        raise RpcError({"code": -1, "message": "P6.5 forbids transaction-index fallback"})
                    if method == "getblock" and params[0] == proxy.missing_block:
                        with proxy.lock:
                            proxy.faults["missing_block"] += 1
                        raise RpcError({"code": -1, "message": "P6.5 injected block data unavailable"})
                    result = upstream(method, *params)
                    if method == "getblock" and len(params) > 1 and params[1] == 3 and params[0] == proxy.missing_undo:
                        with proxy.lock:
                            proxy.faults["missing_undo"] += 1
                        for tx in result["tx"]:
                            for vin in tx["vin"]:
                                vin.pop("prevout", None)
                except RpcError as exc:
                    error = exc.error
                except (OSError, ValueError):
                    error = {"code": -1, "message": "P6.5 upstream unavailable"}
                body = json.dumps(dict(jsonrpc="2.0", id=request.get("id"), result=result, error=error)).encode()
                self.send_response(200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                try:
                    self.wfile.write(body)
                except BrokenPipeError:
                    pass

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.port = self.server.server_port

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)


def capture_anchor(bh, indexer, height):
    """Require a stable pair of service heads and verify their same-height commitment links."""
    def readiness():
        upstream, downstream = bh("get_readiness"), indexer("get_readiness")
        assert upstream["consensus_ready"] and upstream["stable_height"] == height, upstream
        assert downstream["consensus_ready"] and downstream["synced_block_height"] == height, downstream
        return dict(balance_history=upstream, indexer=downstream)

    before = readiness()
    commit = bh("get_block_commit", height)
    upstream = bh("get_state_ref_at_height", dict(block_height=height))
    downstream = indexer("get_state_ref_at_height", dict(block_height=height))
    pass_commit = indexer("get_pass_block_commit", dict(block_height=height))
    local = indexer("get_local_state_commit_info")
    system = indexer("get_system_state_info")
    assert all([commit, upstream, downstream, pass_commit, local, system])
    assert commit["block_height"] == upstream["block_height"] == downstream["block_height"] == height
    assert commit["block_commit"] == upstream["latest_block_commit"] == pass_commit["balance_history_block_commit"]
    assert commit["btc_block_hash"] == upstream["stable_block_hash"] == downstream["snapshot_info"]["stable_block_hash"]
    assert upstream["snapshot_id"] == downstream["snapshot_info"]["snapshot_id"] == local["upstream_snapshot_id"] == system["upstream_snapshot_id"]
    assert local == downstream["local_state_commit_info"] and system == downstream["system_state_info"]
    assert pass_commit["block_height"] == pass_commit["balance_history_block_height"] == local["local_synced_block_height"] == height
    assert local["latest_pass_block_commit"]["block_commit"] == pass_commit["block_commit"]
    assert system["local_state_commit"] == local["local_state_commit"]
    assert bh("get_state_ref_at_height", dict(block_height=height)) == upstream
    assert indexer("get_state_ref_at_height", dict(block_height=height)) == downstream
    after = readiness()
    assert before["indexer"]["upstream_reorg_epoch"] == after["indexer"]["upstream_reorg_epoch"]
    for observation in [before, after]:
        assert observation["balance_history"]["stable_block_hash"] == commit["btc_block_hash"]
        assert observation["balance_history"]["latest_block_commit"] == commit["block_commit"]
        assert observation["indexer"]["upstream_snapshot_id"] == upstream["snapshot_id"]
        assert observation["indexer"]["local_state_commit"] == local["local_state_commit"]
        assert observation["indexer"]["system_state_id"] == system["system_state_id"]
    return dict(height=height, readiness=after, balance_history_commit=commit, balance_history_state_ref=upstream,
                indexer_state_ref=downstream, pass_commit=pass_commit, local_state=local, system_state=system)
