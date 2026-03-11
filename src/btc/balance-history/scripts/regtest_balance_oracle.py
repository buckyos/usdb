#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
from decimal import Decimal, ROUND_DOWN
from pathlib import Path


SAT_SCALE = Decimal("100000000")


def amount_to_sat(value: object) -> int:
    return int((Decimal(str(value)) * SAT_SCALE).to_integral_value(rounding=ROUND_DOWN))


def load_state(path: Path) -> dict:
    return json.loads(path.read_text())


def save_state(path: Path, state: dict) -> None:
    path.write_text(json.dumps(state, sort_keys=True))


def cmd_init(args: argparse.Namespace) -> int:
    addresses = json.loads(args.addresses_json)
    state = {
        "start_height": args.start_height,
        "current_height": args.start_height,
        "tracked_addresses": addresses,
        "tracked_set": {address: True for address in addresses},
        "balances": {address: 0 for address in addresses},
        "history": {address: {str(args.start_height): 0} for address in addresses},
        "utxos": {},
    }
    save_state(Path(args.state_file), state)
    return 0


def extract_addresses(script_pub_key: dict) -> list[str]:
    addresses = []
    address = script_pub_key.get("address")
    if address:
        addresses.append(address)
    addresses.extend(script_pub_key.get("addresses") or [])
    return addresses


def cmd_apply_block(args: argparse.Namespace) -> int:
    state_path = Path(args.state_file)
    state = load_state(state_path)
    block = json.load(args.block_json)
    height = block["height"]

    expected_next = state["current_height"] + 1
    if height != expected_next:
        raise SystemExit(
            f"oracle block height mismatch: expected {expected_next}, got {height}"
        )

    tracked_set = state["tracked_set"]
    balances = state["balances"]
    utxos = state["utxos"]

    for tx in block.get("tx", []):
        for vin in tx.get("vin", []):
            prev_txid = vin.get("txid")
            prev_vout = vin.get("vout")
            if prev_txid is None or prev_vout is None:
                continue
            outpoint = f"{prev_txid}:{prev_vout}"
            tracked_utxo = utxos.pop(outpoint, None)
            if tracked_utxo is not None:
                balances[tracked_utxo["address"]] -= tracked_utxo["value"]

        txid = tx["txid"]
        for vout in tx.get("vout", []):
            value_sat = amount_to_sat(vout["value"])
            for address in extract_addresses(vout.get("scriptPubKey", {})):
                if tracked_set.get(address):
                    balances[address] += value_sat
                    utxos[f"{txid}:{vout['n']}"] = {
                        "address": address,
                        "value": value_sat,
                    }
                    break

    for address, balance in balances.items():
        state["history"][address][str(height)] = balance

    state["current_height"] = height
    save_state(state_path, state)
    return 0


def cmd_get_balance(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    history = state["history"][args.address]
    latest = 0
    for height_str in sorted(history.keys(), key=lambda item: int(item)):
        if int(height_str) > args.height:
            break
        latest = history[height_str]
    print(latest)
    return 0


def cmd_dump_addresses(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    print(json.dumps(state["tracked_addresses"]))
    return 0


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser()
    subparsers = parser.add_subparsers(dest="command", required=True)

    init_parser = subparsers.add_parser("init")
    init_parser.add_argument("--state-file", required=True)
    init_parser.add_argument("--start-height", type=int, required=True)
    init_parser.add_argument("--addresses-json", required=True)
    init_parser.set_defaults(func=cmd_init)

    apply_parser = subparsers.add_parser("apply-block")
    apply_parser.add_argument("--state-file", required=True)
    apply_parser.add_argument("block_json", type=argparse.FileType("r"), nargs="?", default="-")
    apply_parser.set_defaults(func=cmd_apply_block)

    balance_parser = subparsers.add_parser("get-balance")
    balance_parser.add_argument("--state-file", required=True)
    balance_parser.add_argument("--address", required=True)
    balance_parser.add_argument("--height", type=int, required=True)
    balance_parser.set_defaults(func=cmd_get_balance)

    dump_parser = subparsers.add_parser("dump-addresses")
    dump_parser.add_argument("--state-file", required=True)
    dump_parser.set_defaults(func=cmd_dump_addresses)

    return parser


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    return args.func(args)


if __name__ == "__main__":
    raise SystemExit(main())