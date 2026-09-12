#!/usr/bin/env python3
"""Print an independent binary-encoding vector for the proposed bootstrap-origin commitment."""

import hashlib
import json
from pathlib import Path
import struct


def sha(data):
    return hashlib.sha256(data).digest()


def main():
    fixture = Path(__file__).resolve().parents[1] / "fixtures/assumeutxo-p5/chain.json"
    blocks = json.loads(fixture.read_text())["blocks"]
    genesis_hash = sha(sha(bytes.fromhex(blocks[0])[:80]))
    origin_hash = sha(sha(bytes.fromhex(blocks[103])[:80]))
    rows = [
        (bytes.fromhex("22" * 32), 256, bytes.fromhex("ab" * 32), 7),
        (bytes.fromhex("11" * 32), 1, bytes.fromhex("01" * 32), 5),
        (bytes.fromhex("11" * 32), 0, bytes.fromhex("01" * 32), 0),
    ]
    encoded_utxos = sorted((txid + struct.pack(">I", vout) + script + struct.pack(">Q", amount)
                            for txid, vout, script, amount in rows), reverse=True)
    balances = {}
    for _, _, script, amount in rows:
        balances[script] = balances.get(script, 0) + amount
    encoded_balances = [script + struct.pack(">Q", amount)
                        for script, amount in sorted(balances.items(), reverse=True) if amount]
    identity = dict(
        schema_version="balance-history-bootstrap-origin:v1",
        commit_protocol_version="2.0.0",
        network="regtest",
        origin_height=103,
        origin_block_hash=origin_hash[::-1].hex(),
        data_model_version="balance-history-data-model:bip30-generations-core-unspendable-v2",
        utxos=dict(rows=len(rows), total_sats=12, sha256=sha(b"".join(encoded_utxos)).hex()),
        balances=dict(rows=len(encoded_balances), total_sats=12, sha256=sha(b"".join(encoded_balances)).hex()),
    )
    encoded = b""
    for field in ["schema_version", "commit_protocol_version", "data_model_version"]:
        data = identity[field].encode()
        encoded += struct.pack(">I", len(data)) + data
    encoded += genesis_hash + struct.pack(">I", 103) + origin_hash
    for table in [identity["utxos"], identity["balances"]]:
        encoded += struct.pack(">QQ", table["rows"], table["total_sats"]) + bytes.fromhex(table["sha256"])
    print(json.dumps(dict(identity=identity, encoded_hex=encoded.hex(), origin_commit=sha(encoded).hex()), indent=2))


if __name__ == "__main__":
    main()
