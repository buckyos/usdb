"""Peer-operation fixtures sharing the mining test's private RPC/Docker model."""
from unittest import mock
import usdb_node as NODE
import usdb_peers as PEERS
from common.mining import MiningFixture
from common.enode import V4


class PeerFixture(MiningFixture):
    def __enter__(self):
        super().__enter__()
        self.stack.enter_context(mock.patch.object(NODE, "controller_active_state", return_value="inactive"))
        return self

    def rpc(self, layout, method, params=None, **kwargs):
        if method == "admin_nodeInfo":
            return {"id": self.chain["node_id"], "enode": self.chain["enode"]}
        return super().rpc(layout, method, params, **kwargs)

    def edit(self, action="add", enode=V4):
        return PEERS.submit(self.layout, action, enode)

    def run_peers(self):
        with NODE.node_operation_lock(self.layout, "peers"):
            return PEERS.run_operation(self.layout)
