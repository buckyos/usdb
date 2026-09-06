"""Coverage requirements for a completed weekly world-soak workload."""

from typing import Any


REQUIRED_ACTIONS = (
    "standard_mint", "fixed_collab_mint", "address_collab_mint", "invalid_mint",
    "transfer", "standard_remint", "fixed_collab_remint", "address_collab_remint",
    "send_balance", "spend_balance",
)


def check_world_soak_coverage(
    start: dict[str, Any], end: dict[str, Any], blocks: int, seed: int,
) -> dict[str, int]:
    def require(condition: bool, message: str) -> None:
        if not condition:
            raise ValueError(f"world-soak coverage: {message}")

    def count(value: Any, label: str) -> int:
        require(type(value) is int and value >= 0, f"invalid/missing {label}: {value}")
        return value

    require(isinstance(start, dict) and isinstance(end, dict), "missing session boundary")
    require(start.get("seed") == seed and start.get("blocks") == blocks, "workload identity mismatch")
    require(count(end.get("completed_work_ticks"), "completed_work_ticks") == blocks, "incomplete work ticks")
    for field in ("validator_sample_enabled", "validator_sample_tamper_enabled", "agent_self_check_enabled"):
        require(start.get(field) is True, f"required check disabled: {field}")
    require(start.get("validator_sample_mode") == "candidate_set", "candidate-set validation required")
    interval = count(start.get("validator_sample_interval_blocks"), "sample interval")
    self_interval = count(start.get("agent_self_check_interval_blocks"), "self-check interval")
    reorg_interval = count(start.get("reorg_interval_blocks"), "reorg interval")
    require(interval > 0 and self_interval > 0 and reorg_interval > 0, "disabled check cadence")
    require(count(start.get("reorg_depth"), "reorg depth") > 0, "reorg injection disabled")
    reorg_limit = count(start.get("reorg_max_events"), "reorg limit")
    expected_reorgs = blocks // reorg_interval
    if reorg_limit:
        expected_reorgs = min(expected_reorgs, reorg_limit)

    metrics = end.get("final_metrics")
    require(isinstance(metrics, dict), "missing final metrics")
    for name, value in metrics.items():
        count(value, name)
    failures = {name: value for name, value in metrics.items() if name.endswith("_fail") and value}
    require(not failures, f"non-zero failure metrics: {failures}")
    require(count(end.get("reorg_events_applied"), "reorg_events_applied") == expected_reorgs,
            f"expected {expected_reorgs} completed reorgs")
    require(metrics.get("reorg_ok") == expected_reorgs, "reorg success count disagrees with completion")

    samples = end.get("validator_samples")
    require(isinstance(samples, dict), "missing validator sample summary")
    captured = count(samples.get("captured"), "captured samples")
    validated = count(samples.get("validated"), "validated samples")
    require(count(samples.get("pending"), "pending samples") == 0, "unvalidated samples remain")
    require(captured == validated == metrics.get("validator_sample_ok"), "sample accounting mismatch")
    history = count(samples.get("history_validated"), "historical validations")
    require(history <= validated and history == metrics.get("validator_sample_history_ok"),
            "historical validation accounting mismatch")
    finalization = end.get("finalization")
    require(isinstance(finalization, dict), "missing finalization evidence")
    require(finalization.get("work_ticks") == blocks and finalization.get("pending_after") == 0,
            "finalization did not complete the requested workload")

    # Require half of the scheduled historical sampling opportunities, allowing
    # empty candidate sets and reorg-invalidated contexts without vacuous success.
    historical_min = max(1, (blocks // interval) // 2)
    minimums = {
        "validator_sample_history_ok": historical_min,
        "validator_sample_tamper_ok": historical_min,
        "agent_energy_check_ok": max(1, (blocks // self_interval) // 10),
        "agent_energy_check_balance_events": 1,
        **{f"{action}_verified": 1 for action in REQUIRED_ACTIONS},
    }
    for name, minimum in minimums.items():
        actual = count(metrics.get(name), name)
        require(actual >= minimum, f"{name}={actual}, required >= {minimum}")
    return {"reorg_ok": expected_reorgs, **minimums}


def check_world_replay_coverage(start, end, replay):
    """Require this session's successful fresh replay for every saved checkpoint."""
    def require(condition, message):
        if not condition:
            raise ValueError(f"world-replay coverage: {message}")

    require(start.get("replay_check_enabled") is True, "replay disabled")
    require(isinstance(replay, dict) and replay.get("status") == "ok", "no successful replay comparison")
    require(replay.get("schema") == "usdb-world-replay:v1" and replay.get("fresh_databases") is True,
            "fresh replay evidence missing")
    require(replay.get("session_start_ts_ms") == start.get("ts_ms") and replay.get("seed") == start["seed"],
            "replay belongs to a different session")
    require(replay.get("completed_work_ticks") == end["completed_work_ticks"], "replay workload mismatch")
    checkpoints = end.get("replay_checkpoints", [])
    reorgs = [item for item in checkpoints if item.get("kind") == "reorg"]
    finals = [item for item in checkpoints if item.get("kind") == "final"]
    require(len(reorgs) == end["reorg_events_applied"] > 0, "missing post-reorg comparisons")
    require([item["tick"] for item in reorgs] == [
        start["reorg_interval_blocks"] * index for index in range(1, end["reorg_events_applied"] + 1)
    ], "reorg checkpoint cadence mismatch")
    require(len(finals) == 1 and len(checkpoints) == len(reorgs) + 1, "missing final comparison")
    require(finals[0]["tick"] == end["completed_work_ticks"], "final checkpoint tick mismatch")
    require(len({item["file"] for item in checkpoints}) == len(checkpoints), "duplicate checkpoints")
    require(replay.get("final_height") == finals[0]["height"] == end["finalization"]["final_height"],
            "replay final height mismatch")
    require(replay.get("final_hash") == finals[0]["block_hash"], "replay final hash mismatch")
    comparisons = replay.get("comparisons", [])
    require(len(comparisons) == len(checkpoints), "not all checkpoints compared")
    for expected, actual in zip(checkpoints, comparisons):
        require(actual == {**expected, "actual_sha256": expected["sha256"]}, "checkpoint comparison mismatch")
        require(expected.get("passes", 0) > 0, "empty pass comparison")
