#!/usr/bin/env python3
"""Run V2 mint transitions, real Geth mining, and an independent validator on fresh regtest data."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
PORTS = (
    "BTC_RPC_PORT", "BTC_P2P_PORT", "BH_RPC_PORT", "USDB_INDEXER_RPC_PORT",
    "ORD_RPC_PORT", "HTTP_PORT", "P2P_PORT", "AUTHRPC_PORT",
    "VALIDATOR_HTTP_PORT", "VALIDATOR_P2P_PORT", "VALIDATOR_AUTHRPC_PORT",
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ord-bin", type=Path, default=os.environ.get("ORD_BIN"), required=not os.environ.get("ORD_BIN"))
    parser.add_argument("--bitcoin-bin-dir", type=Path,
                        default=os.environ.get("BITCOIN_BIN_DIR", "/home/bucky/btc/bitcoin-28.1/bin"))
    parser.add_argument("--geth-repo", type=Path, default=ROOT.parent / "go-ethereum")
    parser.add_argument("--geth-bin", type=Path, help="Use an explicitly prebuilt candidate; otherwise build current source")
    parser.add_argument("--work-dir", type=Path, help="Must be empty; defaults to a new temporary directory")
    args = parser.parse_args()
    for binary in (args.ord_bin, args.bitcoin_bin_dir / "bitcoind", args.bitcoin_bin_dir / "bitcoin-cli"):
        if not binary.is_file() or not os.access(binary, os.X_OK):
            parser.error(f"Missing executable: {binary}")
    work = args.work_dir.resolve() if args.work_dir else Path(tempfile.mkdtemp(prefix="usdb-miner-pass-v2-profile-"))
    if work.exists() and any(work.iterdir()):
        parser.error(f"Refusing to reuse nonempty test directory: {work}")
    work.mkdir(parents=True, exist_ok=True)
    env = os.environ.copy()
    # Override every service data path, so inherited shell settings cannot target an existing node.
    paths = {
        "WORK_DIR": work, "USDB_CHAIN_WORK_DIR": work / "geth",
        "DATADIR": work / "geth/datadir", "VALIDATOR_DATADIR": work / "geth/validator",
        "GENESIS_JSON": work / "geth/genesis.json", "GETH_LOG_FILE": work / "geth/geth.log",
        "VALIDATOR_LOG_FILE": work / "geth/validator.log", "BITCOIN_DIR": work / "usdb/bitcoin",
        "ORD_DATA_DIR": work / "usdb/ord", "BALANCE_HISTORY_ROOT": work / "usdb/balance-history",
        "USDB_INDEXER_ROOT": work / "usdb/usdb-indexer",
        "BALANCE_HISTORY_LOG_FILE": work / "usdb/balance-history.log",
        "USDB_INDEXER_LOG_FILE": work / "usdb/usdb-indexer.log",
        "ORD_SERVER_LOG_FILE": work / "usdb/ord-server.log",
    }
    env.update({key: str(value) for key, value in paths.items()})
    env.update(USDB_REPO_DIR=str(ROOT), ORD_BIN=str(args.ord_bin.resolve()),
               BITCOIN_BIN_DIR=str(args.bitcoin_bin_dir.resolve()),
               MINER_PASS_V2_TRANSITIONS="1", INSCRIPTION_SOURCE="bitcoind", INSCRIPTION_FIXTURE_FILE="",
               BTC_STABLE_LAG_BLOCKS="10", TARGET_BLOCKS="2", MINER_LIVE_STATE_CHECK="0",
               INDEXER_OUTAGE_CHECK="0", SELECTOR_TAMPER_CHECK="0", ACTIVATION_FRESH_VALIDATOR_CHECK="0",
               ANCHOR_BOUNDARY_CHECK="0", ACTIVATION_CONFORMANCE_BLOCK="", ECONOMIC_CONFORMANCE_V2_BLOCK="",
               ECONOMIC_CONFORMANCE_V3_BLOCK="", POW_CALIBRATION_PROFILE="",
               PRE_ACTIVATION_GETH_BIN="", MID_ACTIVATION_GETH_BIN="", POST_ACTIVATION_GETH_BIN="",
               HTTP_ADDR="127.0.0.1", VALIDATOR_HTTP_ADDR="127.0.0.1",
               USDB_INDEXER_INJECT_REORG_RECOVERY_ENERGY_FAILURES="",
               USDB_INDEXER_INJECT_REORG_RECOVERY_TRANSFER_RELOAD_FAILURES="")
    env["GETH_BIN"] = str(args.geth_bin.resolve()) if args.geth_bin else ""
    reservations = []
    for name in PORTS:
        reservation = socket.socket()
        reservation.bind(("127.0.0.1", 0))
        env[name] = str(reservation.getsockname()[1])
        reservations.append(reservation)
    manifest = {"ports": {name: env[name] for name in PORTS}, "status": "started"}
    for label, repo in (("usdb", ROOT), ("geth", args.geth_repo)):
        manifest[label] = {
            "head": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip(),
            "status": subprocess.check_output(["git", "status", "--short"], cwd=repo, text=True),
        }
    (work / "run.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(f"MinerPass V2 live run: {work}", flush=True)
    exit_code = 1
    try:
        with (work / "build.log").open("w") as log:
            # Separate package builds preserve the runtime feature sets used by cargo run.
            for package in ("balance-history", "usdb-indexer"):
                subprocess.run(["cargo", "build", "--manifest-path", "src/btc/Cargo.toml", "-p", package],
                               cwd=ROOT, env=env, stdout=log, stderr=subprocess.STDOUT, check=True)
        for reservation in reservations:
            reservation.close()
        with (work / "runner.log").open("w") as log:
            result = subprocess.run(["bash", str(args.geth_repo / "scripts/usdb/run_usdb_profile_e2e.sh")],
                                    cwd=args.geth_repo, env=env, stdout=log, stderr=subprocess.STDOUT)
        exit_code = result.returncode
    finally:
        for reservation in reservations:
            reservation.close()
        geth = args.geth_bin or work / "bin/geth"
        if geth.is_file():
            manifest["geth_binary_sha256"] = hashlib.sha256(geth.read_bytes()).hexdigest()
        manifest.update(status="passed" if exit_code == 0 else "failed", exit_code=exit_code)
        (work / "run.json").write_text(json.dumps(manifest, indent=2) + "\n")
        print(f"status={manifest['status']}, logs={work}", flush=True)
    raise SystemExit(exit_code)



if __name__ == "__main__":
    main()
