"""Isolated progress observations with no Docker daemon or live node data."""

from contextlib import ExitStack, contextmanager
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest import mock

import usdb_node as NODE


@contextmanager
def progress_fixture(services, *, pending=False):
    """Exercise the real progress aggregation with ready upstream observations."""
    with TemporaryDirectory(prefix="usdb-chain-progress-") as temporary, ExitStack() as stack:
        node_env = Path(temporary) / "node.env"
        node_env.write_text("USDB_RESOURCE_MODE=auto\nUSDB_RESOURCE_PHASE=steady\n")
        layout = SimpleNamespace(
            node_env=node_env, release_id="test-release", bundle_id="test-bundle",
            network_identity={"btc_index_origin_height": 1, "chain_id": 123,
                              "genesis_block_hash": "0x" + "ab" * 32},
        )
        ready = {"consensus_ready": True, "current": 10, "total": 10,
                 "synced_block_height": 10, "balance_history_stable_height": 10}
        observations = {
            "controller_observed_state": "activating",
            "_collect_compose_services": services,
            "_snapshot_lifecycle_status": {},
            "_snapshot_component": NODE._component_progress("snapshot", "READY", "imported"),
            "_script_registry_component": NODE._component_progress("script_registry", "SKIPPED", "optional"),
            "_bitcoin_startup_progress": {"ready": True, "status": {
                "blocks": 20, "headers": 20, "verification_progress": 1,
                "txindex_synced": True, "txindex_height": 20, "connections": 1}},
            "_bitcoin_data_start_anchor": SimpleNamespace(
                minimum_tip_height=11, stable_height=1, stable_lag_blocks=10, block_hash=None),
            "_read_service_readiness": (ready, None),
            "validate_resource_environment": None,
            "effective_memory_bytes": 64 * 1024**3,
            "_read_resource_state": {"phase": "steady", "pending": pending,
                                     "recover_services": ["usdb-chain", "usdb-control-plane"]},
            "_json_rpc_batch": {"eth_chainId": "0x7b", "eth_blockNumber": "0x0",
                                "eth_getBlockByNumber": {"hash": "0x" + "ab" * 32},
                                "eth_syncing": False, "net_peerCount": "0x0"},
        }
        for name, value in observations.items():
            stack.enter_context(mock.patch.object(NODE, name, return_value=value))
        yield layout
