#!/usr/bin/env python3
"""Read-only P6.5 acceptance of the fixed mainnet 963800 anchor and its live services."""

import argparse
import json
from pathlib import Path

from common.assumeutxo_services import Rpc, capture_anchor


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bh-port", type=int, default=28341)
    parser.add_argument("--indexer-port", type=int, default=28342)
    parser.add_argument("--output", type=Path, required=True, help="New result file; existing files are never overwritten")
    args = parser.parse_args()
    if not __debug__:
        parser.error("Acceptance assertions require Python without -O/PYTHONOPTIMIZE")
    result = dict(status="fail", scope="mainnet_963800_native_projection_and_service_anchor")
    # Reserve the report before RPC work so a mistyped output cannot replace prior evidence.
    with args.output.open("x") as report:
        try:
            bh, indexer = Rpc(args.bh_port), Rpc(args.indexer_port)
            assert bh("get_network_type") == indexer("get_network_type") == "bitcoin", "Both services must use mainnet"
            bootstrap = bh("get_bootstrap_info")
            assert bootstrap and bootstrap["phase"] == "sealed", "Native bootstrap is not sealed"
            origin = bootstrap["origin"]
            assert origin["origin_height"] == 963800, "This probe is pinned to G=963800"
            assert origin["origin_block_hash"] == "000000000000000000012c999b5f6d2043b1d3d76dcf06ee007b5f86290c0551"
            assert origin["utxos"]["rows"] == 165748439, "UTXO row count differs from P4"
            assert origin["utxos"]["sha256"] == "86cba93334b2a6b6862a00d070617a0bdfded56b8eec2a25c41c2dcff82faedd"
            assert origin["balances"]["rows"] == 59356343, "Balance row count differs from P4"
            assert origin["balances"]["sha256"] == "7e9e427332cf8bf95a52cd8689e2b17c732593b69a3f7f062c03dd93ab92449b"
            assert origin["utxos"]["total_sats"] == origin["balances"]["total_sats"]
            assert bootstrap["origin_commit"] == "d5981b06db4e1f9d12f3a9e8b66da45001d0b38d3e2a689de3251542922f1e75"
            checkpoint = Path(__file__).resolve().parents[1] / "src/btc/balance-history/src/bootstrap/checkpoints/mainnet-935000.json"
            assert bootstrap["checkpoint"] == json.loads(checkpoint.read_text())
            anchor = capture_anchor(bh, indexer, 963800)
            assert anchor["balance_history_commit"]["btc_block_hash"] == origin["origin_block_hash"], "Service anchor differs from sealed G hash"
            assert anchor["balance_history_commit"]["block_commit"] == bootstrap["origin_commit"]
            assert anchor["balance_history_commit"]["commit_protocol_version"] == "1.0.0"
            ready = anchor["readiness"]["balance_history"]
            assert ready["balance_query_floor"] == 963800 and ready["history_query_floor"] == 963801
            assert ready["script_registry"]["coverage_mode"] == "post_snapshot_only"
            result.update(status="pass", bootstrap=bootstrap, anchor=anchor,
                          limitation="Projection values read from the sealed verification record; this probe does not rescan the DB or establish an independent mainnet indexer reference")
        except Exception as error:
            result["error"] = repr(error)
            raise
        finally:
            json.dump(result, report, indent=2)
            report.write("\n")
    print(f"P6.5 mainnet anchor passed: {args.output}")


if __name__ == "__main__":
    main()
