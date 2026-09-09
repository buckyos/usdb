"""Host and Docker boundaries for P2P transport acceptance."""
from copy import deepcopy
from unittest import mock
import usdb_node as NODE
import usdb_p2p as P2P
from common.peers import PeerFixture
from common.enode import PUBLIC_KEY

V4 = "8.8.8.8"
V6 = "2001:4860::1"
HOST = {"ipv4": ["192.168.1.10"], "ipv6": [V6], "ipv6_default_route": True, "errors": []}


def container(family, state="running"):
    addresses = {"ipv4": ["0.0.0.0"], "ipv6": ["::"], "dual": ["0.0.0.0", "::"]}[family]
    return {"state": state, "ports": {f"31303/{protocol}": [{"HostIp": ip, "HostPort": "31303"} for ip in addresses]
                                      for protocol in ("tcp", "udp")},
            "networks": {"runtime": {"IPAddress": "172.20.0.3"},
                         **({"p2p": {"GlobalIPv6Address": "fd12:3456::2", "IPv6Gateway": "fd12:3456::1"}}
                            if family != "ipv4" else {})}}


class P2PFixture(PeerFixture):
    def __enter__(self):
        super().__enter__()
        self.update_env(USDB_FIREWALL_MODE="external")
        self.host = deepcopy(HOST)
        self.stack.enter_context(mock.patch.object(P2P, "host_capabilities", side_effect=lambda: deepcopy(self.host)))
        self.stack.enter_context(mock.patch.object(P2P, "engine_capabilities", return_value={"engine": "28.0.0", "compose": "2.33.1"}))
        self.stack.enter_context(mock.patch.object(P2P, "container_view", side_effect=lambda *a:
            container(self.runtime["environment"].get("USDB_P2P_IP_FAMILY", "ipv4"), self.runtime["state"])))
        self.chain["enode"] = f"enode://{PUBLIC_KEY}@172.20.0.3:31303"
        return self

    def adopt(self):
        super().adopt()
        self.runtime["environment"]["USDB_P2P_IP_FAMILY"] = NODE.read_env(self.layout.node_env).get("USDB_P2P_IP_FAMILY", "ipv4")

    def configure(self, family="dual", **kwargs):
        from usdb_peers import submit
        updates, reason = P2P.select(family, **kwargs)
        return submit(self.layout, "configure", transport_updates=updates)
