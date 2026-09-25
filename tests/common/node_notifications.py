"""Notification fixtures backed by temporary local event/config/queue files."""
from pathlib import Path
from contextlib import contextmanager
import json
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

from common.node_monitor import MonitorFixture, BASE
import node_monitor
import node_notification_config as config
from node_notification_queue import Queue


def hook(identity="hook", **fields):
    return dict(id=identity, type="webhook", url="https://example.invalid/notify", **fields)


def mail(identity="mail"):
    return dict(id=identity, type="smtp", host="mail.example.invalid", sender="node@example.invalid",
                recipients=["one@example.invalid", "two@example.invalid"], username="node", password="private")


class NotificationFixture(MonitorFixture):
    def __enter__(self):
        super().__enter__()
        self.directory = node_monitor.root(self.layout)
        config.prepare(self.directory / "notifications")
        self.config_path = self.directory / "notifications/config.json"
        self.queue = Queue(self.directory / "notifications.sqlite3", self.store.get("node_id"))
        self.save([hook()])
        self.sync(0)
        return self

    def save(self, channels, **settings):
        config.atomic_write(self.config_path, {**config.DEFAULTS, "channels": channels, **settings})

    def sync(self, seconds):
        self.queue.sync(self.store, self.config_path, BASE + int(seconds * 1000))

    def jobs(self, state=None):
        rows = self.queue.db.execute("SELECT * FROM deliveries ORDER BY rowid").fetchall()
        return [dict(row) for row in rows if state is None or row["state"] == state]

    def __exit__(self, *args):
        self.queue.close()
        super().__exit__(*args)


@contextmanager
def webhook_server(status=200, headers=None):
    received = []
    class Handler(BaseHTTPRequestHandler):
        def do_POST(self):
            received.append((dict(self.headers), self.rfile.read(int(self.headers["Content-Length"]))))
            self.send_response(status)
            for key, value in (headers or {}).items():
                self.send_header(key, value)
            self.end_headers()
        def log_message(self, *_):
            pass
    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_port}/notify", received
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


@contextmanager
def smtp_server(mode):
    """A disposable certificate and TLS SMTP endpoint; messages never leave loopback."""
    import socketserver
    import ssl
    import subprocess
    import tempfile
    from unittest import mock
    messages = []
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        certificate, key = root / "cert.pem", root / "key.pem"
        subprocess.run(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                        "-subj", "/CN=127.0.0.1", "-addext", "subjectAltName=IP:127.0.0.1",
                        "-keyout", str(key), "-out", str(certificate)], check=True,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        context.load_cert_chain(certificate, key)
        class Handler(socketserver.StreamRequestHandler):
            def finish(self):
                super().finish()
                self.connection.close()

            def handle(self):
                def respond(line):
                    self.wfile.write(line + b"\r\n"); self.wfile.flush()
                def encrypt():
                    self.rfile.close(); self.wfile.close()
                    self.connection = context.wrap_socket(self.connection, server_side=True)
                    self.rfile = self.connection.makefile("rb")
                    self.wfile = self.connection.makefile("wb")
                if mode == "tls":
                    encrypt()
                respond(b"220 local test SMTP")
                while line := self.rfile.readline():
                    command = line.split(b" ", 1)[0].strip().upper()
                    if command == b"EHLO":
                        respond(b"250-local\r\n250-STARTTLS\r\n250 AUTH PLAIN")
                    elif command == b"STARTTLS":
                        respond(b"220 Start TLS")
                        encrypt()
                    elif command == b"AUTH":
                        respond(b"235 Accepted")
                    elif command == b"RCPT" and b"refused@" in line:
                        respond(b"550 Recipient refused")
                    elif command == b"DATA":
                        respond(b"354 End with dot")
                        body = []
                        while (line := self.rfile.readline()) not in (b".\r\n", b""):
                            body.append(line)
                        messages.append(b"".join(body))
                        respond(b"250 Accepted")
                    elif command == b"QUIT":
                        respond(b"221 Bye")
                        break
                    else:
                        respond(b"250 OK")
        class Server(socketserver.ThreadingTCPServer):
            daemon_threads = True
        server = Server(("127.0.0.1", 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        try:
            with mock.patch.dict("os.environ", {"SSL_CERT_FILE": str(certificate)}):
                yield server.server_address[1], messages
        finally:
            server.shutdown(); server.server_close(); thread.join()
