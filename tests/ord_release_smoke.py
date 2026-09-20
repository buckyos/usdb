#!/usr/bin/env python3
"""Qualify the external Ord binary on a fresh, isolated Bitcoin Core regtest chain."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
from ord_release import INDEX_SCHEMA, REVISION, VERSION
from ord_runtime import observe_ord


def port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


def wait_until(label, check, timeout=120):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        try:
            if result := check():
                return result
        except (OSError, ValueError, KeyError):
            pass
        time.sleep(0.2)
    raise RuntimeError(f"Timed out waiting for {label}")


def run(args):
    """Only loopback listeners and temporary wallet/data files are used."""
    if subprocess.check_output([str(args.ord), "--version"], text=True).strip() != f"ord {VERSION}":
        raise ValueError("Ord binary does not match the pinned release")
    with tempfile.TemporaryDirectory(prefix="usdb-ord-release-smoke-") as directory:
        root = Path(directory)
        core_root, ord_root = root / "bitcoin", root / "ord"
        core_root.mkdir()
        ord_root.mkdir()
        core_port, ord_port = port(), port()
        while core_port == ord_port:
            ord_port = port()
        origin = f"http://127.0.0.1:{ord_port}"
        core_origin = f"http://127.0.0.1:{core_port}"
        environment = {key: value for key, value in os.environ.items() if not key.startswith("ORD_")}
        environment.update(ORD_BITCOIN_RPC_URL=core_origin, RUST_LOG="info")
        core_command = [str(args.bitcoind), "-regtest", f"-datadir={core_root}", "-server=1", "-listen=0",
                        "-connect=0", "-dnsseed=0", "-discover=0", f"-rpcport={core_port}",
                        "-txindex=1", "-dbcache=128", "-fallbackfee=0.00001"]
        ord_command = [str(args.ord), "--regtest", "--data-dir", str(ord_root),
                       "--bitcoin-data-dir", str(core_root), "--config-dir", str(root),
                       "--index-addresses", "--index-cache-size", "67108864",
                       "--savepoint-interval", "1", "--max-savepoints", "16"]
        server_command = [*ord_command, "server", "--address", "127.0.0.1", "--http", "--http-port", str(ord_port)]
        core = server = None

        def rpc(method, *params, wallet=None):
            cookie = (core_root / "regtest/.cookie").read_bytes().strip()
            url = core_origin + (f"/wallet/{wallet}" if wallet else "")
            request = urllib.request.Request(url, json.dumps(dict(jsonrpc="2.0", id=1, method=method, params=params)).encode(),
                {"Authorization": "Basic " + base64.b64encode(cookie).decode(), "Content-Type": "application/json"})
            with urllib.request.urlopen(request, timeout=5) as response:
                value = json.load(response)
            if value.get("error"):
                raise ValueError("Isolated Core RPC failed: " + str(value["error"]))
            return value["result"]

        def get(path, *, raw=False):
            request = urllib.request.Request(origin + path, headers={"Accept": "application/json"})
            with urllib.request.urlopen(request, timeout=5) as response:
                return response.read() if raw else json.load(response)

        def synced():
            tip = rpc("getblockcount")
            anchor = rpc("getblockhash", tip)
            report = observe_ord(dict(core_height=tip, anchor_height=tip, anchor_hash=anchor),
                fetch=lambda url: get(url.split(":28030", 1)[1]),
                core_rpc=lambda method, params: rpc(method, *params))
            return report["state"] == "READY" and report["ord_height"] == tip

        def wallet(*arguments):
            result = subprocess.run([*ord_command, "wallet", "--server-url", origin, *arguments],
                                    env=environment, capture_output=True, text=True, timeout=120)
            if result.returncode:
                raise RuntimeError(f"Isolated Ord wallet {arguments[0]} failed: {result.stderr}")
            return json.loads(result.stdout)

        def stop(process):
            if process is not None and process.poll() is None:
                process.send_signal(signal.SIGINT)
                try:
                    process.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
                    raise RuntimeError("Isolated test process did not stop gracefully")

        with (root / "core.log").open("w+") as core_log, (root / "ord.log").open("w+") as ord_log:
            try:
                core = subprocess.Popen(core_command, stdout=core_log, stderr=subprocess.STDOUT)
                wait_until("Core RPC", lambda: rpc("getnetworkinfo"))
                version = rpc("getnetworkinfo")["version"]
                if version != 310100:
                    raise ValueError(f"Expected Bitcoin Core 31.1, got RPC version {version}")
                rpc("createwallet", "funding")
                miner = rpc("getnewaddress", "", "bech32m", wallet="funding")
                rpc("generatetoaddress", 105, miner)
                wait_until("txindex", lambda: rpc("getindexinfo").get("txindex", {}).get("synced"))
                server = subprocess.Popen(server_command, env=environment, stdout=ord_log, stderr=subprocess.STDOUT)
                wait_until("initial canonical Ord index", synced)
                wallet("create")  # Generated test mnemonic is never printed or persisted in reports.
                address = wallet("receive")["addresses"][0]
                rpc("sendtoaddress", address, 1, wallet="funding")
                rpc("generatetoaddress", 1, miner)
                wait_until("funding confirmation", synced)
                payload = root / "miner-pass.json"
                payload.write_text('{"p":"usdb","op":"mint","test":"ord-release-compatibility"}\n')
                inscription = wallet("inscribe", "--fee-rate", "1", "--file", str(payload))["inscriptions"][0]["id"]
                confirmed = rpc("generatetoaddress", 1, miner)[0]
                wait_until("inscription confirmation", synced)
                if get(f"/content/{inscription}", raw=True) != payload.read_bytes():
                    raise AssertionError("Inscription content differs")
                details = get(f"/inscription/{inscription}")
                if details["id"] != inscription or inscription not in json.dumps(get(f"/address/{details['address']}")):
                    raise AssertionError("Inscription or address API differs")
                # Replace with empty blocks so the old inscription must disappear.
                rpc("invalidateblock", confirmed)
                rpc("generateblock", miner, [])
                rpc("generateblock", miner, [])
                wait_until("reorg canonical index", synced)
                try:
                    get(f"/inscription/{inscription}")
                except urllib.error.HTTPError as error:
                    if error.code != 404:
                        raise
                else:
                    raise AssertionError("Orphaned inscription remained indexed")
                rpc("generatetoaddress", 1, miner)
                wait_until("reconfirmed inscription", synced)
                if get(f"/content/{inscription}", raw=True) != payload.read_bytes():
                    raise AssertionError("Reconfirmed inscription content differs")
                stop(server)
                server = None
                rpc("stop")
                core.wait(timeout=30)
                core = subprocess.Popen(core_command, stdout=core_log, stderr=subprocess.STDOUT)
                wait_until("restarted Core", lambda: rpc("getnetworkinfo"))
                server = subprocess.Popen(server_command, env=environment, stdout=ord_log, stderr=subprocess.STDOUT)
                wait_until("persisted index after restart", synced)
                if get(f"/inscription/{inscription}")["id"] != inscription:
                    raise AssertionError("Persisted inscription missing")
                return dict(ord_version=VERSION, ord_revision=REVISION, ord_index_schema=INDEX_SCHEMA,
                            ord_binary_sha256=hashlib.sha256(args.ord.read_bytes()).hexdigest(),
                            bitcoin_binary_sha256=hashlib.sha256(args.bitcoind.read_bytes()).hexdigest(),
                            bitcoin_rpc_version=version, final_height=rpc("getblockcount"),
                            checks=["txindex", "canonical_height_and_hash", "wallet_inscribe", "inscription_content",
                                    "address_lookup", "reorg_removal_and_reconfirmation", "restart_persistence"])
            except BaseException:
                ord_log.flush()
                ord_log.seek(0)
                print("Isolated Ord log tail:\n" + ord_log.read()[-5000:], file=sys.stderr)
                raise
            finally:
                try:
                    stop(server)
                finally:
                    stop(core)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ord", required=True, type=Path)
    parser.add_argument("--bitcoind", required=True, type=Path)
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    report = json.dumps(run(args), indent=2) + "\n"
    if args.report:
        args.report.write_text(report)
    print(report, end="")


if __name__ == "__main__":
    main()
