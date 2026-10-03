#!/usr/bin/env python3
"""Isolated real Core/Ord/indexer/control-plane V2 opening, remint and cold-address migration."""
import argparse
from http.cookiejar import CookieJar
import json
from pathlib import Path
import subprocess
import tempfile
import time
from urllib.request import Request, build_opener, HTTPCookieProcessor, urlopen
from urllib.error import HTTPError

from common.assumeutxo_services import Processes, Rpc, free_port
from common.miner_pass_regtest import configure_indexer

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bitcoind", required=True, type=Path)
    parser.add_argument("--ord", required=True, type=Path)
    parser.add_argument("--binary-dir", type=Path, default=ROOT / "src/btc/target/debug")
    args = parser.parse_args()
    if not __debug__:
        parser.error("Run without Python optimization; assertions are acceptance gates")
    root = Path(tempfile.mkdtemp(prefix="usdb-control-mint-v2-"))
    processes = Processes(root)
    ports = {name: free_port() for name in ("core", "ord", "bh", "indexer", "console")}
    for name in ports:
        (root / name).mkdir()
    btc = Rpc(ports["core"], root / "core/regtest/.cookie")
    bh, indexer = Rpc(ports["bh"]), Rpc(ports["indexer"])
    ord_url = f'http://127.0.0.1:{ports["ord"]}'
    origin = f'http://127.0.0.1:{ports["console"]}'
    stages = []
    print(f"Control-plane V2 live run: {root}", flush=True)

    def record(name, **details):
        stages.append(dict(name=name, **details))
        (root / "result.json").write_text(json.dumps(stages, indent=2) + "\n")
        print(json.dumps(stages[-1]), flush=True)

    def table(name, values):
        return f"[{name}]\n" + "\n".join(f"{key} = {json.dumps(value)}" for key, value in values.items()) + "\n"

    def http(path):
        with urlopen(ord_url + path, timeout=5) as response:
            return response.read().decode().strip().strip('"')

    def ord_wallet(name, *arguments):
        command = [str(args.ord), "--chain", "regtest", "--data-dir", str(root / "ord"),
                   "--bitcoin-rpc-url", btc.url, "--cookie-file", str(btc.cookie), "--format", "json",
                   "wallet", "--server-url", ord_url, "--no-sync", "--name", name, *arguments]
        return json.loads(subprocess.check_output(command, stderr=subprocess.PIPE, timeout=90))

    def ready():
        height = btc("getblockcount")
        if int(http("/blockcount")) != height + 1 or http(f"/blockhash/{height}") != btc("getblockhash", height):
            return False
        for rpc, field in ((bh, "stable_height"), (indexer, "synced_block_height")):
            state = rpc("get_readiness")
            if not state["consensus_ready"] or state[field] != height - 10:
                return False
        return True

    try:
        processes.start("core", [args.bitcoind, "-regtest", f"-datadir={root / 'core'}", "-server=1", "-txindex=1",
                                 "-listen=0", "-connect=0", "-dnsseed=0", "-discover=0", "-fallbackfee=0.0001",
                                 "-dbcache=64", f'-rpcport={ports["core"]}'])
        processes.wait(lambda: btc("getnetworkinfo"), "Bitcoin RPC")
        btc("createwallet", "funding")
        funding = Rpc(ports["core"], btc.cookie)
        funding.url += "/wallet/funding"
        miner = funding("getnewaddress", "", "bech32")
        btc("generatetoaddress", 130, miner)
        processes.start("ord", [args.ord, "--chain", "regtest", "--data-dir", root / "ord", "--bitcoin-rpc-url", btc.url,
                                "--cookie-file", btc.cookie, "--index-addresses", "--index-transactions", "server",
                                "--address", "127.0.0.1", "--http", "--http-port", ports["ord"], "--polling-interval", "1s"])
        processes.wait(lambda: int(http("/blockcount")) == 131, "Ord initial index")
        for name in ("ord-a", "ord-b"):
            ord_wallet(name, "create")
        source = ord_wallet("ord-a", "receive")["addresses"][0]
        owner = ord_wallet("ord-b", "receive")["addresses"][0]
        cold = funding("getnewaddress", "", "bech32")
        funding("sendtoaddress", source, 0.02)
        btc("generatetoaddress", 12, miner)

        config = table("btc", dict(network="regtest", data_dir=str(root / "core/regtest"), rpc_url=btc.url))
        config += "[ordinals]\n[electrs]\n"
        config += table("sync", dict(local_loader_threshold=100000000, batch_size=32, utxo_max_cache_bytes=4194304, balance_max_cache_bytes=4194304))
        config += table("rpc_server", dict(port=ports["bh"]))
        (root / "bh/config.toml").write_text(config)
        (root / "indexer/config.json").write_text(json.dumps(dict(
            bitcoin=dict(network="regtest", rpc_url=btc.url, auth={"CookieFile": str(btc.cookie)}),
            ordinals=dict(rpc_url=ord_url), balance_history=dict(rpc_url=bh.url),
            usdb=dict(genesis_block_height=1, inscription_source="bitcoind", rpc_server_host="127.0.0.1",
                      rpc_server_port=ports["indexer"], upstream_poll_interval_ms=100))))
        configure_indexer(root / "indexer/config.json")
        for name, binary in (("bh", "balance-history"), ("indexer", "usdb-indexer")):
            processes.start(name, [args.binary_dir / binary, "--root-dir", root / name, "--skip-process-lock"])
        processes.wait(ready, "Bitcoin-side services", timeout=120)
        (root / "console/world-sim.json").write_text(json.dumps(dict(agent_wallets=["ord-a", "ord-b"], agent_addresses=[source, owner])))
        config = f'root_dir = {json.dumps(str(root / "console"))}\n'
        config += table("server", dict(host="127.0.0.1", port=ports["console"]))
        config += table("bitcoin", dict(url=btc.url, auth_mode="cookie", cookie_file=str(btc.cookie)))
        config += table("rpc", dict(balance_history_url=bh.url, usdb_indexer_url=indexer.url, ord_url=ord_url, usdb_chain_url="http://127.0.0.1:1"))
        config += table("bootstrap", dict(world_sim_bootstrap_marker=str(root / "console/world-sim.json")))
        config += table("development_mint", dict(enabled=True, ord_bin=str(args.ord.resolve()), ord_data_dir=str(root / "ord"), ord_fee_rate=1.0))
        config += table("web", dict(console_root=str(ROOT / "web/usdb-console-app/dist"),
                                     balance_history_explorer_root=str(ROOT / "web/balance-history-browser/dist"),
                                     usdb_indexer_explorer_root=str(ROOT / "web/usdb-indexer-browser/dist")))
        (root / "console/config.toml").write_text(config)
        processes.start("console", [args.binary_dir / "usdb-control-plane", "--root-dir", root / "console", "--skip-process-lock"])
        processes.wait(lambda: urlopen(origin + "/healthz", timeout=2).status == 200, "Control-plane API")
        opener = build_opener(HTTPCookieProcessor(CookieJar()))

        def api(path, payload):
            request = Request(origin + path, json.dumps(payload).encode(), {"Content-Type": "application/json", "Origin": origin})
            try:
                with opener.open(request, timeout=90) as response:
                    return json.load(response)
            except HTTPError as error:
                raise RuntimeError(f"{path}: HTTP {error.code}: {error.read().decode()}") from error

        api("/api/auth/login", dict(token=(root / "console/access-token").read_text().strip()))
        beneficiary = "0x" + "1" * 40
        previous = []
        protected = {}
        last = None
        for path, wallet, sender, receiver in (("first_opening", "ord-a", source, owner),
                                               ("same_owner", "ord-b", owner, owner),
                                               ("cross_owner", "ord-b", owner, cold)):
            if path != "first_opening":
                funding("sendtoaddress", owner, 0.02)
                btc("generatetoaddress", 12, miner)
                processes.wait(ready, "Funding confirmation")
            mint = dict(source_address=sender, recipient_address=receiver, usdb_main=beneficiary, prev=previous)
            # Service summaries are cached briefly; the execute handler also checks fresh chain evidence.
            prepared = processes.wait(lambda: (r if (r := api("/api/btc/mint/prepare", mint))["execution_available"] else None), "Mint capability")
            assert prepared["eligible"], prepared
            assert prepared["operation_path"] == path, prepared
            result = api("/api/btc/mint/execute", dict(**mint, wallet_name=wallet))
            record(path + "_broadcast", inscription_id=result["inscription_id"], source_outpoint=result["source_outpoint"])
            verify = dict(mint=mint, inscription_id=result["inscription_id"], expected_source_outpoint=result["source_outpoint"])
            assert not api("/api/btc/mint/verify", verify)["verified"]
            btc("generatetoaddress", 12, miner)
            processes.wait(ready, "Reveal confirmation")
            checked = api("/api/btc/mint/verify", verify)
            assert checked["verified"], checked
            audit = indexer("get_pass_mint_audit", dict(inscription_id=result["inscription_id"]))
            assert audit["audit"]["operation_path"] == path
            if path == "first_opening":
                assert audit["audit"]["source"] is None
            assert checked["source"]["source_outpoint"] == result["source_outpoint"]
            for pass_id in previous:
                assert indexer("get_pass_snapshot", dict(inscription_id=pass_id))["state"] == "consumed"
            positions = {item["inscription"]: item["location"] for item in ord_wallet("ord-b", "inscriptions")}
            for pass_id, satpoint in protected.items():
                assert positions[pass_id] == satpoint, (pass_id, satpoint, positions)
            protected.update(positions)
            # A changed expectation never inherits the successful verification result.
            wrong = dict(verify, mint=dict(mint, usdb_main="0x" + "2" * 40))
            assert not api("/api/btc/mint/verify", wrong)["verified"]
            record(path + "_verified", confirmations=checked["confirmations"], protected_inscriptions=len(protected), recipient=receiver)
            previous = [result["inscription_id"]]
            last = verify
        # Once occupied, E cannot be silently re-opened by A.
        forged = dict(source_address=source, recipient_address=cold, usdb_main=beneficiary, prev=[])
        assert not api("/api/btc/mint/prepare", forged)["eligible"]
        funding("sendtoaddress", cold, 0.01)
        btc("generatetoaddress", 12, miner)
        processes.wait(ready, "Cold address funding")
        assert api("/api/btc/mint/verify", last)["verified"]
        record("passed", scope="Real authenticated API, Core, Ord and V2 indexer; no production wallet or network deployment")
    finally:
        # Only stop children created in this private temporary directory; retain evidence.
        for name in list(processes.children)[::-1]:
            if name in ("console", "ord", "indexer", "bh"):
                process = processes.children.pop(name)
                process.terminate()
                try:
                    process.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    process.kill(); process.wait(timeout=5)
            else:
                processes.stop(name)


if __name__ == "__main__":
    main()
