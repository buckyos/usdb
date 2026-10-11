"""Isolated subprocess ownership and bounded, observable P1 fault handshakes."""
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time


def write_json(path, value):
    pending = path.with_suffix(".tmp")
    pending.write_text(json.dumps(value))
    pending.replace(path)


class Processes:
    def __init__(self, case):
        output = os.environ.get("USDB_TEST_ARTIFACT_DIR")
        if output:
            Path(output).mkdir(parents=True, exist_ok=True)
        self.root = Path(tempfile.mkdtemp(prefix="process-recovery-", dir=output))
        self.children = []
        self.events = []
        self.started = time.monotonic()
        write_json(self.root / "summary.json", {"case": case, "scope": "isolated-processes"})

    def record(self, **event):
        self.events.append(dict(at_ms=time.time_ns() // 1_000_000,
                                elapsed_secs=round(time.monotonic() - self.started, 3), **event))

    def start(self, mode, *args, env=None):
        log = self.root / f"{len(self.children):02d}-{mode}.log"
        with log.open("w") as stream:
            child = subprocess.Popen([sys.executable, str(Path(__file__).with_name("process_recovery_worker.py")),
                                      mode, str(self.root), *args],
                                     env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1", **(env or {})},
                                     stdin=subprocess.DEVNULL, stdout=stream, stderr=subprocess.STDOUT,
                                     start_new_session=True)
        child.log_path = log
        self.children.append(child)
        self.record(action="start", mode=mode, pid=child.pid,
                    start_ticks=Path(f"/proc/{child.pid}/stat").read_text().rsplit(")", 1)[1].split()[19])
        return child

    def wait(self, predicate, child=None, timeout=12):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = predicate()
            if value:
                return value
            if child is not None and child.poll() is not None:
                raise AssertionError(f"Worker exited {child.returncode}: {child.log_path.read_text()}")
            time.sleep(0.02)
        raise AssertionError(f"Recovery condition timed out; evidence: {self.root}")

    def boundary(self, child, phase):
        def reached():
            path = self.root / "reached.json"
            if not path.exists():
                return False
            value = json.loads(path.read_text())
            return value if value["pid"] == child.pid and value["phase"] == phase else False
        value = self.wait(reached, child)
        self.record(action="boundary", pid=child.pid, phase=phase)
        return value

    def stop(self, child, sig=signal.SIGTERM):
        # Signal the selected process only, not its unrelated peers or the test runner.
        child.send_signal(sig)
        code = child.wait(timeout=8)
        self.record(action="stop", pid=child.pid, signal=sig.name, exit=code)
        return code

    def success(self, child):
        code = child.wait(timeout=12)
        self.record(action="completed", pid=child.pid, exit=code)
        if code:
            raise AssertionError(f"Worker exited {code}: {child.log_path.read_text()}")

    def close(self):
        for child in self.children:
            # Includes descendants left after a failed assertion, scoped to our own session.
            try:
                os.killpg(child.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            child.wait(timeout=5)
        write_json(self.root / "timeline.json", self.events)
