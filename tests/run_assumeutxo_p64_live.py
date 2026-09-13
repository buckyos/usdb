#!/usr/bin/env python3
"""Run P6.4 historical-input acceptance against a fresh Core with txindex disabled."""

import argparse
import base64
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import tempfile
import time
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bitcoind", required=True, type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[1]
    root = Path(tempfile.mkdtemp(prefix="usdb-p64-live-"))
    data = root / "bitcoin"
    data.mkdir()
    with socket.socket() as reservation:
        reservation.bind(("127.0.0.1", 0))
        port = reservation.getsockname()[1]
    url = f"http://127.0.0.1:{port}"
    cookie = data / "regtest/.cookie"
    print(f"P6.4 isolated run: {root}", flush=True)
    with (root / "core.log").open("w") as log:
        core = subprocess.Popen([str(args.bitcoind), "-regtest", f"-datadir={data}", "-server=1", "-txindex=0", "-prune=0",
                                 "-listen=0", "-connect=0", "-dnsseed=0", "-discover=0", "-dbcache=64", f"-rpcport={port}"], stdout=log, stderr=log)
        started = time.monotonic()
        try:
            for _ in range(150):
                if core.poll() is not None:
                    raise RuntimeError("Isolated Core exited before RPC became ready")
                try:
                    request = urllib.request.Request(url, json.dumps({"id": 1, "method": "getblockcount", "params": []}).encode(),
                        {"Authorization": "Basic " + base64.b64encode(cookie.read_bytes().strip()).decode()})
                    with urllib.request.urlopen(request, timeout=2) as response:
                        assert json.load(response)["result"] == 0
                    break
                except OSError:
                    time.sleep(0.1)
            else:
                raise RuntimeError("Isolated Core RPC start timed out")
            env = dict(os.environ, CARGO_BUILD_JOBS="2", P64_CORE_URL=url, P64_CORE_COOKIE=str(cookie), P64_LIVE_RESULT=str(root / "result.json"))
            with (root / "test.log").open("w") as test_log:
                subprocess.run(["cargo", "test", "--offline", "--locked", "--manifest-path", "src/btc/Cargo.toml", "-p", "usdb-indexer",
                                "--bin", "usdb-indexer", "real_core_spent_prevouts_reveal_and_restart_without_txindex", "--", "--ignored", "--test-threads=1"],
                               cwd=repo, env=env, stdout=test_log, stderr=test_log, check=True)
            result = json.loads((root / "result.json").read_text())
            probe = subprocess.run(["python3", "docker/scripts/tools/check_bitcoin_block_data.py", "--url", url, "--cookie-file", str(cookie),
                                    "--expected-chain", "regtest", "--height", "102", "--block-hash", result["hash"]],
                                   cwd=repo, env=dict(os.environ, PYTHONDONTWRITEBYTECODE="1"), check=True, text=True, capture_output=True)
            result["preflight"] = json.loads(probe.stdout)
            result["elapsed_seconds_including_build"] = round(time.monotonic() - started, 3)
            (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
            print(f"P6.4 passed: {root / 'result.json'}", flush=True)
        finally:
            if core.poll() is None:
                core.send_signal(signal.SIGTERM)
                core.wait(timeout=30)


if __name__ == "__main__":
    main()
