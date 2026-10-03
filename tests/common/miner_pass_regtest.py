"""Explicit V2 configuration and cardinal-coin selection for isolated regtest runs."""

import argparse
from decimal import Decimal
import json
from pathlib import Path
import shutil

REPO_ROOT = Path(__file__).resolve().parents[2]
CATALOG = REPO_ROOT / "tests/fixtures/miner-pass-v2/catalog.json"
REGISTRY_ID = "747b656a814bf8d57409c19aa8df9754a1d46aadbe2ebb6fc09805ca14637014"
RULES_SCOPE = "miner-pass-v2-fixture"


def configure_indexer(path):
    """Pin only a generated regtest config; leave embedded and deployment defaults alone."""
    data = json.loads(path.read_text())
    if data["bitcoin"]["network"] != "regtest":
        raise ValueError(f"V2 test catalog requires regtest: config={path}")
    catalog = json.loads(CATALOG.read_text())
    if catalog["current_registry_id"] != REGISTRY_ID:
        raise ValueError(f"V2 test catalog identity changed: catalog={CATALOG}")
    catalog_name = "miner-pass-v2-catalog.json"
    data["usdb"].update(
        rules_scope=RULES_SCOPE,
        activation_registry_id=REGISTRY_ID,
        activation_registry_catalog_file=catalog_name,
    )
    shutil.copyfile(CATALOG, path.parent / catalog_name)
    path.write_text(json.dumps(data, indent=2) + "\n")


def configure_genesis(path):
    """Select the same catalog in a newly generated single-checkpoint regtest genesis."""
    data = json.loads(path.read_text())
    config = data["config"]["usdb"]
    if config["btcNetworkId"] != "btc-regtest" or config["btcIndexOriginHeight"] != 1:
        raise ValueError(f"Expected isolated regtest origin 1: genesis={path}")
    checkpoints = config["activations"]
    if len(checkpoints) != 1 or checkpoints[0]["block"] != 0:
        raise ValueError(f"Expected a fresh single-checkpoint genesis: genesis={path}")
    checkpoints[0]["btcActivationRegistryId"] = REGISTRY_ID
    path.write_text(json.dumps(data, indent=2) + "\n")


def select_satpoint(unspent, inscriptions, address):
    """Choose a confirmed spendable coin of D, excluding every inscription-bearing output."""
    protected = {item["location"].rsplit(":", 1)[0] for item in inscriptions}
    candidates = []
    for coin in unspent:
        outpoint = f'{coin["txid"]}:{coin["vout"]}'
        sats = int(Decimal(str(coin["amount"])) * 100_000_000)
        if (
            coin.get("address") == address
            and coin.get("spendable") is True
            and coin.get("safe") is True
            and coin.get("confirmations", 0) > 0
            and outpoint not in protected
            and sats >= 100_000
        ):
            candidates.append((-sats, outpoint))
    if not candidates:
        raise ValueError(
            f"No confirmed cardinal funding coin: address={address}, "
            f"utxos={len(unspent)}, protected_outputs={len(protected)}, min_sats=100000"
        )
    return min(candidates)[1] + ":0"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for command in ("configure-indexer", "configure-genesis"):
        sub.add_parser(command).add_argument("path", type=Path)
    select = sub.add_parser("select-satpoint")
    select.add_argument("--unspent", type=Path, required=True)
    select.add_argument("--inscriptions", type=Path, required=True)
    select.add_argument("--address", required=True)
    args = parser.parse_args()
    if args.command == "configure-indexer":
        configure_indexer(args.path)
    elif args.command == "configure-genesis":
        configure_genesis(args.path)
    else:
        print(select_satpoint(
            json.loads(args.unspent.read_text()),
            json.loads(args.inscriptions.read_text()), args.address,
        ))


if __name__ == "__main__":
    main()
