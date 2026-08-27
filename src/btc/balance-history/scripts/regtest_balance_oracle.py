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
        "seen": {address: False for address in addresses},
        "balances": {address: 0 for address in addresses},
        "history": {address: {str(args.start_height): 0} for address in addresses},
        "deltas": {address: {} for address in addresses},
        "utxos": {},
        "spent_utxos": {},
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
    spent_utxos = state["spent_utxos"]
    balances_before = dict(balances)
    touched = set()

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
                touched.add(tracked_utxo["address"])
                spent_utxos[outpoint] = tracked_utxo

        txid = tx["txid"]
        for vout in tx.get("vout", []):
            value_sat = amount_to_sat(vout["value"])
            for address in extract_addresses(vout.get("scriptPubKey", {})):
                if tracked_set.get(address):
                    balances[address] += value_sat
                    touched.add(address)
                    state["seen"][address] = True
                    outpoint = f"{txid}:{vout['n']}"
                    spent_utxos.pop(outpoint, None)
                    utxos[outpoint] = {
                        "address": address,
                        "value": value_sat,
                    }
                    break

    for address, balance in balances.items():
        state["history"][address][str(height)] = balance
        if address in touched:
            state["deltas"][address][str(height)] = balance - balances_before[address]

    state["current_height"] = height
    save_state(state_path, state)
    return 0


def balance_at_height(state: dict, address: str, height: int) -> int:
    history = state["history"][address]
    latest = 0
    for height_str in sorted(history.keys(), key=lambda item: int(item)):
        if int(height_str) > height:
            break
        latest = history[height_str]
    return latest


def cmd_get_balance(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    print(balance_at_height(state, args.address, args.height))
    return 0


def cmd_get_current_height(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    print(state["current_height"])
    return 0


def cmd_get_delta(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    delta = state["deltas"][args.address].get(str(args.height))
    balance = state["history"][args.address].get(str(args.height), 0)
    print(
        json.dumps(
            {
                "present": delta is not None,
                "delta": delta,
                "balance": balance,
            },
            sort_keys=True,
        )
    )
    return 0


def history_rows(state: dict, address: str, start: int, end: int) -> list[dict]:
    rows = []
    for height_str, delta in state["deltas"][address].items():
        height = int(height_str)
        if start <= height < end:
            rows.append(
                {
                    "block_height": height,
                    "delta": delta,
                    "balance": balance_at_height(state, address, height),
                }
            )
    rows.sort(key=lambda row: row["block_height"])
    return rows


def cmd_get_history(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    print(
        json.dumps(
            history_rows(state, args.address, args.start_height, args.end_height),
            sort_keys=True,
        )
    )
    return 0


def cmd_get_summary(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    rows = history_rows(state, args.address, args.start_height, args.end_height)
    start_balance = balance_at_height(state, args.address, args.start_height - 1)
    summary = {
        "range_start": args.start_height,
        "range_end": args.end_height,
        "start_balance": start_balance,
        "end_balance": balance_at_height(state, args.address, args.end_height - 1),
        "change_count": len(rows),
        "total_inflow": sum(row["delta"] for row in rows if row["delta"] >= 0),
        "total_outflow": sum(-row["delta"] for row in rows if row["delta"] < 0),
        "net_delta": sum(row["delta"] for row in rows),
        "first_movement_height": rows[0]["block_height"] if rows else None,
        "latest_movement_height": rows[-1]["block_height"] if rows else None,
        "peak_balance": start_balance,
        "peak_height": args.start_height,
        "low_balance": start_balance,
        "low_height": args.start_height,
    }
    for row in rows:
        if row["balance"] > summary["peak_balance"]:
            summary["peak_balance"] = row["balance"]
            summary["peak_height"] = row["block_height"]
        if row["balance"] < summary["low_balance"]:
            summary["low_balance"] = row["balance"]
            summary["low_height"] = row["block_height"]
    print(json.dumps(summary, sort_keys=True))
    return 0


def cmd_dump_utxos(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    source = state["utxos"] if args.state == "live" else state["spent_utxos"]
    rows = [
        {"outpoint": outpoint, **entry}
        for outpoint, entry in sorted(source.items())
    ]
    print(json.dumps(rows, sort_keys=True))
    return 0


def cmd_is_seen(args: argparse.Namespace) -> int:
    state = load_state(Path(args.state_file))
    print("true" if state["seen"][args.address] else "false")
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

    height_parser = subparsers.add_parser("get-current-height")
    height_parser.add_argument("--state-file", required=True)
    height_parser.set_defaults(func=cmd_get_current_height)

    delta_parser = subparsers.add_parser("get-delta")
    delta_parser.add_argument("--state-file", required=True)
    delta_parser.add_argument("--address", required=True)
    delta_parser.add_argument("--height", type=int, required=True)
    delta_parser.set_defaults(func=cmd_get_delta)

    history_parser = subparsers.add_parser("get-history")
    history_parser.add_argument("--state-file", required=True)
    history_parser.add_argument("--address", required=True)
    history_parser.add_argument("--start-height", type=int, required=True)
    history_parser.add_argument("--end-height", type=int, required=True)
    history_parser.set_defaults(func=cmd_get_history)

    summary_parser = subparsers.add_parser("get-summary")
    summary_parser.add_argument("--state-file", required=True)
    summary_parser.add_argument("--address", required=True)
    summary_parser.add_argument("--start-height", type=int, required=True)
    summary_parser.add_argument("--end-height", type=int, required=True)
    summary_parser.set_defaults(func=cmd_get_summary)

    utxo_parser = subparsers.add_parser("dump-utxos")
    utxo_parser.add_argument("--state-file", required=True)
    utxo_parser.add_argument("--state", choices=["live", "spent"], required=True)
    utxo_parser.set_defaults(func=cmd_dump_utxos)

    seen_parser = subparsers.add_parser("is-seen")
    seen_parser.add_argument("--state-file", required=True)
    seen_parser.add_argument("--address", required=True)
    seen_parser.set_defaults(func=cmd_is_seen)

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
