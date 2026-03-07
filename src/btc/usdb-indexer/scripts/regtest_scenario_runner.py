#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
import time
from dataclasses import dataclass
from decimal import Decimal, ROUND_DOWN
from typing import Any
from urllib import request


class ScenarioError(Exception):
    pass


@dataclass
class RunnerArgs:
    btc_cli: str
    bitcoin_dir: str
    btc_rpc_port: int
    wallet_name: str
    balance_history_rpc_url: str
    usdb_rpc_url: str
    target_height: int
    sync_timeout_sec: int
    send_amount_btc: str
    min_spendable_block_height: int
    rpc_connect_timeout_sec: float
    rpc_max_time_sec: float
    mining_address: str | None
    enable_transfer_check: bool

    @property
    def rpc_timeout_sec(self) -> float:
        return max(self.rpc_connect_timeout_sec, self.rpc_max_time_sec)


class RegtestScenarioRunner:
    REQUIRED_USDB_FEATURES = {
        "pass_snapshot",
        "active_passes_at_height",
        "invalid_passes",
        "active_balance_snapshot",
        "latest_active_balance_snapshot",
    }

    def __init__(self, args: RunnerArgs) -> None:
        self.args = args

    @staticmethod
    def log(message: str) -> None:
        print(f"[usdb-scenario-runner] {message}", flush=True)

    def run_btc_cli(self, rpc_args: list[str]) -> str:
        cmd = [
            self.args.btc_cli,
            "-regtest",
            f"-datadir={self.args.bitcoin_dir}",
            f"-rpcport={self.args.btc_rpc_port}",
            f"-rpcwallet={self.args.wallet_name}",
            *rpc_args,
        ]
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0:
            raise ScenarioError(
                f"bitcoin-cli failed: cmd={' '.join(cmd)}, stderr={proc.stderr.strip()}"
            )
        return proc.stdout.strip()

    def rpc_call(
        self, url: str, method: str, params: Any, retries: int = 20, sleep_sec: float = 0.2
    ) -> dict[str, Any]:
        payload = json.dumps(
            {"jsonrpc": "2.0", "id": 1, "method": method, "params": params}
        ).encode("utf-8")
        req = request.Request(
            url,
            data=payload,
            headers={"content-type": "application/json"},
            method="POST",
        )

        last_error: str | None = None
        for _ in range(retries):
            try:
                with request.urlopen(req, timeout=self.args.rpc_timeout_sec) as resp:
                    body_text = resp.read().decode("utf-8")
                if not body_text:
                    raise ScenarioError(
                        f"Empty response: url={url}, method={method}, params={params}"
                    )
                payload_obj = json.loads(body_text)
                if not isinstance(payload_obj, dict):
                    raise ScenarioError(
                        f"Invalid JSON-RPC response object: {body_text[:256]}"
                    )
                return payload_obj
            except Exception as e:  # noqa: PERF203
                last_error = str(e)
                time.sleep(sleep_sec)

        raise ScenarioError(
            f"RPC call failed after retries: url={url}, method={method}, params={params}, error={last_error}"
        )

    @staticmethod
    def rpc_result(payload: dict[str, Any], method: str) -> Any:
        if payload.get("error") is not None:
            raise ScenarioError(f"{method} returned error: {payload['error']}")
        return payload.get("result")

    def wait_rpc_ready(self, service_name: str, url: str, method: str) -> None:
        self.log(f"Waiting for {service_name} RPC readiness")
        deadline = time.time() + self.args.sync_timeout_sec
        while time.time() < deadline:
            try:
                self.rpc_call(url, method, [])
                return
            except Exception:
                time.sleep(0.5)
        raise ScenarioError(f"{service_name} RPC not ready in time: url={url}")

    def wait_balance_history_synced(self, target_height: int) -> None:
        self.log(f"Waiting until balance-history synced height >= {target_height}")
        deadline = time.time() + self.args.sync_timeout_sec
        while time.time() < deadline:
            result = self.rpc_result(
                self.rpc_call(
                    self.args.balance_history_rpc_url, "get_block_height", []
                ),
                "get_block_height",
            )
            height = int(result or 0)
            if height >= target_height:
                self.log(f"balance-history synced height={height}")
                return
            time.sleep(1)

        raise ScenarioError(
            f"balance-history sync timeout: target_height={target_height}"
        )

    def wait_usdb_synced(self, target_height: int) -> None:
        self.log(f"Waiting until usdb-indexer synced height >= {target_height}")
        deadline = time.time() + self.args.sync_timeout_sec
        while time.time() < deadline:
            result = self.rpc_result(
                self.rpc_call(self.args.usdb_rpc_url, "get_synced_block_height", []),
                "get_synced_block_height",
            )
            height = 0 if result is None else int(result)
            if height >= target_height:
                self.log(f"usdb-indexer synced height={height}")
                return
            time.sleep(1)

        raise ScenarioError(f"usdb-indexer sync timeout: target_height={target_height}")

    @staticmethod
    def btc_amount_to_sat(amount_btc: str) -> int:
        amount = Decimal(amount_btc)
        return int((amount * Decimal("100000000")).to_integral_value(rounding=ROUND_DOWN))

    def address_to_script_hash(self, address: str) -> str:
        address_info = json.loads(self.run_btc_cli(["getaddressinfo", address]))
        script_pubkey = address_info["scriptPubKey"]
        script_bytes = bytes.fromhex(script_pubkey)
        return hashlib.sha256(script_bytes).digest()[::-1].hex()

    def assert_networks(self) -> None:
        bh_network = self.rpc_result(
            self.rpc_call(self.args.balance_history_rpc_url, "get_network_type", []),
            "get_network_type",
        )
        if bh_network != "regtest":
            raise ScenarioError(f"Unexpected balance-history network: {bh_network}")

        usdb_network = self.rpc_result(
            self.rpc_call(self.args.usdb_rpc_url, "get_network_type", []),
            "get_network_type",
        )
        if usdb_network != "regtest":
            raise ScenarioError(f"Unexpected usdb-indexer network: {usdb_network}")

        self.log("Service network assertions passed: regtest/regtest")

    def assert_usdb_rpc_info(self) -> None:
        result = self.rpc_result(
            self.rpc_call(self.args.usdb_rpc_url, "get_rpc_info", []), "get_rpc_info"
        )
        if not isinstance(result, dict):
            raise ScenarioError(f"Invalid get_rpc_info result: {result}")
        if result.get("service") != "usdb-indexer":
            raise ScenarioError(f"Unexpected get_rpc_info.service: {result}")
        if result.get("network") != "regtest":
            raise ScenarioError(f"Unexpected get_rpc_info.network: {result}")

        features = set(result.get("features") or [])
        missing = sorted(self.REQUIRED_USDB_FEATURES - features)
        if missing:
            raise ScenarioError(f"Missing required usdb features: {missing}")

        self.log("usdb-indexer rpc_info assertion passed.")

    def assert_usdb_state_at_height(
        self, expected_height: int, expected_total_balance: int, expected_active_count: int
    ) -> None:
        sync_status = self.rpc_result(
            self.rpc_call(self.args.usdb_rpc_url, "get_sync_status", []), "get_sync_status"
        )
        if not isinstance(sync_status, dict):
            raise ScenarioError(f"Invalid get_sync_status result: {sync_status}")
        synced_height = sync_status.get("synced_block_height")
        depend_height = sync_status.get("latest_depend_synced_block_height")
        if synced_height is None or int(synced_height) < expected_height:
            raise ScenarioError(
                f"Synced block height too low: got={synced_height}, expected_at_least={expected_height}"
            )
        if depend_height is None or int(depend_height) < expected_height:
            raise ScenarioError(
                f"Dependency synced height too low: got={depend_height}, expected_at_least={expected_height}"
            )

        active_page = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url,
                "get_active_passes_at_height",
                [{"at_height": expected_height, "page": 0, "page_size": 64}],
            ),
            "get_active_passes_at_height",
        )
        items = (active_page or {}).get("items") or []
        resolved_height = int((active_page or {}).get("resolved_height", -1))
        if resolved_height != expected_height:
            raise ScenarioError(
                f"Unexpected active pass resolved_height: got={resolved_height}, expected={expected_height}"
            )
        if len(items) != 0:
            raise ScenarioError(
                f"Expected no active passes at height {expected_height}, got={len(items)}"
            )

        invalid_page = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url,
                "get_invalid_passes",
                [
                    {
                        "error_code": None,
                        "from_height": 0,
                        "to_height": expected_height,
                        "page": 0,
                        "page_size": 64,
                    }
                ],
            ),
            "get_invalid_passes",
        )
        invalid_items = (invalid_page or {}).get("items") or []
        invalid_resolved = int((invalid_page or {}).get("resolved_height", -1))
        if invalid_resolved != expected_height:
            raise ScenarioError(
                f"Unexpected invalid pass resolved_height: got={invalid_resolved}, expected={expected_height}"
            )
        if len(invalid_items) != 0:
            raise ScenarioError(
                f"Expected no invalid passes at height {expected_height}, got={len(invalid_items)}"
            )

        latest_snapshot = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url, "get_latest_active_balance_snapshot", []
            ),
            "get_latest_active_balance_snapshot",
        )
        if latest_snapshot is None:
            raise ScenarioError("Expected latest active balance snapshot, got null")
        latest_height = int(latest_snapshot.get("block_height", -1))
        latest_total = int(latest_snapshot.get("total_balance", -1))
        latest_count = int(latest_snapshot.get("active_address_count", -1))
        if latest_height < expected_height:
            raise ScenarioError(
                f"Latest snapshot height too low: got={latest_height}, expected_at_least={expected_height}"
            )
        if latest_total != expected_total_balance or latest_count != expected_active_count:
            raise ScenarioError(
                "Unexpected latest snapshot values: "
                f"got_total={latest_total}, expected_total={expected_total_balance}, "
                f"got_count={latest_count}, expected_count={expected_active_count}"
            )

        exact_snapshot = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url,
                "get_active_balance_snapshot",
                [{"block_height": expected_height}],
            ),
            "get_active_balance_snapshot",
        )
        exact_height = int((exact_snapshot or {}).get("block_height", -1))
        exact_total = int((exact_snapshot or {}).get("total_balance", -1))
        exact_count = int((exact_snapshot or {}).get("active_address_count", -1))
        if (
            exact_height != expected_height
            or exact_total != expected_total_balance
            or exact_count != expected_active_count
        ):
            raise ScenarioError(
                "Unexpected exact snapshot values: "
                f"height={exact_height}, total={exact_total}, count={exact_count}, "
                f"expected_height={expected_height}, expected_total={expected_total_balance}, expected_count={expected_active_count}"
            )

        self.log(f"usdb-indexer state assertion passed at height={expected_height}.")

    def run(self) -> None:
        effective_target_height = self.args.target_height
        if (
            self.args.enable_transfer_check
            and self.args.target_height < self.args.min_spendable_block_height
        ):
            effective_target_height = self.args.min_spendable_block_height
            self.log(
                f"target_height={self.args.target_height} is lower than min_spendable_block_height={self.args.min_spendable_block_height}; "
                f"using effective_target_height={effective_target_height}"
            )

        self.wait_rpc_ready(
            "balance-history", self.args.balance_history_rpc_url, "get_network_type"
        )
        self.wait_rpc_ready("usdb-indexer", self.args.usdb_rpc_url, "get_network_type")
        self.assert_networks()
        self.assert_usdb_rpc_info()

        self.wait_balance_history_synced(effective_target_height)
        self.wait_usdb_synced(effective_target_height)
        self.assert_usdb_state_at_height(
            effective_target_height, expected_total_balance=0, expected_active_count=0
        )

        if not self.args.enable_transfer_check:
            self.log("Scenario finished without transfer check.")
            return

        receiver_address = self.run_btc_cli(["getnewaddress"])
        self.log(
            f"Sending {self.args.send_amount_btc} BTC to receiver address={receiver_address}"
        )
        txid = self.run_btc_cli(["sendtoaddress", receiver_address, self.args.send_amount_btc])
        self.log(f"Created txid={txid}")

        mining_address = self.args.mining_address or self.run_btc_cli(["getnewaddress"])
        self.log(f"Mining 1 block to confirm transfer: mining_address={mining_address}")
        self.run_btc_cli(["generatetoaddress", "1", mining_address])

        expected_height = effective_target_height + 1
        self.wait_balance_history_synced(expected_height)
        self.wait_usdb_synced(expected_height)

        script_hash = self.address_to_script_hash(receiver_address)
        expected_sat = self.btc_amount_to_sat(self.args.send_amount_btc)

        balance_rows = self.rpc_result(
            self.rpc_call(
                self.args.balance_history_rpc_url,
                "get_address_balance",
                [
                    {
                        "script_hash": script_hash,
                        "block_height": expected_height,
                        "block_range": None,
                    }
                ],
            ),
            "get_address_balance",
        )
        got_balance = int(balance_rows[0]["balance"]) if balance_rows else 0
        self.log(
            f"Balance assertion: height={expected_height}, script_hash={script_hash}, expected={expected_sat}, got={got_balance}"
        )
        if got_balance != expected_sat:
            raise ScenarioError(
                f"Balance mismatch at height={expected_height}: expected={expected_sat}, got={got_balance}"
            )

        self.assert_usdb_state_at_height(
            expected_height, expected_total_balance=0, expected_active_count=0
        )
        self.log("Scenario finished with transfer check.")


def parse_args() -> RunnerArgs:
    parser = argparse.ArgumentParser(
        prog="regtest_scenario_runner",
        description="Run regtest assertions for balance-history + usdb-indexer",
    )
    parser.add_argument("--btc-cli", required=True)
    parser.add_argument("--bitcoin-dir", required=True)
    parser.add_argument("--btc-rpc-port", required=True, type=int)
    parser.add_argument("--wallet-name", required=True)
    parser.add_argument("--balance-history-rpc-url", required=True)
    parser.add_argument("--usdb-rpc-url", required=True)
    parser.add_argument("--target-height", required=True, type=int)
    parser.add_argument("--sync-timeout-sec", default=300, type=int)
    parser.add_argument("--send-amount-btc", default="1.0")
    parser.add_argument("--min-spendable-block-height", default=101, type=int)
    parser.add_argument("--rpc-connect-timeout-sec", default=2.0, type=float)
    parser.add_argument("--rpc-max-time-sec", default=5.0, type=float)
    parser.add_argument("--mining-address")
    parser.add_argument("--enable-transfer-check", action="store_true")
    parsed = parser.parse_args()

    return RunnerArgs(
        btc_cli=parsed.btc_cli,
        bitcoin_dir=parsed.bitcoin_dir,
        btc_rpc_port=parsed.btc_rpc_port,
        wallet_name=parsed.wallet_name,
        balance_history_rpc_url=parsed.balance_history_rpc_url,
        usdb_rpc_url=parsed.usdb_rpc_url,
        target_height=parsed.target_height,
        sync_timeout_sec=parsed.sync_timeout_sec,
        send_amount_btc=parsed.send_amount_btc,
        min_spendable_block_height=parsed.min_spendable_block_height,
        rpc_connect_timeout_sec=parsed.rpc_connect_timeout_sec,
        rpc_max_time_sec=parsed.rpc_max_time_sec,
        mining_address=parsed.mining_address,
        enable_transfer_check=parsed.enable_transfer_check,
    )


def main() -> int:
    args = parse_args()
    runner = RegtestScenarioRunner(args)
    try:
        runner.run()
    except ScenarioError as e:
        RegtestScenarioRunner.log(f"Scenario failed: {e}")
        return 1
    except Exception as e:  # noqa: BLE001
        RegtestScenarioRunner.log(f"Unexpected exception: {e}")
        return 1

    RegtestScenarioRunner.log("Scenario succeeded.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
