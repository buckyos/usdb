"""Capture deterministic committed state and all scenario audits for recovery comparison."""

import argparse
import json
from pathlib import Path
import urllib.request


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--rpc", required=True)
    parser.add_argument("--cases", type=Path, required=True)
    parser.add_argument("--height", type=int, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    def rpc(method, params):
        request = urllib.request.Request(args.rpc, data=json.dumps(dict(
            jsonrpc="2.0", id=1, method=method, params=params,
        )).encode(), headers={"content-type": "application/json"})
        with urllib.request.urlopen(request, timeout=20) as response:
            payload = json.load(response)
        if payload.get("error"):
            raise RuntimeError(f"RPC failed: method={method}, params={params}, error={payload['error']}")
        return payload["result"]

    state = rpc("get_system_state_info", [])
    result = {"height": args.height, "system_state_id": state["system_state_id"], "cases": {}}
    for row in args.cases.read_text().splitlines():
        name, pass_id = row.split("\t")
        params = [dict(inscription_id=pass_id, at_height=args.height)]
        result["cases"][name] = dict(snapshot=rpc("get_pass_snapshot", params),
                                     audit=rpc("get_pass_mint_audit", params))
    args.output.write_text(json.dumps(result, sort_keys=True, indent=2) + "\n")


if __name__ == "__main__":
    main()
