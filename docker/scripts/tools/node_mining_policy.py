"""Local UIP-0018 configuration and display-only Go work diagnostics."""

import re
import subprocess

KEY = "USDB_MINER_ANCHOR_GAP"
DEFAULT_GAP = 2
MAX_HEIGHT = 2**32 - 1
REASONS = {"gap", "history_origin", "parent_anchor", "age_limit"}


def parse_gap(value):
    """Accept canonical unsigned BTC-height integers, including explicit zero."""
    if not isinstance(value, str) or not re.fullmatch(r"0|[1-9][0-9]{0,9}", value) or int(value) > MAX_HEIGHT:
        raise ValueError(f"{KEY} must be an integer from 0 to {MAX_HEIGHT}")
    return int(value)


def configured_gap(env):
    return parse_gap(env.get(KEY, str(DEFAULT_GAP)))


def policy_view(env, runtime, mining):
    """Never infer the running binary's default from saved configuration."""
    value = configured_gap(env)
    actual = None
    if runtime.get("state") == "running":
        raw = mining.flag_values(runtime.get("argv", [])).get("--miner.usdb-anchor-gap")
        if isinstance(raw, str):
            actual = parse_gap(raw)
    return dict(configured_gap=value, source="node.env" if KEY in env else "node-kit default",
                runtime_gap=actual, runtime_state="unknown" if actual is None else "matches" if actual == value else "differs")


def configure(layout, value, mining):
    """Save only local policy; the existing up reconciliation applies chain drift."""
    import usdb_peers
    node = mining.node
    gap = parse_gap(value)
    with node.node_operation_lock(layout, "mining-configure"):
        if not layout.node_env.is_file():
            raise ValueError("Node is not configured; run usdb-node setup first")
        if mining.pending(layout) or usdb_peers.pending(layout):
            raise ValueError("Finish the pending mining/peer operation before changing anchor gap")
        original = layout.node_env.read_text()
        env = node.read_env(layout.node_env)
        previous = configured_gap(env)
        node._atomic_write_private(layout.node_env, node.upsert_env(original, {KEY: str(gap)}))
    return dict(state="SAVED", configured_gap=gap, previous_gap=previous,
                detail="Local policy saved. Run usdb-node up to apply it; a running node recreates only usdb-chain. "
                       "Data and existing mining authorization are retained. Check mining status after startup.")


def _height(value):
    return type(value) is int and 0 <= value <= MAX_HEIGHT


def _build(value, mining):
    """Check display invariants without reimplementing Go's height selection."""
    if not isinstance(value, dict) or not isinstance(value.get("stage"), str):
        raise ValueError("Invalid builder diagnostic")
    if not mining.node.ADDRESS_RE.fullmatch(str(value.get("miner_address", ""))):
        raise ValueError("Missing builder address")
    latest = value.get("latest_height")
    selection = value.get("selection")
    if latest is not None and not _height(latest):
        raise ValueError("Invalid latest height")
    if selection is not None:
        if (not isinstance(selection, dict) or any(not _height(selection.get(k)) for k in
                ("latest_height", "selected_height", "configured_gap", "actual_gap"))
                or selection["latest_height"] != latest or selection["selected_height"] > latest
                or selection["actual_gap"] != latest - selection["selected_height"]
                or selection.get("reason") not in REASONS):
            raise ValueError("Invalid anchor selection")
    if value.get("stage") == "validated":
        if (selection is None or value.get("failure") or not mining.PASS.fullmatch(str(value.get("pass_id", "")))
                or not _height(value.get("anchor_age")) or not _height(value.get("max_anchor_age"))
                or value["anchor_age"] > value["max_anchor_age"]
                or any(not mining.HASH.fullmatch(str(value.get(k, ""))) for k in ("snapshot_id", "system_state_id"))):
            raise ValueError("Invalid validated work")


def observe_work(layout, env, mining):
    """A missing/old RPC yields unknown, never a guessed gap or selected pass."""
    try:
        value = mining.rpc(layout, "miner_getUSDBWorkStatus")
        if (not isinstance(value, dict) or value.get("schema_version") != "usdb-miner-work:v1"
                or not _height(value.get("configured_gap"))
                or type(value.get("mining")) is not bool or type(value.get("work_current")) is not bool
                or not mining.HASH.fullmatch(str(value.get("head_hash", "")))):
            raise ValueError("Invalid work status")
        if value.get("work") is not None and not isinstance(value["work"], dict):
            raise ValueError("Invalid submitted work")
        for build in (value.get("attempt"), (value.get("work") or {}).get("build")):
            if build is not None:
                _build(build, mining)
                if build["miner_address"].lower() != env.get("USDB_MINER_ADDRESS", "").lower():
                    raise ValueError("Work address mismatch")
                if build.get("selection", {}).get("configured_gap", value["configured_gap"]) != value["configured_gap"]:
                    raise ValueError("Work configuration mismatch")
        work = value.get("work")
        if work is not None:
            if (not isinstance(work, dict) or work.get("build", {}).get("stage") != "validated"
                    or any(not mining.HASH.fullmatch(str(work.get(k, ""))) for k in ("seal_hash", "parent_hash"))):
                raise ValueError("Invalid submitted work")
        if value["work_current"] and (not value["mining"] or not work or work["parent_hash"] != value["head_hash"]):
            raise ValueError("Stale submitted work")
        return dict(available=True, **value)
    except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError):
        return dict(available=False, detail="Go work diagnostics unavailable or incompatible; actual anchor/pass unknown")


def candidate_hint(layout, report, mining):
    """Estimate mint visibility only from a pinned snapshot, never promise eligibility."""
    work = report.get("anchor_work", {})
    candidate = report.get("eligibility", {}).get("candidate", {})
    profile, external = candidate.get("pass", {}), candidate.get("external_state", {})
    current = work.get("work", {}).get("build", {}) if work.get("work_current") and report.get("work_available") else {}
    if not work.get("available") or not profile or current.get("pass_id") == profile.get("pass_id"):
        return
    report["candidate_hint"] = "Latest candidate differs from current work; historical eligibility is decided by Go."
    try:
        height = external["btc_height"]
        context = {"requested_height": height, "expected_state": {
            "snapshot_id": external["snapshot_id"], "system_state_id": external["system_state_id"]}}
        snap = mining.rpc(layout, "get_pass_snapshot", [{"inscription_id": profile["pass_id"],
                          "at_height": height, "context": context}], indexer=True)
        mint = snap.get("mint_block_height") if isinstance(snap, dict) else None
        if (not _height(mint) or mint > height or snap.get("inscription_id") != profile["pass_id"]
                or snap.get("resolved_height") != height or snap.get("state") != "active"
                or str(snap.get("usdb_main", "")).lower() != str(profile.get("usdb_main", "")).lower()):
            return
        target = mint + work["configured_gap"]
        if target > MAX_HEIGHT:
            return
        report["candidate_hint"] = (f"Latest candidate minted at BTC {mint}; normal gap visibility from indexer height {target} "
            f"(latest observed {height}). Conditional on continued eligibility and safe historical state; "
            "parent/age constraints may select it earlier. This is not a block-finding ETA.")
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        pass


def display_lines(report):
    """Share the compact diagnostics between mining status and status --watch."""
    lines = []
    policy = report.get("anchor_policy", {})
    if policy:
        actual = policy.get("runtime_gap")
        lines.append(f"BTC anchor gap: configured {policy['configured_gap']} ({policy['source']}) | "
                     f"running {actual if actual is not None else 'unknown'}")
    work = report.get("anchor_work", {})
    if work.get("available"):
        current = work.get("work", {}).get("build", {}) if work.get("work_current") and report.get("work_available") else {}
        selection = current.get("selection", {})
        if selection:
            lines.append(f"Work anchor: L={selection['latest_height']} -> H={selection['selected_height']} | "
                         f"actual gap {selection['actual_gap']} | {selection['reason']} | "
                         f"age {current['anchor_age']}/{current['max_anchor_age']}")
            lines.append(f"Work Pass: {current['pass_id']}")
        else:
            lines.append("Current work anchor/pass: unavailable (not yet matched to current eth_getWork)")
        if work.get("retry_pending"):
            lines.append("Worker retry pending; observing last submitted work separately. Inspect chain logs if this persists.")
        attempt = work.get("attempt") or {}
        if attempt.get("failure"):
            selection = attempt.get("selection") or {}
            lines.append(f"Latest build: {attempt['stage']} / {attempt['failure']} | "
                         f"L={attempt.get('latest_height', '?')} | H={selection.get('selected_height', '?')} | "
                         f"required height={attempt.get('required_height', '?')}; inspect chain logs for details")
    elif work:
        lines.append(work["detail"])
    if report.get("candidate_hint"):
        lines.append(report["candidate_hint"])
    return lines
