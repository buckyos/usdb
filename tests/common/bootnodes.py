"""Write isolated public seed catalogs for configuration and release acceptance."""
import json

from peer_sources import BOOTNODES_FILE, BOOTNODES_SCHEMA


def write_bootnodes(bundle, entries, *, bundle_id="usdb-testnet-v0"):
    """Create a catalog without touching chain identity or release metadata."""
    bundle.mkdir(parents=True, exist_ok=True)
    path = bundle / BOOTNODES_FILE
    path.write_text(json.dumps({"schema_version": BOOTNODES_SCHEMA,
                               "network_bundle_id": bundle_id, "bootnodes": entries}) + "\n")
    return path
