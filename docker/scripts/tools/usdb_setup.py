"""Edit operator-owned settings without recreating node identity or deployment state."""

from __future__ import annotations

from dataclasses import replace
from pathlib import Path
import tempfile

import usdb_minting as minting
from resource_policy import memory_bytes


def _display_value(key, value, node):
    """Format capacities for review while retaining exact values in node.env."""
    if value == "(unset)":
        return value
    if key == "BTC_DBCACHE_MB":
        return node._human_bytes(int(value) * 1024**2)
    if key in node.CAP_DEFAULTS or key.endswith(("_BYTES", "_MEMORY_LIMIT", "_MEMORY_SWAP_LIMIT")):
        return node._human_bytes(0 if value == "0" else memory_bytes(value, key))
    return value


def _require_stopped(layout, node):
    """Do not let the wizard race a recovery operation or a running container."""
    import usdb_mining
    import usdb_peers
    if usdb_peers.pending(layout):
        raise ValueError("A peer operation is pending; finish it with usdb-node peers apply before setup")
    if usdb_mining.pending(layout):
        raise ValueError("A mining operation is pending; finish it or use mining disable before setup")
    if any(item.get("state") not in {"exited", "dead", "created"}
           for item in node._collect_compose_services(layout).values()):
        raise ValueError("Stop the node with usdb-node down before editing its configuration")


def _resources(env, updates, node, *, choice, yes_no, prompt, resource_mode, bitcoin_profile, caps):
    """Retain allocations unless explicitly recalculated or their inputs change."""
    current = node.resource_mode(env)
    mode = choice("Resource management", ("auto", "manual"), resource_mode or current)
    if bitcoin_profile is not None and mode != "manual":
        raise ValueError("--bitcoin-profile requires manual resource mode")
    defaults = node.resource_cap_defaults(env)
    settings = {key: env.get(key, default) for key, default in defaults.items()}
    settings.update(caps or {})
    if yes_no("Adjust memory budgets (bytes or k/m/g; external reserve may be 0)", bool(caps)):
        for key in node.CAP_DEFAULTS:
            settings[key] = prompt(key, settings[key])
    for key, value in settings.items():
        if key != "USDB_EXTERNAL_MEMORY_BUDGET" or value != "0":
            memory_bytes(value, key)
    caps_changed = any(value != env.get(key, defaults[key]) for key, value in settings.items())
    mint_changed = any(key in updates for key in ("USDB_MINTING_ENABLED", "ORD_MEMORY_LIMIT"))
    recalculate = mode != current or caps_changed or mint_changed
    if mode == "auto" and not recalculate:
        recalculate = yes_no("Recalculate automatic budgets for current host memory", False)
    if recalculate:
        updates.update(node._resource_policy_updates(mode, {**env, **updates, **settings}))
    if mode == "manual":
        # Keep custom manual values on Enter, including profiles from older kits.
        default = bitcoin_profile or (node.DEFAULT_BITCOIN_RESOURCE_PROFILE if current == "auto" else "keep")
        profile = choice("Bitcoin memory profile (keep preserves current limits)",
                         ("keep", node.AUTO_BITCOIN_RESOURCE_PROFILE, *node.BITCOIN_RESOURCE_PROFILES), default)
        if profile != "keep":
            selected, values = node.resolve_bitcoin_resource_profile(profile)
            updates.update(BTC_RESOURCE_PROFILE=selected, BTC_MEMORY_LIMIT=values["memory_limit"],
                           BTC_MEMORY_SWAP_LIMIT=values["memory_swap_limit"], BTC_DBCACHE_MB=values["dbcache_mb"])
            recalculate = True
    return recalculate


def _collect(env, node, *, input_fn, output, resource_mode, bitcoin_profile, caps):
    """Collect a candidate in memory; Enter retains effective current settings."""
    def prompt(label, default):
        return node._prompt(label, default=default, input_fn=input_fn)

    def memory_prompt(label, default):
        # Enter must retain the exact budget, not the rounded display value.
        return input_fn(f"{label} [{_display_value(label, default, node)}]: ").strip() or default

    def choice(label, choices, default):
        return node._prompt_choice(label, choices, default=default, input_fn=input_fn, output=output)

    def yes_no(label, default):
        return node._prompt_yes_no(label, default=default, input_fn=input_fn, output=output)

    updates = {}
    current_monitor = env.get("USDB_MONITOR_ENABLED", "1")
    if current_monitor not in {"0", "1"}:
        raise ValueError("USDB_MONITOR_ENABLED must be 0 or 1")
    selected_monitor = "1" if yes_no("Enable node monitor (local events and alerts)", current_monitor == "1") else "0"
    if selected_monitor != current_monitor:
        updates["USDB_MONITOR_ENABLED"] = selected_monitor
    state, tracing, remaining = node.chain_query_settings(env)
    if (state, tracing) in {("archive", "1"), ("full", "0")}:
        full = yes_no("Provide full Explorer support (archive + private tracing)", state == "archive")
        target = ("archive", "1") if full else ("full", "0")
    else:
        print(f"Current query settings: state={state}, private tracing={'on' if tracing == '1' else 'off'}", file=output)
        selected = choice("Explorer support (keep retains independent query settings)", ("keep", "full", "basic"), "keep")
        target = (state, tracing) if selected == "keep" else ("archive", "1") if selected == "full" else ("full", "0")
    if target != (state, tracing):
        updates.update(USDB_CHAIN_GCMODE=target[0], USDB_CHAIN_TRACING=target[1])
        if "USDB_CHAIN_EXTRA_ARGS" in env:
            updates["USDB_CHAIN_EXTRA_ARGS"] = " ".join(remaining)
        print("Archive retains future states; it does not restore pruned history. Tracing stays private.", file=output)

    active = yes_no("Enable local minting backend (txindex + private Ord)", minting.enabled(env))
    if active != minting.enabled(env):
        updates.update(minting.environment(env["USDB_DATA_ROOT"], active,
                       legacy_txindex="0" if env.get("SNAPSHOT_MODE") == "assumeutxo" else "1"))
        for key in ("ORD_MEMORY_LIMIT", "ORD_INDEX_CACHE_BYTES", "ORD_MIN_FREE_BYTES"):
            if key in env:
                updates[key] = env[key]
    if active:
        print("Ord waits for Bitcoin historical validation and txindex; existing data is retained. Wallet signing remains disabled.", file=output)
        settings = {**minting.environment(env["USDB_DATA_ROOT"], True), **env, **updates}
        print("Ord resources (recommended defaults unless previously customized): "
              f"memory {_display_value('ORD_MEMORY_LIMIT', settings['ORD_MEMORY_LIMIT'], node)}, "
              f"index cache {_display_value('ORD_INDEX_CACHE_BYTES', settings['ORD_INDEX_CACHE_BYTES'], node)}, "
              f"free-disk reserve {_display_value('ORD_MIN_FREE_BYTES', settings['ORD_MIN_FREE_BYTES'], node)}.", file=output)
        print("The free-disk reserve is a safety threshold, not the total index size.", file=output)
        capacity = minting.check_disk_capacity(settings)
        if capacity["new_index"]:
            print(f"Ord new-index capacity check passed: {node._human_bytes(capacity['free_bytes'])} free; "
                  f"at least {node._human_bytes(capacity['required_bytes'])} required in addition to node storage. "
                  "Bitcoin txindex and other service growth need additional capacity.", file=output)
        else:
            print("Existing Ord index will be reused; a second 300 GiB index budget is not required. "
                  "Runtime free-space protection remains active.", file=output)
    # Only effective changes should trigger resource recalculation.
    updates = {key: value for key, value in updates.items() if env.get(key) != value}
    recalculate = _resources(env, updates, node, choice=choice, yes_no=yes_no, prompt=memory_prompt,
                             resource_mode=resource_mode, bitcoin_profile=bitcoin_profile, caps=caps)
    public = yes_no("Accept inbound Bitcoin peers on TCP/" + env.get("BTC_P2P_BIND_PORT", "8333"),
                    env.get("BTC_P2P_BIND_ADDRESS") == "0.0.0.0")
    address = "0.0.0.0" if public else "127.0.0.1"
    if address != env.get("BTC_P2P_BIND_ADDRESS"):
        updates["BTC_P2P_BIND_ADDRESS"] = address
    current_firewall = env.get("USDB_FIREWALL_MODE", "managed")
    managed = yes_no("Manage this host firewall with the bundled UFW profile", current_firewall == "managed")
    mode = "managed" if managed else "external"
    if mode != current_firewall:
        updates["USDB_FIREWALL_MODE"] = mode
    if managed:
        port = str(node._require_port("operator SSH port", int(prompt("Operator SSH server port", env["USDB_OPERATOR_SSH_PORT"]))))
        if port != env["USDB_OPERATOR_SSH_PORT"]:
            updates["USDB_OPERATOR_SSH_PORT"] = port
    return {key: value for key, value in updates.items() if env.get(key) != value}, recalculate


def _validate_candidate(layout, node, content):
    """Validate a private sibling file before replacing the live configuration."""
    with tempfile.TemporaryDirectory(prefix=".setup-", dir=layout.node_env.parent) as directory:
        path = Path(directory) / "node.env"
        node._atomic_write_private(path, content)
        candidate = replace(layout, node_env=path)
        node._validate_node_config(candidate, require_runtime=False, require_bitcoin_runtime=True)
        env = node.read_env(path)
        node.validate_resource_environment(env, node.effective_memory_bytes())
        minting.validate(env)


def _next_steps(layout, node, env, *, firewall_changed, changed, output):
    """Distinguish saved settings, release activation and host firewall application."""
    if any(env.get(key) != value for key, value in layout.images.items()):
        print("Release images were preserved. Run usdb-node activate-release before startup to select this installed release.", file=output)
    if firewall_changed:
        if env.get("USDB_FIREWALL_MODE", "managed") == "managed":
            print("Apply the saved host firewall policy with usdb-node firewall apply --confirm, then verify with usdb-node firewall check.", file=output)
        else:
            print("Host firewall rules were not changed or removed. Manage SSH/P2P access with your external firewall policy.", file=output)
    if changed:
        print("Saved settings take effect at the next startup; no prepare --replace is needed.", file=output)
    if node.controller_unit_path(layout).is_file():
        print("Run usdb-node doctor, then usdb-node up. Existing controller units were preserved.", file=output)
    else:
        print("Run usdb-node doctor, then usdb-node up --foreground; for background startup install the controller with usdb-node controller install.", file=output)


def edit(layout, node, *, input_fn, output, resource_mode=None, bitcoin_profile=None, caps=None, p2p_options=None):
    """Review and save a stopped node's settings under the caller's operation lock."""
    if any(value is not None for value in (p2p_options or {}).values()):
        raise ValueError("Existing P2P settings use usdb-node peers configure --ip-family; run setup without P2P flags to edit other settings")
    if layout.node_env.is_symlink() or not layout.node_env.is_file():
        raise ValueError("Existing node configuration must be a regular file, not a symlink")
    _require_stopped(layout, node)
    import node_monitor
    if node_monitor.is_running(layout):
        raise ValueError("Stop the node monitor with usdb-node down before editing configuration")
    original = layout.node_env.read_text(encoding="utf-8")
    env = node.read_env(layout.node_env)
    print(f"Edit existing node configuration: {layout.node_env}", file=output)
    print(f"Data root: {env['USDB_DATA_ROOT']}; role: {env.get('USDB_NODE_ROLE', 'full')}; resource mode: {node.resource_mode(env)}", file=output)
    print("Enter keeps current values. Data paths, credentials, release images and mining identity are retained.", file=output)
    print("Use set-role, mining enable/disable and peers commands for role, mining identity, seeds and P2P changes.", file=output)
    try:
        updates, recalculate = _collect(env, node, input_fn=input_fn, output=output,
                                        resource_mode=resource_mode, bitcoin_profile=bitcoin_profile, caps=caps)
        candidate = {**env, **updates}
        firewall_changed = bool(set(updates) & {"USDB_FIREWALL_MODE", "USDB_OPERATOR_SSH_PORT", "BTC_P2P_BIND_ADDRESS"})
        if not updates and not recalculate:
            print("No configuration changes; existing files were preserved.", file=output)
            _next_steps(layout, node, env, firewall_changed=False, changed=False, output=output)
            return
        # Resource planning receives the full environment, but only changed keys
        # are rendered or displayed. Never display preserved credentials.
        print("Configuration changes:", file=output)
        if recalculate and node.resource_mode(candidate) == "auto":
            print(f"  Automatic resource policy: phase={candidate['USDB_RESOURCE_PHASE']}. "
                  "Service limits and caches below are calculated budgets, not manual overrides.", file=output)
            if minting.enabled(candidate):
                print("  Ord memory is reserved in every phase; txindex shares Bitcoin's budget. "
                      "Enabling txindex does not restart the Bitcoin-only resource phase.", file=output)
        for key, value in sorted(updates.items()):
            print(f"  {key}: {_display_value(key, env.get(key, '(unset)'), node)} -> "
                  f"{_display_value(key, value, node)}", file=output)
        if recalculate:
            print("  Resource transition journal will be reset; startup rechecks readiness using the saved resource policy.", file=output)
        content = node.upsert_env(original, updates)
        _validate_candidate(layout, node, content)
        if not node._prompt_yes_no("Save these changes", default=True, input_fn=input_fn, output=output):
            raise ValueError("setup cancelled; existing configuration was preserved")
    except EOFError as error:
        raise ValueError("setup cancelled; existing configuration was preserved") from error
    # Recheck after interaction: direct Docker starts and hand edits do not take
    # the node operation lock. Do not overwrite an operator's concurrent edit.
    _require_stopped(layout, node)
    if layout.node_env.is_symlink() or layout.node_env.read_text(encoding="utf-8") != original:
        raise ValueError("Node configuration changed during setup; retry without overwriting it")
    backup = layout.node_env.with_name(layout.node_env.name + ".setup-backup")
    if backup.is_symlink():
        raise ValueError("Setup backup must not be a symlink")
    if recalculate:
        node._read_resource_state(layout)
    minting.prepare(candidate)
    node._atomic_write_private(backup, original)
    if recalculate:
        # A stopped node can regenerate this derived journal even if the final
        # write fails; never leave an old recovery plan paired with new limits.
        node._resource_state_path(layout).unlink(missing_ok=True)
    node._atomic_write_private(layout.node_env, content)
    print(f"Updated node configuration: {layout.node_env}; previous configuration: {backup}", file=output)
    _next_steps(layout, node, candidate, firewall_changed=firewall_changed, changed=True, output=output)
