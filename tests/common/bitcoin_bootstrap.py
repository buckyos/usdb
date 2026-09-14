"""Authenticated fake Core RPC with observable long-running snapshot activation."""

from collections import Counter
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import socket
import threading
import time


class BootstrapCore:
    def __init__(self, snapshot):
        self.snapshot = snapshot
        self.calls = Counter()
        self.active = False
        self.validated = False
        self.snapshot_hash = snapshot.base_hash
        self.canonical_hash = snapshot.base_hash
        self.chain = snapshot.chain
        self.pruned = False
        self.version = 310100
        self.height = snapshot.base_height
        self.headers = snapshot.base_height + 20
        self.connections = 2
        self.tip_time = int(time.time())
        self.origin_height = 963800
        self.origin_hash = "c" * 64
        self.header_delay = 0
        self.warmup_remaining = 0
        self.loading = False
        self.drop_reply = False
        self.drop_before_activation = False
        self.reject_code = None
        self.redirect = None
        self.started_load = threading.Event()
        self.release_load = threading.Event()
        self.release_load.set()
        core = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_POST(self):
                if self.headers.get("Authorization") != "Basic dGVzdDpwYXNz":
                    self.send_error(401)
                    return
                request = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                method = request["method"]
                core.calls[method] += 1
                if core.warmup_remaining:
                    core.warmup_remaining -= 1
                    body = json.dumps(dict(jsonrpc="2.0", id=request["id"], error=dict(code=-28, message="Loading"))).encode()
                    self.send_response(500)
                    self.send_header("Content-Length", str(len(body)))
                    self.end_headers()
                    self.wfile.write(body)
                    return
                if core.redirect:
                    self.send_response(302)
                    self.send_header("Location", core.redirect)
                    self.end_headers()
                    return
                active = dict(blocks=core.height if core.active else 0,
                              bestblockhash="f" * 64 if core.active else "0" * 64,
                              validated=core.validated if core.active else True,
                              verificationprogress=0.2)
                if core.active and core.snapshot_hash:
                    active["snapshot_blockhash"] = core.snapshot_hash
                result, error = None, None
                if method == "getnetworkinfo":
                    result = dict(version=core.version, connections=core.connections)
                elif method == "getblockchaininfo":
                    result = dict(chain=core.chain, pruned=core.pruned, bestblockhash=active["bestblockhash"])
                elif method == "getchainstates":
                    result = dict(headers=core.headers,
                                  chainstates=[dict(blocks=12, bestblockhash="b" * 64, validated=True), active]
                                  if core.active and not core.validated else [active])
                elif method == "getblockhash":
                    result = core.origin_hash if request["params"] == [core.origin_height] else core.canonical_hash
                elif method == "getblockheader":
                    if core.calls[method] <= core.header_delay:
                        error = dict(code=-5, message="Header not found")
                    else:
                        result = dict(hash=snapshot.base_hash, height=snapshot.base_height)
                        if request["params"] == [active["bestblockhash"]]:
                            result = dict(hash=active["bestblockhash"], height=core.height, time=core.tip_time)
                elif method == "getrpcinfo":
                    result = dict(active_commands=[dict(method="loadtxoutset")] if core.loading else [])
                elif method == "loadtxoutset":
                    core.loading = True
                    core.started_load.set()
                    # Tests release explicitly; close() also unblocks failed tests.
                    core.release_load.wait()
                    if not core.reject_code and not core.drop_before_activation:
                        core.active = True
                    core.loading = False
                    if core.drop_reply or core.drop_before_activation:
                        self.connection.shutdown(socket.SHUT_RDWR)
                        self.connection.close()
                        return
                    if core.reject_code:
                        error = dict(code=core.reject_code, message="Sensitive upstream detail must not be logged")
                    else:
                        result = dict(base_height=snapshot.base_height, tip_hash=snapshot.base_hash, coins_loaded=101)
                else:
                    error = dict(code=-32601, message="Unexpected method")
                body = json.dumps(dict(jsonrpc="2.0", id=request["id"], result=result, error=error)).encode()
                self.send_response(500 if error else 200)
                self.send_header("Content-Type", "application/json")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.release_load.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
