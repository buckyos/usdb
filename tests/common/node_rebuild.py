"""Disposable node filesystem for destructive-operation tests; no real host data."""

import json
from pathlib import Path


class RebuildFixture:
    def __init__(self, root: Path):
        self.home = root / "operator"
        self.data = root / "data/.usdb"
        self.backup = root / "reference"
        self.units = root / "units"
        self.bundle = "usdb-testnet-v0"
        self.config = self.home / ".config/usdb" / self.bundle
        self.config.mkdir(parents=True)
        self.paths = dict(BTC_NODE_DATA_HOST_DIR=self.data / "datasets/bitcoin/btc-mainnet",
            BH_DATA_HOST_DIR=self.data / "datasets/balance-history/btc-mainnet" / ("a" * 64),
            USDB_INDEXER_DATA_HOST_DIR=self.data / "datasets/usdb-indexer" / ("b" * 64),
            USDB_CHAIN_DATA_HOST_DIR=self.data / "networks" / self.bundle / "usdb-chain",
            CONTROL_PLANE_DATA_HOST_DIR=self.data / "networks" / self.bundle / "control-plane",
            BH_SNAPSHOT_HOST_DIR=self.data / "artifacts/balance-history")
        env = dict(USDB_DATA_LAYOUT="usdb-node-data-layout:v2", USDB_DATA_ROOT=self.data,
                   BTC_RPC_PASSWORD="private-test-value", **self.paths)
        self.env = self.config / "node.env"
        self.env.write_text("".join(f"{key}={value}\n" for key, value in env.items()))
        self.env.chmod(0o600)
        (self.config / ".usdb-node-operation.lock").write_text("")
        for path in self.paths.values():
            path.mkdir(parents=True)
        bh = self.paths["BH_DATA_HOST_DIR"]
        for name, value in (("db/balance_history/000001.sst", b"coins" * 1000),
                            ("db/balance_history/LOCK", b""),
                            ("auxiliary/registry/000001.sst", b"scripts"), ("config.toml", b"private=1")):
            path = bh / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(value)
        (self.paths["BTC_NODE_DATA_HOST_DIR"] / "blocks").mkdir()
        (self.paths["BTC_NODE_DATA_HOST_DIR"] / "blocks/blk00000.dat").write_bytes(b"discardable blocks")
        (self.paths["BTC_NODE_DATA_HOST_DIR"] / "wallet.dat").write_bytes(b"private wallet")
        chain = self.paths["USDB_CHAIN_DATA_HOST_DIR"]
        (chain / "keystore").mkdir()
        (chain / "keystore/key").write_bytes(b"private chain key")
        (chain / "geth/chaindata").mkdir(parents=True)
        (chain / "geth/chaindata/table").write_bytes(b"discardable chain data")
        self.release = self.home / ".local/share/usdb/releases" / (self.bundle + "-r17")
        (self.release / "release").mkdir(parents=True)
        (self.release / "release/usdb-release-manifest.json").write_text(json.dumps(dict(release_id=self.release.name)))
        tool = self.release / "docker/scripts/tools/usdb_node.py"
        tool.parent.mkdir(parents=True)
        tool.write_text("# old reader\n")
        self.launcher = self.home / ".local/bin/usdb-node"
        self.launcher.parent.mkdir(parents=True)
        self.launcher.symlink_to(tool)

    def plan(self, tool):
        return tool.build_plan(self.home, self.bundle, self.backup, self.units)

    def session(self, tool, mode="copy"):
        self.backup.mkdir(mode=0o700, exist_ok=True)
        return tool.Session(self.plan(tool), self.backup, mode)
