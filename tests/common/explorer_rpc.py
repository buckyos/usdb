"""Canonical RPC samples for public explorer capability checks."""
from copy import deepcopy


class ExplorerRpcFixture:
    def __init__(self):
        self.identity = {"chain_id": 202608250, "network_id": 202608250, "genesis_block_hash": "0x" + "aa" * 32}
        self.transaction = "0x" + "bb" * 32
        self.head = {"number": "0x12c", "hash": "0x" + "cc" * 32, "miner": "0x" + "11" * 20,
                     "extraData": "0x" + "22" * 111, "transactions": []}
        self.tx_block = {"number": "0xa", "hash": "0x" + "dd" * 32, "transactions": [self.transaction]}
        self.calls = []
        self.failures = {}
        self.reorg = False

    def __call__(self, method, params):
        self.calls.append((method, params))
        if method in self.failures:
            failure = self.failures[method]
            if isinstance(failure, Exception):
                raise failure
            return deepcopy(failure)
        if method == "eth_getBlockByNumber":
            if params[0] == "0x0":
                return {"hash": self.identity["genesis_block_hash"]}
            if params[0] == "0xa":
                return deepcopy(self.tx_block)
            result = deepcopy(self.head)
            if self.reorg and params[0] != "latest":
                result["hash"] = "0x" + "ee" * 32
            return result
        values = {"eth_chainId": hex(self.identity["chain_id"]), "net_version": str(self.identity["network_id"]),
                  "eth_syncing": False, "eth_blockNumber": self.head["number"], "web3_clientVersion": "Geth/fixture",
                  "rpc_modules": {"eth": "1.0", "admin": "1.0"}, "eth_getBlockByHash": self.head,
                  "eth_getBalance": "0x0", "eth_getTransactionCount": "0x0", "eth_getCode": "0x", "eth_call": "0x",
                  "eth_gasPrice": "0x1", "eth_estimateGas": "0x5208", "eth_maxPriorityFeePerGas": "0x1",
                  "eth_feeHistory": {"baseFeePerGas": ["0x1", "0x1"]}, "eth_getLogs": [],
                  "eth_getTransactionByHash": {"hash": self.transaction, "blockNumber": "0xa", "blockHash": self.tx_block["hash"]},
                  "eth_getTransactionReceipt": {"transactionHash": self.transaction, "blockNumber": "0xa", "blockHash": self.tx_block["hash"]},
                  "debug_traceTransaction": {"type": "CREATE"}}
        if method not in values:
            raise AssertionError(f"Unexpected probe method: {method}")
        return deepcopy(values[method])
