"""Monitor-owned notification worker and isolated senders; no additional service required."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import sqlite3
import subprocess
import sys
import tempfile
import threading
import time

from node_monitor_store import Store
import node_notification_config as config
from node_notification_queue import Queue


def milliseconds():
    return time.time_ns() // 1000000


def stop_process(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=1)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait(timeout=1)


class Worker:
    """Supervise delivery separately; all failures are reports, never collector failures."""

    def __init__(self, root, scope, start_seq=0):
        self.root, self.scope, self.start_seq = root, scope, start_seq
        self.process = None
        self.start_error = None
        self.next_start = 0

    def poll(self):
        if (self.process is None or self.process.poll() is not None) and time.monotonic() >= self.next_start:
            self.next_start = time.monotonic() + 10
            try:
                if self.process is not None:
                    try:
                        os.killpg(self.process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                self.process = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--root", str(self.root),
                                                 "--scope", json.dumps(self.scope), "--start-seq", str(self.start_seq), "--parent-pid", str(os.getpid())], start_new_session=True,
                                                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL)
                self.start_error = None
            except OSError:
                self.start_error = "WORKER_START_FAILED"
        try:
            value = config.read_json(self.root / "delivery-status.json")
            if not isinstance(value, dict) or not -5000 <= milliseconds() - value.get("updated_at_ms", 0) < 30000:
                raise ValueError("Stale delivery worker")
            if (self.process is None or self.process.poll() is not None) and value.get("state") != "failed":
                raise ValueError("Delivery worker exited")
            return value
        except (OSError, ValueError, TypeError):
            return dict(state="unavailable", code=self.start_error or "WORKER_STARTING_OR_UNAVAILABLE", counts={}, channels=[])

    def close(self):
        if self.process:
            # Include network sender descendants, including a resolver stuck in libc.
            try:
                os.killpg(self.process.pid, signal.SIGTERM)
                self.process.wait(timeout=3)
            except subprocess.TimeoutExpired:
                os.killpg(self.process.pid, signal.SIGKILL)
                self.process.wait(timeout=2)
            except ProcessLookupError:
                pass
            self.process = None


def run(root, scope, start_seq, parent_pid):
    stopped = threading.Event()
    for signum in (signal.SIGTERM, signal.SIGINT):
        signal.signal(signum, lambda *_: stopped.set())
    directory = root / "notifications"
    config.prepare(directory)
    jobs = {}
    with Store(root / "events.sqlite3", scope) as source:
        queue = Queue(root / "notifications.sqlite3", source.get("node_id"), start_seq=start_seq)
        try:
            while not stopped.is_set() and (parent_pid is None or os.getppid() == parent_pid):
                at = milliseconds()
                queue.sync(source, directory / "config.json", at)
                queue.test_request(directory / "test-request.json", at)
                for identity, (job, process, output, started) in list(jobs.items()):
                    state = queue.db.execute("SELECT state FROM deliveries WHERE id=?", (identity,)).fetchone()
                    cancelled = state is None or state[0] != "pending"
                    timed_out = time.monotonic() - started > 30
                    if cancelled or timed_out or process.poll() is not None:
                        stop_process(process)
                        result = dict(state="retry", code="SEND_TIMEOUT" if timed_out else "SENDER_FAILED")
                        if not cancelled and not timed_out and process.returncode == 0:
                            try:
                                output.seek(0)
                                result = json.loads(output.read(4096))
                                if result.get("state") not in ("accepted", "retry", "failed"):
                                    raise ValueError("Invalid sender result")
                            except (ValueError, TypeError):
                                result = dict(state="retry", code="SENDER_FAILED")
                        if not cancelled:
                            queue.result(job, result, at)
                        output.close()
                        del jobs[identity]
                busy = {v[0]["channel"] for v in jobs.values()}
                for job, channel in queue.due(at, busy):
                    if len(jobs) >= 4 or stopped.is_set():
                        break
                    output = tempfile.TemporaryFile()
                    try:
                        with tempfile.TemporaryFile() as request:
                            request.write(json.dumps(dict(channel=channel, payload=json.loads(job["payload"]), recipient=job["recipient"])).encode())
                            request.seek(0)
                            process = subprocess.Popen([sys.executable, str(Path(__file__).resolve()), "--send"],
                                                       stdin=request, stdout=output, stderr=subprocess.DEVNULL)
                    except (OSError, BrokenPipeError):
                        output.close()
                        queue.result(job, dict(state="retry", code="SENDER_START_FAILED"), at)
                        continue
                    jobs[job["id"]] = (job, process, output, time.monotonic())
                    busy.add(job["channel"])
                config.atomic_write(root / "delivery-status.json", queue.summary(at))
                stopped.wait(1)
        finally:
            for _, process, output, _ in jobs.values():
                stop_process(process)
                output.close()
            queue.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path)
    parser.add_argument("--scope")
    parser.add_argument("--send", action="store_true")
    parser.add_argument("--start-seq", type=int, default=0)
    parser.add_argument("--parent-pid", type=int)
    args = parser.parse_args()
    os.umask(0o077)
    if args.send:
        from node_notification_transport import send
        value = json.loads(sys.stdin.buffer.read(65537))
        print(json.dumps(send(value["channel"], value["payload"], value.get("recipient"))))
        return
    try:
        run(args.root, json.loads(args.scope), args.start_seq, args.parent_pid)
    except (OSError, ValueError, sqlite3.Error) as error:
        code = getattr(error, "code", getattr(error, "sqlite_errorname", type(error).__name__))
        print(f"NOTIFICATION_WORKER_FAILED: code={code}", file=sys.stderr, flush=True)
        try:
            config.atomic_write(args.root / "delivery-status.json", dict(state="failed", code=code, updated_at_ms=milliseconds(), counts={}, channels=[]))
        except (OSError, ValueError):
            pass
        sys.exit(1)


if __name__ == "__main__":
    main()
