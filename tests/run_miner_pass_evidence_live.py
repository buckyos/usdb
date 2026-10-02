#!/usr/bin/env python3
"""Run MinerPass source evidence against a fresh Core with txindex disabled."""

from run_assumeutxo_p64_live import main

if __name__ == "__main__":
    main(test_name="real_core_miner_pass_source_evidence_without_txindex", label="MinerPass evidence", env_prefix="MPE", preflight=False)
