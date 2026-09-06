"""Shared completed-workload evidence for world-soak summary regression tests."""

from world_soak_coverage import REQUIRED_ACTIONS


def completed_soak_fixture():
    start = {
        "event": "session_start", "seed": 43, "blocks": 2500,
        "validator_sample_enabled": True, "validator_sample_tamper_enabled": True,
        "validator_sample_mode": "candidate_set", "validator_sample_interval_blocks": 100,
        "agent_self_check_enabled": True, "agent_self_check_interval_blocks": 5,
        "reorg_interval_blocks": 500, "reorg_depth": 3, "reorg_max_events": 4,
    }
    end = {
        "event": "session_end", "completed_work_ticks": 2500, "reorg_events_applied": 4,
        "validator_samples": {"captured": 25, "validated": 25, "pending": 0, "history_validated": 25},
        "finalization": {"work_ticks": 2500, "pending_after": 0, "final_height": 28000},
        "final_metrics": {
            "reorg_ok": 4, "validator_sample_ok": 25, "validator_sample_history_ok": 25,
            "validator_sample_tamper_ok": 25, "agent_energy_check_ok": 50,
            "agent_energy_check_balance_events": 1,
            **{f"{action}_verified": 1 for action in REQUIRED_ACTIONS},
        },
    }
    return start, end


def add_replay_fixture(start, end):
    start.update(replay_check_enabled=True, ts_ms=12345)
    checkpoints = [{
        "kind": "reorg", "tick": 500 * index, "height": 5500 * index,
        "block_hash": f"hash-{index}", "file": f"reorg-{index}.json", "sha256": f"digest-{index}", "passes": 10,
    } for index in range(1, 5)]
    checkpoints.append({
        "kind": "final", "tick": 2500, "height": 28000, "block_hash": "final-hash",
        "file": "final.json", "sha256": "final-digest", "passes": 20,
    })
    end["replay_checkpoints"] = checkpoints
    return {
        "event": "replay_comparison", "schema": "usdb-world-replay:v1", "status": "ok",
        "seed": 43, "session_start_ts_ms": 12345, "completed_work_ticks": 2500,
        "final_height": 28000, "final_hash": "final-hash", "fresh_databases": True,
        "comparisons": [{**item, "actual_sha256": item["sha256"]} for item in checkpoints],
    }
