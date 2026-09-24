"""Isolated progress observations with no Docker daemon or live node data."""

from contextlib import ExitStack, contextmanager
from pathlib import Path
from tempfile import TemporaryDirectory
from types import SimpleNamespace
from unittest import mock

import usdb_node as NODE


def ready_miner_progress():
    """Core services are ready while the optional minting index is building."""
    snapshot = NODE._component_progress("snapshot", "READY", "Existing snapshot baseline reused; no file rescan needed", progress_percent=100)
    snapshot.update(label="UTXO snapshot", file_preparation=dict(state="VERIFIED", size_bytes=9_375_000_000))
    bitcoin = NODE._component_progress("bitcoin", "READY", "foreground=967943; background=None; history_validated=True", current=967943, total=967943)
    bitcoin.update(service_elapsed_secs=184, background_validation=dict(available=True, validated=True, height=None, target=935000))
    bh = NODE._component_progress("balance_history", "READY", "Synced up to block height 967933", current=967933, total=967933)
    bh.update(service_elapsed_secs=176, genesis_milestone=dict(height=963800, state="available"))
    indexer = NODE._component_progress("usdb_indexer", "READY", "Waiting for new blocks...", current=967933, total=967933)
    indexer.update(service_elapsed_secs=176)
    chain = NODE._component_progress("usdb_chain", "READY", "peers=0; FIRST_NODE: acknowledged first node", current=6287)
    chain.update(service_elapsed_secs=141, head=dict(number=6287, hash="0x" + "ab" * 32))
    return dict(release_id="usdb-testnet-v0-r33", node_role="miner", observed_at="2026-09-21T05:07:39+00:00",
                overall_state="READY", observation_elapsed_secs=198, resources=dict(phase="steady"),
                network=dict(name="usdb-testnet-v0", chain_id=202608250, network_id=202608250,
                             genesis_hash="0x" + "12" * 32, bitcoin_network="btc-mainnet"),
                controller=dict(display_state="update_required", runtime_state="inactive", exit_status="0",
                                observation_available=True, action_required=True,
                                summary="Refresh controller configuration before the next background startup.",
                                actions=["usdb-node controller install"]),
                minting=dict(enabled=True, state="WAITING_TXINDEX", core_height=967943, history_height=967943,
                             txindex_height=184401, txindex_synced=False, transactions_enabled=False,
                             guidance="Waiting for txindex to cover the foreground tip. If absent, check that Core adopted BTC_TXINDEX=1."),
                mining=dict(state="ACTIVE", configured={"USDB_NODE_ROLE": "miner", "USDB_MINER_THREADS": "1"}),
                components=[snapshot, NODE._component_progress("script_registry", "SKIPPED", "Native observed-script registry"), bitcoin, bh, indexer, chain])


def native_bootstrap_progress():
    """Mirror a foreground-ready node still validating Core history and building the BH baseline."""
    report = ready_miner_progress()
    report.update(release_id="usdb-testnet-v0-r35", node_role="full", overall_state="SYNCING",
                  observed_at="2026-09-24T11:43:58+00:00", controller=dict(display_state="running"), mining=None,
                  resources=dict(phase="overlap"), minting=dict(enabled=False))
    report.pop("observation_elapsed_secs")
    components = {item["id"]: item for item in report["components"]}
    components["bitcoin"].update(current=968390, total=968390, progress_phase="foreground", service_elapsed_secs=5338,
        detail="foreground=968390; background=340941; history_validated=False",
        background_validation=dict(available=True, validated=False, height=340941, target=935000))
    components["balance_history"].update(state="SYNCING", current=954400, total=968380,
        progress_percent=(954400 - 935000) * 100 / (968380 - 935000), progress_phase="replaying",
        detail="Replaying blocks toward genesis baseline", sync_start_height=935000,
        service_started_at="2026-09-24T11:09:19.592226+00:00", service_elapsed_secs=2078,
        genesis_milestone=dict(height=963800, state="replaying", remaining_blocks=9400))
    components["usdb_indexer"].update(state="WAITING", current=None, total=None, service_elapsed_secs=5331,
        detail="Waiting for balance-history queryable baseline; indexing has not started")
    components["usdb_chain"].update(state="WAITING", current=None,
        detail="Waiting for balance-history readiness: balance-history readiness check failed: [Errno 104] Connection reset by peer")
    components["usdb_chain"].pop("head")
    components["usdb_chain"].pop("service_elapsed_secs")
    report["native_bootstrap"] = dict(balance_history=dict(phase="replaying", height=954400,
                                                         target=963800, observed_file_mtime=1790250236.0))
    return report


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
            "_mining_status": {"state": "DISABLED", "applied": True},
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
