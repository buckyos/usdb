"""Local HTTPS range origin for real-curl snapshot download integration tests."""

import collections
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
import re
import shlex
import socket
import ssl
import subprocess
import threading


class SnapshotRangeServer:
    def __init__(self, root: Path, payload: bytes):
        self.payload = payload
        self.requests = []
        self.plans = {}
        self.lock = threading.Lock()
        self.active = 0
        self.max_active = 0
        self.delay = threading.Event()
        origin = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                if self.path == "/redirect":
                    self.send_response(302)
                    self.send_header("Location", origin.url)
                    self.end_headers()
                    return
                match = re.fullmatch(r"bytes=(\d+)-(\d+)", self.headers.get("Range", ""))
                if match is None:
                    self.send_error(400)
                    return
                start, end = map(int, match.groups())
                with origin.lock:
                    origin.requests.append((start, end))
                    plan = origin.plans.get(start, collections.deque())
                    mode = plan.popleft() if plan else "ok"
                    origin.active += 1
                    origin.max_active = max(origin.max_active, origin.active)
                try:
                    # Keep concurrent requests overlapping without relying on network timing.
                    origin.delay.wait(0.03)
                    data = origin.payload[start:end + 1]
                    self.send_response(200 if mode == "status-200" else 206)
                    actual_start = start + 1 if mode == "wrong-range" else start
                    actual_size = len(origin.payload) + (1 if mode == "wrong-total" else 0)
                    self.send_header("Content-Range", f"bytes {actual_start}-{end}/{actual_size}")
                    if mode == "duplicate-range":
                        self.send_header("Content-Range", f"bytes {start}-{end}/{actual_size}")
                    if mode == "encoded":
                        self.send_header("Content-Encoding", "gzip")
                    if mode != "oversize":
                        self.send_header("Content-Length", str(len(data)))
                    self.end_headers()
                    if mode == "cut":
                        self.wfile.write(b"X" * (len(data) // 2))
                        self.wfile.flush()
                        self.connection.shutdown(socket.SHUT_WR)
                    else:
                        self.wfile.write(data + (b"overflow" if mode == "oversize" else b""))
                except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
                    pass
                finally:
                    with origin.lock:
                        origin.active -= 1

        cert, key = root / "server.crt", root / "server.key"
        subprocess.run([
            "openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
            "-keyout", str(key), "-out", str(cert), "-days", "1",
            "-subj", "/CN=localhost", "-addext", "subjectAltName=IP:127.0.0.1",
        ], check=True, capture_output=True)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(cert, key)
        self.server.socket = context.wrap_socket(self.server.socket, server_side=True)
        self.url = f"https://127.0.0.1:{self.server.server_port}/artifact"
        self.curl = root / "curl-test-origin"
        self.curl.write_text(
            f'#!/bin/sh\nexec /usr/bin/curl --noproxy "*" --cacert {shlex.quote(str(cert))} "$@"\n'
        )
        self.curl.chmod(0o755)
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.delay.set()
        self.server.shutdown()
        self.server.server_close()
        self.thread.join()
