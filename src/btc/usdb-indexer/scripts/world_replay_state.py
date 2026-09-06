"""Semantic state checkpoints for comparing rollback with fresh canonical replay."""

import hashlib
import json
from pathlib import Path


SCHEMA = "usdb-world-replay:v1"
VIEW_VERSION = "uip-0006-usdb-economic-state-view:v1"


def require(condition, message):
    if not condition:
        raise ValueError(f"world replay: {message}")


def digest(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":")).encode()).hexdigest()


def write_json(path, value):
    """Publish complete evidence atomically, retaining the previous checkpoint on interruption."""
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_suffix(".tmp")
    temporary.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")
    temporary.replace(path)


def collect_pages(rpc, method, params, key, *, cursor=False):
    """Exhaust either paging contract and reject truncated, duplicate or drifting pages."""
    items, seen, cursors = [], set(), set()
    page, token, header = 0, None, None
    while True:
        paging = {"cursor": token, "limit": 8} if cursor else {"page": page, "page_size": 32}
        result = rpc(method, [{**params, **paging}])
        require(isinstance(result, dict) and isinstance(result.get("items"), list), f"bad page: {method}")
        current = {k: v for k, v in result.items() if k not in {"items", "next_cursor", "limit", "max_limit"}}
        if header is None:
            header = current
            require(type(header.get("total")) is int and header["total"] >= 0, f"bad total: {method}")
        require(current == header, f"page metadata changed: {method}")
        for item in result["items"]:
            identity = item[key]
            require(identity not in seen, f"duplicate {key} in {method}: {identity}")
            seen.add(identity)
            items.append(item)
        require(len(items) <= header["total"], f"too many rows: {method}")
        if cursor:
            token = result.get("next_cursor")
            if token is None:
                break
            require(token not in cursors and result["items"], f"stalled cursor: {method}")
            cursors.add(token)
        else:
            if len(items) == header["total"]:
                break
            require(result["items"], f"truncated pages: {method}")
            page += 1
    require(len(items) == header["total"], f"incomplete rows: {method}")
    return {**header, "items": items}


def capture_state(usdb, balance, height, block_hash, owners, context_factory, *, history=False):
    """Read every pass at a fixed height; final checkpoints additionally read full ledgers."""
    state_ref = usdb("get_state_ref_at_height", [{"block_height": height}])
    require(state_ref["block_height"] == height, "indexer state height mismatch")
    require(state_ref["snapshot_info"]["stable_block_hash"] == block_hash, "indexer canonical hash mismatch")
    balance_ref = balance("get_state_ref_at_height", [{"block_height": height}])
    require(balance_ref["block_height"] == height and balance_ref["stable_block_hash"] == block_hash,
            "balance canonical height/hash mismatch")
    context = context_factory(state_ref)
    require(all(isinstance(value, str) and value for value in context["expected_state"].values()),
            "incomplete historical state identity")
    view = {"view_version": VIEW_VERSION, "block_height": height, "context": context}
    recent = collect_pages(usdb, "get_recent_passes", {"at_height": height, "order": "asc"}, "inscription_id")
    stats = usdb("get_pass_stats_at_height", [{"at_height": height}])
    require(stats["total_count"] == len(recent["items"]), "pass enumeration disagrees with stats")
    require(recent["resolved_height"] == height, "pass enumeration height mismatch")
    candidates = collect_pages(usdb, "get_candidate_set_view", view, "pass_id", cursor=True)
    passes, breakdowns, ledgers = {}, {}, {}
    all_owners = set(owners)
    for item in recent["items"]:
        pass_id = item["inscription_id"]
        snapshot = usdb("get_pass_snapshot", [{"inscription_id": pass_id, "at_height": height, "context": context}])
        require(isinstance(snapshot, dict), f"missing pass snapshot: {pass_id}")
        require(snapshot["resolved_height"] == height, f"pass snapshot height mismatch: {pass_id}")
        # SQLite sequence numbers are local allocation details, not canonical events.
        snapshot = {k: v for k, v in snapshot.items() if k != "last_event_id"}
        profile = usdb("get_pass_economic_profile", [{**view, "pass_id": pass_id}])
        passes[pass_id] = {"snapshot": snapshot, "profile": profile}
        all_owners.update((snapshot["owner"], snapshot["mint_owner"]))
        if snapshot["pass_kind"] == "standard":
            breakdowns[pass_id] = collect_pages(
                usdb, "get_collab_breakdown", {**view, "leader_pass_id": pass_id, "sort": "collab_pass_id_asc"},
                "collab_pass_id", cursor=True,
            )
        if history:
            query = {"inscription_id": pass_id, "from_height": 1, "to_height": height, "order": "asc"}
            events = collect_pages(usdb, "get_pass_history", query, "event_id")
            all_owners.update(row["owner"] for row in events["items"])
            events["items"] = [{k: v for k, v in row.items() if k != "event_id"} for row in events["items"]]
            energy = collect_pages(usdb, "get_pass_energy_range", query, "record_block_height")
            ledgers[pass_id] = {"history": events, "energy": energy}
    owner_views = {}
    for owner in sorted(all_owners):
        owner_views[owner] = {
            "active": usdb("get_owner_active_pass_at_height", [{"owner": owner, "at_height": height}]),
            "passes": collect_pages(usdb, "get_owner_passes_at_height", {
                "owner": owner, "at_height": height, "order": "asc",
            }, "inscription_id"),
            "balance": balance("get_address_balance", [{"script_hash": owner, "block_height": height}]),
        }
        # This endpoint also returns a snapshot with a local SQLite event id.
        if isinstance(owner_views[owner]["active"], dict):
            owner_views[owner]["active"].pop("last_event_id", None)
        if history:
            owner_views[owner]["balance_history"] = balance("get_address_balance", [{
                "script_hash": owner, "block_range": {"start": 0, "end": height + 1},
            }])
    require(usdb("get_state_ref_at_height", [{"block_height": height}]) == state_ref,
            "indexer state changed during checkpoint")
    require(balance("get_state_ref_at_height", [{"block_height": height}]) == balance_ref,
            "balance state changed during checkpoint")
    return {
        "schema": SCHEMA, "height": height, "block_hash": block_hash,
        "history_included": history, "requested_owners": sorted(set(owners)),
        "state_ref": state_ref, "balance_state_ref": balance_ref,
        "recent": recent, "stats": stats, "passes": passes, "candidates": candidates,
        "breakdowns": breakdowns, "owners": owner_views, "ledgers": ledgers,
    }


def first_difference(expected, actual, path="$"):
    """Return a precise first semantic mismatch, without hiding unexpected fields."""
    if type(expected) is not type(actual):
        return {"path": path, "expected": expected, "actual": actual}
    if isinstance(expected, dict):
        if expected.keys() != actual.keys():
            return {"path": path, "missing": sorted(expected.keys() - actual.keys()),
                    "extra": sorted(actual.keys() - expected.keys())}
        for key in sorted(expected):
            difference = first_difference(expected[key], actual[key], f"{path}.{key}")
            if difference:
                return difference
    elif isinstance(expected, list):
        if len(expected) != len(actual):
            return {"path": path, "expected_length": len(expected), "actual_length": len(actual)}
        for index, (left, right) in enumerate(zip(expected, actual)):
            difference = first_difference(left, right, f"{path}[{index}]")
            if difference:
                return difference
    elif expected != actual:
        return {"path": path, "expected": expected, "actual": actual}
    return None
