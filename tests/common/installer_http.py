"""Local release downloads with controlled HTTP failures and response delays."""

from collections import Counter
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import threading


class InstallerHTTPServer:
    """Serve temporary public assets; a hook can delay or reject one request."""

    def __init__(self, root, before_request=lambda path, count: None):
        self.requests = Counter()
        fixture = self

        class Handler(SimpleHTTPRequestHandler):
            def log_message(self, *args):
                pass

            def do_GET(self):
                fixture.requests[self.path] += 1
                status = before_request(self.path, fixture.requests[self.path])
                if status is not None:
                    self.send_error(status)
                    return
                super().do_GET()

        self.server = ThreadingHTTPServer(("127.0.0.1", 0), partial(Handler, directory=str(root)))
        self.url = f"http://127.0.0.1:{self.server.server_port}"
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)

    def __enter__(self):
        self.thread.start()
        return self

    def __exit__(self, *args):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=5)
