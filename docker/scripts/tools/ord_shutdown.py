#!/usr/bin/env python3
"""Wait for an explicit Ord stop without Docker's automatic SIGKILL deadline."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import time


def shutdown_status(root: Path, elapsed: int) -> str:
    """Use bounded, allowlisted observations; tolerate older supervisors."""
    text = f"Waiting for Ord graceful shutdown: elapsed={elapsed}s"
    try:
        path = root / "progress.json"
        if path.is_symlink() or path.stat().st_size > 8192:
            return text + "; progress unavailable"
        report = json.loads(path.read_text())
        observed = report.get("observed_at_ms")
        fresh = type(observed) is int and 0 <= time.time() * 1000 - observed <= 60000
        height = report.get("ord_height")
        if type(height) is int and height >= 0:
            text += f"; last committed height={height}"
        if fresh:
            phase = report.get("index_phase")
            if phase in {"STARTING", "RECOVERING", "PROCESSING", "COMMITTING", "IDLE"}:
                text += f"; phase={phase}"
            read, written, seconds = (report.get(key) for key in
                                      ("sample_read_bytes", "sample_write_bytes", "sample_elapsed_secs"))
            if all(type(v) is int and v >= 0 for v in (read, written, seconds)) and seconds:
                text += f"; I/O sample={read / 1024**2:.1f} MiB read/{written / 1024**2:.1f} MiB written in {seconds}s"
        else:
            text += "; progress stale (container exit is still pending)"
    except (OSError, ValueError, TypeError, AttributeError):
        text += "; progress unavailable"
    return text


def stop_ord(container: str, root: Path) -> None:
    """Stop only Ord first; any failure leaves upstream services and logs intact."""
    initial = subprocess.run(["docker", "inspect", "--format", "{{json .State}}", container],
                             capture_output=True, text=True, check=True, timeout=15)
    previous = json.loads(initial.stdout)
    if previous.get("Status") in {"exited", "created", "dead"} and previous.get("Running") is False:
        print(f"Ord already stopped (last exit={previous.get('ExitCode')}); continuing node shutdown.", flush=True)
        return
    print("Stopping Ord gracefully; waiting for its current database batch. "
          "No automatic force-kill deadline. Ctrl-C stops this wait; it does not cancel Ord shutdown. "
          "Inspect with 'usdb-node logs ord-server', then rerun 'usdb-node down'.", flush=True)
    started = time.monotonic()
    # A Docker stop request disables restart policy while waiting. Sending a
    # bare SIGTERM instead can let unless-stopped restart a successfully exited Ord.
    child = subprocess.Popen(["docker", "stop", "--timeout", "-1", container],
                             stdout=subprocess.DEVNULL, start_new_session=True)
    try:
        while True:
            print(shutdown_status(root, int(time.monotonic() - started)), flush=True)
            try:
                code = child.wait(timeout=15)
                break
            except subprocess.TimeoutExpired:
                continue
        if code:
            raise ValueError("Ord stop request failed; remaining services were not stopped. Inspect Docker access and Ord logs.")
        result = subprocess.run(["docker", "inspect", "--format", "{{json .State}}", container],
                                capture_output=True, text=True, check=True, timeout=15)
        state = json.loads(result.stdout)
        if state.get("Running") is not False or state.get("ExitCode") != 0 or state.get("OOMKilled"):
            raise ValueError("Ord did not exit cleanly; container and logs retained. "
                             "Inspect 'usdb-node logs ord-server' before continuing shutdown.")
        print(f"Ord graceful shutdown completed in {int(time.monotonic() - started)}s.", flush=True)
    finally:
        if child.poll() is None:
            # Terminate only the waiting Docker client, never the Ord process.
            child.terminate()
            try:
                child.wait(timeout=15)
            except subprocess.TimeoutExpired:
                child.kill()
                child.wait(timeout=15)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--container", required=True)
    parser.add_argument("--data-dir", type=Path, required=True)
    args = parser.parse_args()
    try:
        stop_ord(args.container, args.data_dir)
    except KeyboardInterrupt:
        print("Ord shutdown wait interrupted; remaining services were not stopped. Rerun usdb-node down to resume.")
        return 130
    except (OSError, ValueError, subprocess.SubprocessError) as error:
        print(f"Ord shutdown incomplete: {error}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
