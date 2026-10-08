"""Read-only, redacted projection of saved node configuration, without runtime probes."""

import json
from pathlib import Path
import stat
import textwrap
from types import SimpleNamespace
from urllib.parse import urlsplit, urlunsplit

from resource_policy import memory_bytes


# Explicit keys prevent a newly added credential from being exposed merely
# because its name does not contain PASSWORD/TOKEN. Unknown values stay hidden.
GROUPS = {
    "Identity and data paths": """
        USDB_NETWORK_BUNDLE_ID USDB_CHAIN_ID USDB_NETWORK_ID BTC_NETWORK USDB_DATA_ROOT
        USDB_DATA_LAYOUT USDB_RUNTIME_COMPATIBILITY_ID USDB_NODE_ROLE
        BTC_NODE_DATA_HOST_DIR BH_DATA_HOST_DIR USDB_INDEXER_DATA_HOST_DIR USDB_CHAIN_DATA_HOST_DIR
        CONTROL_PLANE_DATA_HOST_DIR ORD_DATA_HOST_DIR BTC_CONTAINER_UID BTC_CONTAINER_GID
    """,
    "Networking and firewall": """
        USDB_DOCKER_NETWORK USDB_BOOTNODES USDB_NAT USDB_P2P_IP_FAMILY USDB_P2P_REQUESTED_FAMILY
        USDB_P2P_ADVERTISE_IPV4 USDB_P2P_ADVERTISE_IPV6 USDB_P2P_ADVERTISE_PORT USDB_P2P_ADVERTISE_DISCOVERY_PORT
        USDB_P2P_BIND_ADDRESS USDB_P2P_BIND_PORT USDB_P2P_PORT USDB_DISCOVERY_PORT
        USDB_HTTP_BIND_ADDRESS USDB_HTTP_BIND_PORT USDB_HTTP_PORT USDB_WS_BIND_ADDRESS USDB_WS_BIND_PORT USDB_WS_PORT
        BH_BIND_ADDRESS BH_BIND_PORT USDB_INDEXER_BIND_ADDRESS USDB_INDEXER_BIND_PORT
        CONTROL_PLANE_BIND_ADDRESS CONTROL_PLANE_BIND_PORT BTC_P2P_BIND_ADDRESS BTC_P2P_BIND_PORT
        BTC_RPC_BIND_ADDRESS BTC_RPC_BIND_PORT USDB_FIREWALL_MODE USDB_OPERATOR_SSH_PORT
    """,
    "Memory budgets and caches": """
        USDB_RESOURCE_MODE USDB_RESOURCE_PHASE USDB_RESOURCE_HOST_MEMORY_BYTES USDB_STORAGE_PROFILE
        USDB_RESOURCE_MEMORY_PERCENT USDB_EXTERNAL_MEMORY_BUDGET USDB_BH_MEMORY_CAP
        USDB_BTC_IBD_MEMORY_CAP USDB_BTC_OVERLAP_MEMORY_CAP USDB_BTC_STEADY_MEMORY_CAP USDB_ORD_MEMORY_CAP
        BTC_RESOURCE_PROFILE BTC_MEMORY_LIMIT BTC_MEMORY_SWAP_LIMIT BTC_DBCACHE_MB BTC_BOOTSTRAP_MEMORY_LIMIT
        BH_MEMORY_LIMIT BH_MEMORY_SWAP_LIMIT BH_SYNC_UTXO_MAX_CACHE_BYTES BH_SYNC_BALANCE_MAX_CACHE_BYTES
        BH_SYNC_MAX_MEMORY_PERCENT USDB_INDEXER_MEMORY_LIMIT USDB_CHAIN_MEMORY_LIMIT CONTROL_PLANE_MEMORY_LIMIT
        BH_SCRIPT_REGISTRY_MEMORY_LIMIT USDB_CHECKPOINT_VERIFY_MEMORY_LIMIT ORD_MEMORY_LIMIT
        ORD_INDEX_CACHE_BYTES ORD_STEADY_INDEX_CACHE_BYTES USDB_ORD_RESOURCE_POLICY ORD_STARTUP_DEFERRED
    """,
    "Bitcoin Core": """
        BTC_RPC_URL BTC_AUTH_MODE BTC_RPCAUTH_HOST_FILE BTC_COOKIE_FILE BTC_DISABLE_WALLET BTC_TXINDEX
        BTC_MIN_READY_HEIGHT BTC_MAX_TIP_AGE_SECS BTC_MIN_CONNECTIONS
    """,
    "Snapshots and bootstrap": """
        SNAPSHOT_MODE BH_SNAPSHOT_HOST_DIR BH_SNAPSHOT_FILE BH_SNAPSHOT_MANIFEST BH_SCRIPT_REGISTRY_ENABLED
        BH_SCRIPT_REGISTRY_RECORD_URL BH_SCRIPT_REGISTRY_ARTIFACT_ID BH_SNAPSHOT_TRUST_MODE
        BH_SNAPSHOT_TRUSTED_KEYS_FILE USDB_INDEXER_CHECKPOINT_MANIFEST BH_ASSUMEUTXO_BASE_HEIGHT
        BH_ASSUMEUTXO_ENABLED BTC_ASSUMEUTXO_ARTIFACT_HOST_DIR BTC_ASSUMEUTXO_STATE_HOST_DIR
        BTC_ASSUMEUTXO_SNAPSHOT_FILE BTC_ASSUMEUTXO_SNAPSHOT_URL BTC_ASSUMEUTXO_SNAPSHOT_SHA256
        BTC_ASSUMEUTXO_SOURCE_URL BTC_ASSUMEUTXO_MANIFEST_URL BTC_ASSUMEUTXO_MANIFEST_FILE BTC_ASSUMEUTXO_DISTRIBUTION_MODE
        BH_ASSUMEUTXO_ORIGIN_BLOCK_HASH BH_ASSUMEUTXO_SNAPSHOT_FILE
        BH_ASSUMEUTXO_MANIFEST_HOST_FILE BH_ASSUMEUTXO_TRUST_HOST_DIR BH_SYNC_LOCAL_LOADER_THRESHOLD
        USDB_GENESIS_BLOCK_HEIGHT BTC_ACTIVATION_REGISTRY_ID BTC_ACTIVATION_REGISTRY_CATALOG_FILE USDB_RULES_SCOPE
    """,
    "Mining and optional services": """
        USDB_MINER_ADDRESS USDB_MINER_THREADS USDB_CHAIN_GCMODE USDB_CHAIN_TRACING INSCRIPTION_SOURCE INSCRIPTION_SOURCE_SHADOW_COMPARE
        USDB_MINTING_ENABLED ORD_COMMIT_INTERVAL ORD_MIN_FREE_BYTES
        USDB_DEEP_REORG_GUARD_ENABLED USDB_DEEP_REORG_GUARD_POLL_INTERVAL_SECS
        USDB_DEEP_REORG_GUARD_REQUEST_TIMEOUT_SECS USDB_DEEP_REORG_GUARD_MAX_CONSECUTIVE_ERRORS
    """,
    "Monitoring and logs": "USDB_MONITOR_ENABLED USDB_LOG_MAX_SIZE USDB_LOG_MAX_FILES",
    "Configured release images": "USDB_SERVICES_IMAGE USDB_CHAIN_IMAGE USDB_BITCOIN_IMAGE",
}
PUBLIC = {key: group for group, keys in GROUPS.items() for key in keys.split()}


def _read(path, node, label):
    """Do not echo parser exceptions: malformed lines can contain credentials."""
    try:
        info = path.stat()
        if not stat.S_ISREG(info.st_mode) or info.st_size > 1024 * 1024:
            raise ValueError("unsupported configuration file")
        return node.read_env(path)
    except (OSError, ValueError):
        raise ValueError(f"Cannot read {label}; check file access and configuration syntax. Values were not printed.") from None


def _value(key, value):
    """Hide credentials, unknown settings, arbitrary arguments and URL secrets."""
    if key not in PUBLIC:
        return ("[redacted]" if value else ""), bool(value)
    if key.endswith("_URL") and value:
        try:
            url = urlsplit(value)
            if url.scheme not in {"http", "https", "tcp"} or not url.hostname:
                raise ValueError("unsupported endpoint")
            authority = url.netloc.rsplit("@", 1)[-1]
            redacted = bool(url.username is not None or url.query or url.fragment)
            return urlunsplit((url.scheme, ("[redacted]@" if url.username is not None else "") + authority,
                               url.path, "[redacted]" if url.query else "", "[redacted]" if url.fragment else "")), redacted
        except ValueError:
            return "[redacted]", True
    return value, False


def collect(kit_root, node_env, node):
    """Show persisted values and their source; defaults inside services are not guessed."""
    version = node.collect_node_version(kit_root, node_env)
    path = Path(version["node_env"])
    configured = version["configured_images"]["state"] != "UNCONFIGURED"
    env = _read(path, node, "node.env") if configured else {}
    bundle = Path(version["kit_root"]) / "docker/networks" / version["network"]["bundle_id"]
    network = _read(bundle / "network.env", node, "bundled network.env")
    groups = {name: [] for name in (*GROUPS, "Sensitive or unclassified settings")}
    for source, values in (("network.env", network), ("node.env", env)):
        for key, value in sorted(values.items()):
            safe, hidden = _value(key, value)
            groups[PUBLIC.get(key, "Sensitive or unclassified settings")].append(
                dict(key=key, value=safe, source=source, redacted=hidden))
    monitor = {"path": str(path.resolve().parent / "monitor/config.json"), "settings": {}}
    if configured:
        import node_monitor
        try:
            monitor["settings"] = node_monitor.config(SimpleNamespace(node_env=path))
            monitor["state"] = "saved_with_defaults" if Path(monitor["path"]).exists() else "defaults"
        except (OSError, ValueError):
            monitor["state"] = "unavailable"
    else:
        monitor["state"] = "unconfigured"
    return dict(schema_version="usdb-node-config:v1", configured=configured, runtime_observed=False,
                release_id=version["release_id"], network=version["network"], node_env=str(path),
                network_env=str(bundle / "network.env"),
                configured_images=version["configured_images"],
                groups=[dict(name=name, settings=items) for name, items in groups.items() if items],
                monitor=monitor,
                notes=["Saved configuration only; use usdb-node status to check running services.",
                       "Secrets, unknown values and arbitrary extra arguments are hidden; empty values remain visible.",
                       "Service-internal defaults and shell overrides are not expanded. Notification credentials, wallets and systemd unit contents are not read."])


def show(kit_root, node_env, node, *, json_output=False):
    """Render the same redacted fields in both terminal and JSON output."""
    report = collect(kit_root, node_env, node)
    if json_output:
        print(json.dumps(report, indent=2, sort_keys=True))
        return 0
    def clean(value):
        return "".join(char if char.isprintable() else f"\\u{ord(char):04x}" for char in str(value))
    print(f"USDB saved configuration | {report['release_id']}")
    print(f"Network: {report['network']['bundle_id']} | Chain ID: {report['network']['chain_id']}")
    print(f"Node config: {clean(report['node_env'])}")
    print(f"Network defaults: {clean(report['network_env'])}")
    print(f"Configured images: {report['configured_images']['state']}")
    if not report["configured"]:
        print("Node is not configured; run usdb-node setup. Only bundled network settings are shown.")
    for group in report["groups"]:
        print("\n" + group["name"])
        width = max(len(item["key"]) for item in group["settings"])
        for item in group["settings"]:
            value = item["value"] if item["value"] else "(empty)"
            if not item["redacted"] and item["value"]:
                key = item["key"]
                try:
                    size = None
                    if key == "BTC_DBCACHE_MB":
                        size = int(value) * 1024**2
                    elif key in node.CAP_DEFAULTS or key.endswith(("_BYTES", "_MEMORY_LIMIT", "_MEMORY_SWAP_LIMIT")):
                        size = 0 if value == "0" else memory_bytes(value, key)
                    if size is not None:
                        value = f"{node._human_bytes(size)} (saved: {value})"
                except ValueError:
                    pass  # Keep malformed saved values visible for diagnosis.
            prefix = f"  {item['key']:<{width}}  "
            wrapped = textwrap.fill(clean(value), width=110, initial_indent=prefix,
                                    subsequent_indent=" " * len(prefix), break_long_words=False, break_on_hyphens=False)
            print(wrapped + f"  [{item['source']}]")
    monitor = report["monitor"]
    print(f"\nMonitor policy | {monitor['state']} | {clean(monitor['path'])}")
    for key, value in monitor["settings"].items():
        print(f"  {key:<28} {value}")
    print()
    for note in report["notes"]:
        print(note)
    print("Edit: usdb-node down -> usdb-node setup -> usdb-node doctor -> usdb-node up")
    return 0
