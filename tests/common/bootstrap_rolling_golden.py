#!/usr/bin/env python3
"""Print independent SHA-256 vectors for rolling v1 from a reviewed snapshot checkpoint."""

import hashlib
import json
from pathlib import Path
import struct


def main():
    fixtures = Path(__file__).resolve().parents[1] / "fixtures"
    checkpoint = json.loads((fixtures / "assumeutxo-p5/checkpoint.json").read_text())
    chain = json.loads((fixtures / "assumeutxo-p5/downstream-inputs.json").read_text())
    previous = bytes.fromhex(checkpoint["block_commit"])
    vectors = []
    for row in chain["blocks"][1:]:
        block = row["full_replay"]["block_commit"]
        height = block["block_height"]
        block_hash = bytes.fromhex(block["btc_block_hash"])[::-1]
        delta = bytes.fromhex(block["balance_delta_root"])
        encoded = b"balance-history:block-commit:v1" + struct.pack(">I", height)
        encoded += block_hash + delta + previous
        commit = hashlib.sha256(encoded).digest()
        assert commit.hex() == block["block_commit"]
        vectors.append(dict(height=height, block_hash=block_hash[::-1].hex(), delta_root=delta.hex(),
                            previous_commit=previous.hex(), encoded_hex=encoded.hex(), commit=commit.hex()))
        previous = commit
    print(json.dumps(vectors, indent=2))


if __name__ == "__main__":
    main()
