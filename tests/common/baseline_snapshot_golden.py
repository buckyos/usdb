#!/usr/bin/env python3
"""Independently encode a baseline SQLite artifact using only Python's standard library."""

import hashlib
import json
from pathlib import Path
import sqlite3
import sys


def calculate(path):
    connection = sqlite3.connect(Path(path).resolve().as_uri() + "?mode=ro", uri=True)
    metadata = json.loads(connection.execute("SELECT state_json FROM meta").fetchone()[0])
    identity = {name: metadata["identity"][name] for name in (
        "network", "height", "block_hash", "data_model_version", "commit_protocol_version",
        "registry_policy", "balance_query_floor", "history_query_floor",
    )}
    specs = {
        "balances": ("SELECT script_hash, balance FROM balances ORDER BY script_hash", ("bytes", 8)),
        "utxos": ("SELECT outpoint, script_hash, value FROM utxos ORDER BY outpoint", ("bytes", "bytes", 8)),
        "block_commits": ("SELECT block_height, btc_block_hash, balance_delta_root, block_commit FROM block_commits ORDER BY block_height", (4, "bytes", "bytes", "bytes")),
        "script_registry": ("SELECT script_hash, script_pubkey FROM script_registry ORDER BY script_hash", ("bytes", "variable")),
        "genesis_block": ("SELECT raw_block FROM genesis_block ORDER BY id", ("variable",)),
    }
    tables = {}
    for name in sorted(specs):
        query, encoding = specs[name]
        digest, count = hashlib.sha256(), 0
        for row in connection.execute(query):
            for value, kind in zip(row, encoding, strict=True):
                if isinstance(kind, int):
                    digest.update(value.to_bytes(kind, "big"))
                else:
                    if kind == "variable":
                        digest.update(len(value).to_bytes(8, "big"))
                    digest.update(value)
            count += 1
        tables[name] = {"rows": count, "sha256": digest.hexdigest()}
    state = {"identity": identity, "tables": tables}
    content = json.dumps(state, separators=(",", ":"), ensure_ascii=False).encode()
    domain = b"balance-history-baseline-state:v1"
    digest = hashlib.sha256(len(domain).to_bytes(4, "big") + domain + len(content).to_bytes(8, "big") + content)
    connection.close()
    return {"state": state, "logical_sha256": digest.hexdigest()}


if __name__ == "__main__":
    print(json.dumps(calculate(sys.argv[1])))
