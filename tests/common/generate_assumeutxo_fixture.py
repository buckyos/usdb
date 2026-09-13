#!/usr/bin/env python3
"""Generate a small real Core snapshot and replay blocks using an isolated temporary regtest node."""

import argparse
import base64
import hashlib
import json
from pathlib import Path
import socket
import struct
import subprocess
import tempfile
import time
import urllib.request


def compact(n):
    return bytes([n]) if n < 253 else b"\xfd" + struct.pack("<H", n)


def transaction(txid, vout, outputs):
    data = struct.pack("<I", 2) + b"\x01" + bytes.fromhex(txid)[::-1]
    data += struct.pack("<I", vout) + b"\x00" + b"\xff" * 4 + compact(len(outputs))
    for value, script in outputs:
        data += struct.pack("<Q", value) + compact(len(script)) + script
    return (data + bytes(4)).hex()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bitcoind", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--p5", action="store_true", help="Include distinct historical scripts and real replacement branches")
    parser.add_argument("--chain", type=Path, help="Reuse existing real blocks instead of mining a new chain")
    parser.add_argument("--snapshot-height", type=int, default=101, help="Snapshot baseline when reusing --chain")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    if any(args.output.iterdir()):
        raise RuntimeError("Fixture output must be empty")
    with tempfile.TemporaryDirectory(prefix="usdb-assumeutxo-fixture-") as temporary:
        root = Path(temporary)
        with socket.socket() as reservation:
            reservation.bind(("127.0.0.1", 0))
            port = reservation.getsockname()[1]
        with (root / "console.log").open("wb") as log:
            process = subprocess.Popen([str(args.bitcoind), "-regtest", f"-datadir={root}",
                                        "-server=1", "-listen=0", "-connect=0", "-dnsseed=0",
                                        "-discover=0", "-dbcache=64", f"-rpcport={port}"], stdout=log, stderr=log)

            def rpc(method, *params):
                cookie = (root / "regtest/.cookie").read_bytes().strip()
                request = urllib.request.Request(f"http://127.0.0.1:{port}",
                    json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).encode(),
                    {"Authorization": "Basic " + base64.b64encode(cookie).decode(), "Content-Type": "application/json"})
                with urllib.request.urlopen(request, timeout=30) as response:
                    result = json.load(response)
                if result.get("error"):
                    raise RuntimeError(result["error"])
                return result["result"]

            try:
                for _ in range(100):
                    try:
                        rpc("getblockcount")
                        break
                    except (OSError, RuntimeError):
                        time.sleep(0.1)
                else:
                    raise RuntimeError("Isolated regtest RPC did not start")
                if args.chain:
                    chain_bytes = args.chain.read_bytes()
                    blocks = json.loads(chain_bytes)["blocks"]
                    if not 0 < args.snapshot_height < len(blocks):
                        raise RuntimeError("Snapshot height must exist in the supplied chain")
                    for block in blocks[1:args.snapshot_height + 1]:
                        result = rpc("submitblock", block)
                        if result is not None:
                            raise RuntimeError(result)
                    snapshot = root / "snapshot.dat"
                    dump = rpc("dumptxoutset", str(snapshot), "latest")
                    stats = rpc("gettxoutsetinfo", "hash_serialized_3")
                    data = snapshot.read_bytes()
                    identity = dict(network="regtest", base_height=args.snapshot_height,
                                    base_hash=rpc("getblockhash", args.snapshot_height),
                                    file_sha256=hashlib.sha256(data).hexdigest(),
                                    hash_serialized_3=stats["hash_serialized_3"])
                    (args.output / "snapshot.dat").write_bytes(data)
                    (args.output / "identity.json").write_text(json.dumps(identity, indent=2) + "\n")
                    dump.pop("path", None)
                    evidence = dict(core_version=rpc("getnetworkinfo")["subversion"], dump=dump,
                                    stats=stats, chain_sha256=hashlib.sha256(chain_bytes).hexdigest())
                    (args.output / "generation.json").write_text(json.dumps(evidence, indent=2) + "\n")
                    print(json.dumps(identity, indent=2))
                    return
                if args.p5:
                    hashes = rpc("generatetodescriptor", 1, "raw(5161)")
                    hashes += rpc("generatetodescriptor", 1, "raw(5151)")
                    hashes += rpc("generatetodescriptor", 98, "raw(51)")
                else:
                    hashes = rpc("generatetodescriptor", 100, "raw(51)")
                first = rpc("getblock", hashes[0], 2)["tx"][0]["txid"]
                outputs = [(49 * 100_000_000, b"\x51")] + [(0, b"\x51")] * 299
                outputs += [(0, b""), (0, b"\x6a"), (0, b"\x76\xa9\x14" + bytes(20) + b"\x88\xac"),
                            (0, b"\xa9\x14" + bytes(20) + b"\x87")]
                raw = transaction(first, 0, outputs)
                base = rpc("generateblock", "raw(51)", [raw])["hash"]
                snapshot = root / "snapshot.dat"
                dump = rpc("dumptxoutset", str(snapshot), "latest")
                stats = rpc("gettxoutsetinfo", "hash_serialized_3")
                (args.output / "snapshot.dat").write_bytes(snapshot.read_bytes())
                txid = rpc("decoderawtransaction", raw)["txid"]
                spend = transaction(txid, 0, [(20 * 100_000_000, b"\x51"), (28 * 100_000_000, b"\x51\x75\x51" if args.p5 else b"\x51")])
                rpc("generateblock", "raw(51)", [spend])
                spend_id = rpc("decoderawtransaction", spend)["txid"]
                parent = transaction(spend_id, 0, [(19 * 100_000_000, b"\x51\x61" if args.p5 else b"\x51")])
                parent_id = rpc("decoderawtransaction", parent)["txid"]
                child = transaction(parent_id, 0, [(18 * 100_000_000, b"\x52" if args.p5 else b"\x51")])
                rpc("generateblock", "raw(51)", [parent, child])
                fixture = {"core_version": rpc("getnetworkinfo")["subversion"], "dump": dump,
                           "stats": stats, "blocks": [rpc("getblock", rpc("getblockhash", h), 0) for h in range(104)]}
                fixture["dump"].pop("path", None)
                if args.p5:
                    # Replace only post-baseline blocks with a longer, independently mined branch.
                    rpc("invalidateblock", rpc("getblockhash", 102))
                    alternate = transaction(txid, 0, [(47 * 100_000_000, b"\x53")])
                    rpc("generateblock", "raw(53)", [alternate])
                    rpc("generateblock", "raw(53)", [])
                    rpc("generateblock", "raw(53)", [])
                    fixture["fork_blocks"] = [rpc("getblock", rpc("getblockhash", h), 0) for h in range(105)]
                    # A second branch crosses the baseline and must be refused by an imported DB.
                    rpc("invalidateblock", base)
                    rpc("generateblock", "raw(54)", [])
                    rpc("generateblock", "raw(54)", [])
                    rpc("generateblock", "raw(54)", [])
                    rpc("generateblock", "raw(54)", [])
                    rpc("generateblock", "raw(54)", [])
                    fixture["deep_fork_blocks"] = [rpc("getblock", rpc("getblockhash", h), 0) for h in range(106)]
                    fixture["scenarios"] = {"history_only_at_baseline": "5161", "unchanged_live_script": "5151", "created_after_baseline": "517551", "same_block_spend": "5161"}
                (args.output / "chain.json").write_text(json.dumps(fixture, indent=2) + "\n")
                identity = {"network": "regtest", "base_height": 101, "base_hash": base,
                            "file_sha256": hashlib.sha256(snapshot.read_bytes()).hexdigest(),
                            "hash_serialized_3": stats["hash_serialized_3"]}
                (args.output / "identity.json").write_text(json.dumps(identity, indent=2) + "\n")
                print(json.dumps(identity, indent=2))
            finally:
                try:
                    rpc("stop")
                except (OSError, RuntimeError):
                    process.terminate()
                try:
                    process.wait(timeout=20)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


if __name__ == "__main__":
    main()
