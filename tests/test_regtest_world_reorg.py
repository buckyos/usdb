#!/usr/bin/env python3
"""Exercise world-sim replay with real Bitcoin Core and Ord in isolated datadirs.

Run with BITCOIN_BIN_DIR and ORD_BIN set to the same binaries as weekly CI.
"""

import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from urllib.request import urlopen

SCRIPTS = Path(__file__).resolve().parents[1] / "src/btc/usdb-indexer/scripts"
sys.path.insert(0, str(SCRIPTS))
from regtest_world_simulator import RegtestWorldSimulator  # noqa: E402


def unused_port():
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        return listener.getsockname()[1]


@unittest.skipUnless(
    os.environ.get("BITCOIN_BIN_DIR") and os.environ.get("ORD_BIN"),
    "set BITCOIN_BIN_DIR and ORD_BIN to run the live reorg regression",
)
class WorldReorgWalletTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="usdb-world-reorg-")
        self.root = Path(self.temp.name)
        self.addCleanup(self.temp.cleanup)
        self.btc_port = unused_port()
        self.ord_port = unused_port()
        self.btc_args = [
            "-regtest", f"-datadir={self.root}", f"-rpcport={self.btc_port}"
        ]
        # An explicit empty config and isolated datadir prevent reading host settings.
        (self.root / "bitcoin.conf").write_text("", encoding="utf-8")
        self.btc_args.append(f"-conf={self.root / 'bitcoin.conf'}")
        self.start_service("bitcoin", [
            str(Path(os.environ["BITCOIN_BIN_DIR"]) / "bitcoind"), *self.btc_args,
            "-server=1", "-listen=0", "-networkactive=0", "-dnsseed=0",
            "-discover=0", "-rpcbind=127.0.0.1", "-rpcallowip=127.0.0.1",
            "-fallbackfee=0.00001000", "-printtoconsole=1",
        ])
        self.wait_for(lambda: self.rpc("getblockcount") == 0)
        self.rpc("createwallet", "miner")
        self.mining_address = self.rpc("getnewaddress", wallet="miner")
        self.mine(110)
        self.ord_args = [
            os.environ["ORD_BIN"], "--regtest",
            "--bitcoin-rpc-url", f"http://127.0.0.1:{self.btc_port}",
            "--cookie-file", str(self.root / "regtest/.cookie"),
            "--bitcoin-data-dir", str(self.root),
            "--data-dir", str(self.root / "ord"),
        ]
        self.start_service("ord", [
            *self.ord_args, "--savepoint-interval", "10", "--max-savepoints", "5",
            "--index-addresses", "--index-transactions", "server",
            "--address", "127.0.0.1", "--http", "--http-port", str(self.ord_port),
            "--polling-interval", "200ms",
        ])
        self.wait_ord()
        self.ord("create")
        address = self.ord("receive")["addresses"][0]
        self.rpc("sendtoaddress", address, "4", wallet="miner")
        self.mine(1)
        self.wait_ord()
        self.sim = RegtestWorldSimulator.__new__(RegtestWorldSimulator)
        self.sim.args = SimpleNamespace(stable_lag_blocks=10, miner_wallet="miner")
        self.sim.run_btc_cli = self.run_btc_cli

    def start_service(self, name, command):
        log_path = self.root / f"{name}.log"
        log = log_path.open("w", encoding="utf-8")
        self.addCleanup(log.close)
        process = subprocess.Popen(command, stdout=log, stderr=subprocess.STDOUT)

        def stop():
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=30)

        self.addCleanup(stop)

    def run_btc_cli(self, wallet, args):
        command = [str(Path(os.environ["BITCOIN_BIN_DIR"]) / "bitcoin-cli"), *self.btc_args]
        if wallet:
            command.append(f"-rpcwallet={wallet}")
        result = subprocess.run(command + args, capture_output=True, text=True, timeout=20)
        if result.returncode:
            raise RuntimeError(f"bitcoin-cli {args[0]}: {result.stderr}")
        return result.stdout.strip()

    def rpc(self, method, *args, wallet=None):
        output = self.run_btc_cli(wallet, [method, *map(str, args)])
        try:
            return json.loads(output)
        except json.JSONDecodeError:
            return output

    def ord(self, *args):
        result = subprocess.run([
            *self.ord_args, "wallet", "--no-sync", "--name", "actor",
            "--server-url", f"http://127.0.0.1:{self.ord_port}", *args,
        ], capture_output=True, text=True, timeout=30)
        self.assertEqual(result.returncode, 0, result.stderr)
        return json.loads(result.stdout)

    def mine(self, count):
        return self.rpc("generatetoaddress", count, self.mining_address)

    def wait_for(self, predicate):
        deadline = time.monotonic() + 30
        last_error = None
        while time.monotonic() < deadline:
            try:
                if predicate():
                    return
            except (OSError, RuntimeError, ValueError) as error:
                last_error = error
            time.sleep(0.1)
        logs = "\n".join(path.read_text()[-6000:] for path in self.root.glob("*.log"))
        self.fail(f"service convergence timed out: {last_error}\n{logs}")

    def wait_ord(self):
        def synced():
            height = self.rpc("getblockcount")
            with urlopen(f"http://127.0.0.1:{self.ord_port}/blockcount", timeout=2) as response:
                count = int(response.read())
            if count != height + 1:
                return False
            with urlopen(f"http://127.0.0.1:{self.ord_port}/blockhash/{height}", timeout=2) as response:
                block_hash = response.read().decode().strip().strip('"')
            return block_hash == self.rpc("getblockhash", height)

        self.wait_for(synced)

    def check_replay(self, raw_depth):
        content = self.root / "inscription.txt"
        content.write_text("wallet-reorg-before", encoding="utf-8")
        before = self.ord("inscribe", "--fee-rate", "1", "--file", str(content))
        first_height = self.rpc("getblockcount") + 1
        self.mine(raw_depth)
        self.wait_ord()
        old_tip = self.rpc("getblockcount")
        blocks = self.sim.capture_reorg_blocks(first_height, old_tip)
        txids = [tx["txid"] for block in blocks for tx in block["transactions"]]
        self.assertEqual(set(txids), {before["commit"], before["reveal"]})
        self.rpc("invalidateblock", blocks[0]["hash"])
        self.assertEqual(old_tip - self.rpc("getblockcount"), raw_depth)
        mempool = self.rpc("getrawmempool")
        if raw_depth > 10:
            self.assertFalse(set(txids) & set(mempool), "fixture must cover evicted transactions")
        else:
            self.assertEqual(set(mempool), set(txids))
        # Exercise Core's raw disconnect boundary directly, including ten blocks.
        result = self.sim.mine_replacement_blocks(raw_depth - 10, blocks)
        self.assertEqual(result["replacement_replayed_tx_count"], 2)
        self.assertEqual(self.rpc("getblockcount"), old_tip)
        self.assertEqual(self.rpc("getrawmempool"), [])
        self.mine(1)
        self.wait_ord()
        for txid in txids:
            self.assertGreater(self.rpc("gettransaction", txid, wallet="actor")["confirmations"], 0)
        self.assertGreater(self.ord("balance")["cardinal"], 0)
        # A second real inscription proves that Ord can spend restored change.
        content.write_text("wallet-reorg-after", encoding="utf-8")
        after = self.ord("inscribe", "--fee-rate", "1", "--file", str(content))
        self.mine(1)
        self.wait_ord()
        self.assertGreater(self.rpc("gettransaction", after["reveal"], wallet="actor")["confirmations"], 0)
        print(f"raw_depth={raw_depth}: replayed commit/reveal and confirmed subsequent inscription", flush=True)

    def test_ten_disconnected_blocks(self):
        self.check_replay(10)

    def test_eleven_disconnected_blocks(self):
        self.check_replay(11)

    def test_thirteen_disconnected_blocks(self):
        self.check_replay(13)


if __name__ == "__main__":
    unittest.main()
