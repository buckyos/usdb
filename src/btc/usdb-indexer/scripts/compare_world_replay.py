#!/usr/bin/env python3
"""Start fresh BTC-side indexes and compare saved post-reorg semantic checkpoints."""

import argparse
import json
import os
from pathlib import Path
import re
import signal
import socket
import subprocess
import tempfile
import time
from urllib import request

from regtest_world_simulator import RegtestWorldSimulator
from world_replay_state import SCHEMA, capture_state, digest, first_difference, require, write_json


def load_session(path):
    """Use the latest session only; its recovery-restored manifest covers earlier sessions."""
    start = end = None
    for line in Path(path).read_text().splitlines():
        event = json.loads(line)
        if event.get("event") == "session_start":
            start, end = event, None
        elif event.get("event") == "session_end":
            end = event
    require(start is not None and end is not None, "no completed simulation session")
    require(start.get("replay_check_enabled") is True, "replay checkpoint collection disabled")
    checkpoints = end.get("replay_checkpoints", [])
    reorgs = [item for item in checkpoints if item.get("kind") == "reorg"]
    finals = [item for item in checkpoints if item.get("kind") == "final"]
    require(len(reorgs) == end["reorg_events_applied"] > 0, "missing post-reorg checkpoints")
    require(len(finals) == 1 and len(checkpoints) == len(reorgs) + 1, "missing/invalid final checkpoint")
    require(finals[0]["height"] == end["finalization"]["final_height"], "final checkpoint height mismatch")
    require(len({item["file"] for item in checkpoints}) == len(checkpoints), "duplicate checkpoint files")
    return start, end


class ReplayRPCError(ValueError):
    def __init__(self, method, payload):
        self.code = payload.get("code")
        self.message = payload.get("message")
        super().__init__(f"world replay: {method}: {payload}")


class ReplayRPC:
    def __init__(self, url, deadline, processes):
        self.url, self.deadline, self.processes = url, deadline, processes

    def __call__(self, method, params):
        require(time.monotonic() < self.deadline, "comparison time budget exceeded")
        for process in self.processes:
            if process.poll() is not None:
                raise RuntimeError(f"world replay service exited: {process.args}")
        payload = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode()
        req = request.Request(self.url, data=payload, headers={"Content-Type": "application/json"})
        with request.urlopen(req, timeout=min(8, self.deadline - time.monotonic())) as response:
            value = json.load(response)
        if "error" in value:
            raise ReplayRPCError(method, value["error"])
        require("result" in value, f"{method}: {value}")
        return value["result"]


def wait_historical_refs(usdb, checkpoints, final_height, deadline):
    """Bulk catch-up publishes head before backfilling exact historical upstream anchors."""
    hashes = {item["height"]: item["block_hash"] for item in checkpoints}
    # Backfill is ascending; also wait near the final head to cover the entire
    # historical interval, even when all checkpoints happen to be early anchors.
    heights = sorted(set(hashes) | {max(1, final_height - 1)})
    for height in heights:
        while True:
            try:
                state = usdb("get_state_ref_at_height", [{"block_height": height}])
                require(state["block_height"] == height, "historical height mismatch")
                if height in hashes:
                    require(state["snapshot_info"]["stable_block_hash"] == hashes[height],
                            f"historical canonical hash mismatch at {height}")
                break
            except ReplayRPCError as error:
                if error.code != -32049 or error.message != "HISTORY_NOT_AVAILABLE":
                    raise
                require(time.monotonic() < deadline, f"historical backfill timeout at {height}: {error}")
                time.sleep(min(0.5, max(0, deadline - time.monotonic())))


def wait_ready(balance, usdb, height, block_hash, deadline):
    """Require exact height and hash, plus both consensus readiness gates."""
    last_error = "services not started"
    while time.monotonic() < deadline:
        try:
            for rpc in (balance, usdb):
                ready = rpc("get_readiness", [])
                require(ready.get("consensus_ready") is True, "replay service not consensus-ready")
                snapshot = rpc("get_snapshot_info", [])
                current_height = snapshot.get("stable_height", snapshot.get("balance_history_stable_height"))
                require(current_height == height and snapshot["stable_block_hash"] == block_hash,
                        f"replay frontier not converged: height={current_height}, target={height}")
            require(usdb("get_synced_block_height", []) == height, "replay indexer still catching up")
            return
        except (ValueError, OSError) as error:
            last_error = str(error)
            time.sleep(min(0.5, max(0, deadline - time.monotonic())))
    raise ValueError(f"world replay: readiness timeout: {last_error}")


def prepare_configs(source_balance, source_usdb, scratch, balance_port, usdb_port):
    """Copy configuration only; both service roots are newly created and contain no state."""
    balance_root, usdb_root = scratch / "balance-history", scratch / "usdb-indexer"
    balance_root.mkdir()
    usdb_root.mkdir()
    config = (source_balance / "config.toml").read_text()
    config, count = re.subn(r'(?m)^root_dir = .*$', lambda _: f"root_dir = {json.dumps(str(balance_root))}", config)
    require(count == 1, "unexpected balance-history root configuration")
    config, count = re.subn(r'(?m)(^\[rpc_server\]\n)port = \d+$', lambda match: f"{match[1]}port = {balance_port}", config)
    require(count == 1, "unexpected balance-history RPC configuration")
    (balance_root / "config.toml").write_text(config)
    config = json.loads((source_usdb / "config.json").read_text())
    require(config["bitcoin"]["network"] == "regtest", "replay is restricted to regtest")
    require(config["usdb"]["genesis_block_height"] == 1, "replay must start at genesis height 1")
    require(config["usdb"]["inscription_source"] == "bitcoind", "replay requires canonical Bitcoin inscriptions")
    config["balance_history"]["rpc_url"] = f"http://127.0.0.1:{balance_port}"
    config["usdb"]["rpc_server_port"] = usdb_port
    (usdb_root / "config.json").write_text(json.dumps(config, indent=2) + "\n")
    return balance_root, usdb_root


def stop_services(processes):
    """Stop only the fresh process groups created by this comparison, including cargo children."""
    for process in reversed(processes):
        try:
            os.killpg(process.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
    for process in reversed(processes):
        try:
            process.wait(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait(timeout=5)


def run(args):
    started = time.monotonic()
    deadline = started + args.timeout_sec
    start, end = load_session(args.report)
    output = args.output_dir
    # A new directory on every attempt prevents a stale successful replay from being reused.
    scratch = Path(tempfile.mkdtemp(prefix="canonical-replay-", dir=args.work_dir))
    processes, logs, comparisons = [], [], []
    height = end["finalization"]["final_height"]
    final = next(item for item in end["replay_checkpoints"] if item["kind"] == "final")
    report = {
        "schema": SCHEMA, "event": "replay_comparison", "seed": start["seed"],
        "session_start_ts_ms": start["ts_ms"], "completed_work_ticks": end["completed_work_ticks"],
        "final_height": height, "final_hash": final["block_hash"],
        "fresh_databases": True, "comparisons": comparisons, "status": "failed",
    }
    try:
        ports = (args.balance_port, args.indexer_port)
        require(len(set(ports)) == 2, "replay ports must be distinct")
        for port in ports:
            with socket.socket() as probe:
                # Match the server's bind behavior after a previous attempt;
                # TIME_WAIT sockets must not be mistaken for a live listener.
                probe.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
                probe.bind(("127.0.0.1", port))
        roots = prepare_configs(args.balance_root, args.indexer_root, scratch, *ports)
        for package, root in zip(("balance-history", "usdb-indexer"), roots):
            log = (output / f"replay-{package}.log").open("w")
            logs.append(log)
            processes.append(subprocess.Popen([
                "cargo", "run", "--manifest-path", str(args.manifest), "-p", package, "--",
                "--root-dir", str(root), "--skip-process-lock",
            ], stdout=log, stderr=subprocess.STDOUT, start_new_session=True))
            if package == "balance-history":
                balance = ReplayRPC(f"http://127.0.0.1:{args.balance_port}", deadline, processes)
                while True:
                    try:
                        require(balance("get_network_type", []) == "regtest", "unexpected replay network")
                        break
                    except OSError:
                        require(time.monotonic() < deadline, "balance-history startup timeout")
                        time.sleep(0.5)
        balance = ReplayRPC(f"http://127.0.0.1:{args.balance_port}", deadline, processes)
        usdb = ReplayRPC(f"http://127.0.0.1:{args.indexer_port}", deadline, processes)
        print(f"[world-replay] Syncing fresh databases to height={height}, scratch={scratch}", flush=True)
        wait_ready(balance, usdb, height, final["block_hash"], deadline)
        wait_historical_refs(usdb, end["replay_checkpoints"], height, deadline)
        report["sync_elapsed_ms"] = round((time.monotonic() - started) * 1000)
        for checkpoint in end["replay_checkpoints"]:
            filename = checkpoint["file"]
            require(Path(filename).name == filename, "checkpoint must be a basename")
            expected = json.loads((output / filename).read_text())
            require(digest(expected) == checkpoint["sha256"], f"checkpoint evidence changed: {filename}")
            require(expected["schema"] == SCHEMA, "unknown checkpoint schema")
            require(expected["history_included"] is (checkpoint["kind"] == "final"),
                    "final checkpoint must include full ledgers")
            require(expected["height"] == checkpoint["height"] and expected["block_hash"] == checkpoint["block_hash"],
                    "checkpoint identity mismatch")
            actual = capture_state(
                usdb, balance, expected["height"], expected["block_hash"], expected["requested_owners"],
                RegtestWorldSimulator.build_consensus_context_from_state_ref,
                history=expected["history_included"],
            )
            difference = first_difference(expected, actual)
            if difference:
                write_json(output / f"actual-{filename}", actual)
                write_json(output / "difference.json", {"checkpoint": checkpoint, "difference": difference})
                raise ValueError(f"world replay mismatch: checkpoint={filename}, path={difference['path']}")
            comparisons.append({**checkpoint, "actual_sha256": digest(actual)})
            print(f"[world-replay] Matched {filename}: passes={len(actual['passes'])}, owners={len(actual['owners'])}", flush=True)
        # A moving source head must not turn this into a comparison of unrelated snapshots.
        wait_ready(balance, usdb, height, final["block_hash"], deadline)
        report["status"] = "ok"
    except BaseException as error:
        report["error"] = str(error)
        raise
    finally:
        stop_services(processes)
        for log in logs:
            log.close()
        report["elapsed_ms"] = round((time.monotonic() - started) * 1000)
        write_json(output / "comparison.json", report)
    with args.report.open("a") as target:
        target.write(json.dumps(report, separators=(",", ":")) + "\n")


def interrupt(*_):
    raise KeyboardInterrupt("replay interrupted")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for option in ("report", "output-dir", "work-dir", "balance-root", "indexer-root", "manifest"):
        parser.add_argument(f"--{option}", type=Path, required=True)
    parser.add_argument("--balance-port", type=int, required=True)
    parser.add_argument("--indexer-port", type=int, required=True)
    parser.add_argument("--timeout-sec", type=int, default=1800)
    args = parser.parse_args()
    require(args.timeout_sec > 0, "timeout must be positive")
    # Ensure interruption also reaches finally and closes the fresh service groups.
    signal.signal(signal.SIGTERM, interrupt)
    run(args)


if __name__ == "__main__":
    main()
