#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
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
    scenario_file: str | None
    skip_initial_usdb_state_assert: bool

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
        "energy_snapshot",
    }

    def __init__(self, args: RunnerArgs) -> None:
        self.args = args
        self.vars: dict[str, Any] = {}

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

    def get_balance_history_height(self) -> int:
        result = self.rpc_result(
            self.rpc_call(self.args.balance_history_rpc_url, "get_block_height", []),
            "get_block_height",
        )
        return int(result or 0)

    def get_usdb_synced_height(self) -> int:
        result = self.rpc_result(
            self.rpc_call(self.args.usdb_rpc_url, "get_synced_block_height", []),
            "get_synced_block_height",
        )
        return 0 if result is None else int(result)

    @staticmethod
    def btc_amount_to_sat(amount_btc: str) -> int:
        amount = Decimal(amount_btc)
        return int((amount * Decimal("100000000")).to_integral_value(rounding=ROUND_DOWN))

    def address_to_script_hash(self, address: str) -> str:
        address_info = json.loads(self.run_btc_cli(["getaddressinfo", address]))
        script_pubkey = address_info["scriptPubKey"]
        script_bytes = bytes.fromhex(script_pubkey)
        return hashlib.sha256(script_bytes).digest()[::-1].hex()

    def resolve_value(self, value: Any) -> Any:
        if isinstance(value, str) and value.startswith("$"):
            return self.resolve_ref(value[1:])
        return value

    def resolve_ref(self, path: str) -> Any:
        parts = path.split(".")
        if not parts:
            raise ScenarioError(f"Invalid reference path: {path}")
        if parts[0] not in self.vars:
            raise ScenarioError(
                f"Unknown scenario variable: {parts[0]}, available={sorted(self.vars.keys())}"
            )
        current: Any = self.vars[parts[0]]
        for part in parts[1:]:
            if isinstance(current, dict):
                if part not in current:
                    raise ScenarioError(
                        f"Invalid scenario reference path: {path}, missing key={part}"
                    )
                current = current[part]
            elif isinstance(current, list):
                try:
                    idx = int(part)
                except ValueError as e:
                    raise ScenarioError(
                        f"Invalid scenario reference path: {path}, list index must be integer, got={part}"
                    ) from e
                if idx < 0 or idx >= len(current):
                    raise ScenarioError(
                        f"Invalid scenario reference path: {path}, list index out of range={idx}"
                    )
                current = current[idx]
            else:
                raise ScenarioError(
                    f"Invalid scenario reference path: {path}, non-dict node at {part}"
                )
        return current

    @staticmethod
    def to_int(value: Any, field: str) -> int:
        try:
            return int(value)
        except Exception as e:  # noqa: BLE001
            raise ScenarioError(f"Invalid integer for {field}: value={value}, error={e}") from e

    def assert_balance_history_balance(
        self, script_hash: str, height: int, expected_sat: int
    ) -> None:
        rows = self.rpc_result(
            self.rpc_call(
                self.args.balance_history_rpc_url,
                "get_address_balance",
                [{"script_hash": script_hash, "block_height": height, "block_range": None}],
            ),
            "get_address_balance",
        )
        got_balance = int(rows[0]["balance"]) if rows else 0
        self.log(
            f"Balance assertion: height={height}, script_hash={script_hash}, expected={expected_sat}, got={got_balance}"
        )
        if got_balance != expected_sat:
            raise ScenarioError(
                f"Balance mismatch at height={height}: expected={expected_sat}, got={got_balance}"
            )

    def get_pass_energy_snapshot(
        self, inscription_id: str, block_height: int, mode: str = "at_or_before"
    ) -> dict[str, Any]:
        result = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url,
                "get_pass_energy",
                [
                    {
                        "inscription_id": inscription_id,
                        "block_height": block_height,
                        "mode": mode,
                    }
                ],
            ),
            "get_pass_energy",
        )
        if not isinstance(result, dict):
            raise ScenarioError(
                f"Invalid get_pass_energy result: inscription_id={inscription_id}, block_height={block_height}, mode={mode}, result={result}"
            )
        if "energy" not in result:
            raise ScenarioError(
                f"Missing energy field in get_pass_energy result: inscription_id={inscription_id}, block_height={block_height}, mode={mode}, result={result}"
            )
        return result

    def assert_pass_energy_eq(
        self,
        inscription_id: str,
        block_height: int,
        expected_energy: int,
        mode: str = "at_or_before",
        expected_state: str | None = None,
        message: str | None = None,
    ) -> None:
        snapshot = self.get_pass_energy_snapshot(inscription_id, block_height, mode)
        got_energy = int(snapshot.get("energy", -1))
        if got_energy != expected_energy:
            detail = (
                "assert_pass_energy_eq failed: "
                f"inscription_id={inscription_id}, block_height={block_height}, mode={mode}, "
                f"got_energy={got_energy}, expected_energy={expected_energy}, snapshot={snapshot}"
            )
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

        if expected_state is not None:
            got_state = str(snapshot.get("state"))
            if got_state != expected_state:
                detail = (
                    "assert_pass_energy_eq state check failed: "
                    f"inscription_id={inscription_id}, block_height={block_height}, mode={mode}, "
                    f"got_state={got_state}, expected_state={expected_state}, snapshot={snapshot}"
                )
                if message:
                    detail = f"{message}: {detail}"
                raise ScenarioError(detail)

    def assert_pass_energy_ge(
        self,
        inscription_id: str,
        block_height: int,
        expected_min_energy: int,
        mode: str = "at_or_before",
        expected_state: str | None = None,
        message: str | None = None,
    ) -> None:
        snapshot = self.get_pass_energy_snapshot(inscription_id, block_height, mode)
        got_energy = int(snapshot.get("energy", -1))
        if got_energy < expected_min_energy:
            detail = (
                "assert_pass_energy_ge failed: "
                f"inscription_id={inscription_id}, block_height={block_height}, mode={mode}, "
                f"got_energy={got_energy}, expected_min_energy={expected_min_energy}, snapshot={snapshot}"
            )
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

        if expected_state is not None:
            got_state = str(snapshot.get("state"))
            if got_state != expected_state:
                detail = (
                    "assert_pass_energy_ge state check failed: "
                    f"inscription_id={inscription_id}, block_height={block_height}, mode={mode}, "
                    f"got_state={got_state}, expected_state={expected_state}, snapshot={snapshot}"
                )
                if message:
                    detail = f"{message}: {detail}"
                raise ScenarioError(detail)

    def assert_pass_energy_delta(
        self,
        inscription_id: str,
        from_height: int,
        to_height: int,
        mode: str = "at_or_before",
        expected_delta: int | None = None,
        min_delta: int | None = None,
        max_delta: int | None = None,
        message: str | None = None,
    ) -> None:
        snapshot_from = self.get_pass_energy_snapshot(inscription_id, from_height, mode)
        snapshot_to = self.get_pass_energy_snapshot(inscription_id, to_height, mode)

        energy_from = int(snapshot_from.get("energy", -1))
        energy_to = int(snapshot_to.get("energy", -1))
        delta = energy_to - energy_from

        if expected_delta is not None and delta != expected_delta:
            detail = (
                "assert_pass_energy_delta exact check failed: "
                f"inscription_id={inscription_id}, mode={mode}, from_height={from_height}, to_height={to_height}, "
                f"energy_from={energy_from}, energy_to={energy_to}, delta={delta}, expected_delta={expected_delta}"
            )
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

        if min_delta is not None and delta < min_delta:
            detail = (
                "assert_pass_energy_delta min check failed: "
                f"inscription_id={inscription_id}, mode={mode}, from_height={from_height}, to_height={to_height}, "
                f"energy_from={energy_from}, energy_to={energy_to}, delta={delta}, min_delta={min_delta}"
            )
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

        if max_delta is not None and delta > max_delta:
            detail = (
                "assert_pass_energy_delta max check failed: "
                f"inscription_id={inscription_id}, mode={mode}, from_height={from_height}, to_height={to_height}, "
                f"energy_from={energy_from}, energy_to={energy_to}, delta={delta}, max_delta={max_delta}"
            )
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

    def send_to_new_address_and_confirm(
        self, amount_btc: str, mine_blocks: int, var_name: str | None
    ) -> dict[str, Any]:
        receiver_address = self.run_btc_cli(["getnewaddress"])
        self.log(f"Sending {amount_btc} BTC to receiver address={receiver_address}")
        txid = self.run_btc_cli(["sendtoaddress", receiver_address, amount_btc])
        self.log(f"Created txid={txid}")

        mining_address = self.args.mining_address or self.run_btc_cli(["getnewaddress"])
        before_height = self.get_balance_history_height()
        self.log(
            f"Mining {mine_blocks} block(s) for confirmation: mining_address={mining_address}"
        )
        self.run_btc_cli(["generatetoaddress", str(mine_blocks), mining_address])
        expected_height = before_height + mine_blocks
        self.wait_balance_history_synced(expected_height)
        self.wait_usdb_synced(expected_height)

        script_hash = self.address_to_script_hash(receiver_address)
        amount_sat = self.btc_amount_to_sat(amount_btc)
        transfer_info = {
            "receiver_address": receiver_address,
            "txid": txid,
            "script_hash": script_hash,
            "amount_btc": amount_btc,
            "amount_sat": amount_sat,
            "confirmed_height": expected_height,
            "mine_blocks": mine_blocks,
        }
        if var_name:
            self.vars[var_name] = transfer_info
            self.log(f"Stored scenario variable: {var_name}")
        return transfer_info

    def mine_blocks(self, blocks: int, var_name: str | None) -> dict[str, Any]:
        if blocks <= 0:
            raise ScenarioError(f"mine_blocks requires blocks > 0, got={blocks}")

        mining_address = self.args.mining_address or self.run_btc_cli(["getnewaddress"])
        before_height = self.get_balance_history_height()
        self.log(f"Mining {blocks} block(s): mining_address={mining_address}")
        self.run_btc_cli(["generatetoaddress", str(blocks), mining_address])

        expected_height = before_height + blocks
        self.wait_balance_history_synced(expected_height)
        self.wait_usdb_synced(expected_height)

        info = {
            "blocks": blocks,
            "mining_address": mining_address,
            "before_height": before_height,
            "after_height": expected_height,
        }
        if var_name:
            self.vars[var_name] = info
            self.log(f"Stored scenario variable: {var_name}")
        return info

    def run_btc_cli_step(
        self,
        cli_args: list[str],
        parse_json: bool,
        var_name: str | None,
    ) -> Any:
        if not cli_args:
            raise ScenarioError("btc_cli step requires non-empty args")
        output = self.run_btc_cli(cli_args)
        result: Any = output
        if parse_json:
            try:
                result = json.loads(output)
            except Exception as e:  # noqa: BLE001
                raise ScenarioError(
                    f"btc_cli step failed to parse JSON output: args={cli_args}, output={output}, error={e}"
                ) from e
        if var_name:
            self.vars[var_name] = result
            self.log(f"Stored scenario variable: {var_name}")
        return result

    def assert_eq(self, left: Any, right: Any, message: str | None = None) -> None:
        if left != right:
            detail = f"assert_eq failed: left={left}, right={right}"
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

    def assert_gt(self, left: Any, right: Any, message: str | None = None) -> None:
        if not left > right:
            detail = f"assert_gt failed: left={left}, right={right}"
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

    def assert_ge(self, left: Any, right: Any, message: str | None = None) -> None:
        if not left >= right:
            detail = f"assert_ge failed: left={left}, right={right}"
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

    def assert_len(self, value: Any, expected_len: int, message: str | None = None) -> None:
        try:
            actual_len = len(value)
        except Exception as e:  # noqa: BLE001
            raise ScenarioError(f"assert_len failed: value has no len(), error={e}") from e
        if actual_len != expected_len:
            detail = (
                f"assert_len failed: actual_len={actual_len}, expected_len={expected_len}"
            )
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

    def assert_contains(
        self, container: Any, item: Any, message: str | None = None
    ) -> None:
        if isinstance(container, dict):
            ok = item in container
        else:
            try:
                ok = item in container
            except Exception as e:  # noqa: BLE001
                raise ScenarioError(
                    f"assert_contains failed: non-iterable container, error={e}"
                ) from e

        if not ok:
            detail = f"assert_contains failed: item={item} not found in container={container}"
            if message:
                detail = f"{message}: {detail}"
            raise ScenarioError(detail)

    def rpc_call_by_service(
        self, service: str, method: str, params: Any
    ) -> dict[str, Any]:
        normalized = service.strip().lower()
        if normalized in {"usdb", "usdb-indexer", "usdb_indexer"}:
            url = self.args.usdb_rpc_url
        elif normalized in {"balance-history", "balance_history", "bh"}:
            url = self.args.balance_history_rpc_url
        else:
            raise ScenarioError(
                f"Unsupported service in assert_rpc_error_code: {service}"
            )
        return self.rpc_call(url, method, params)

    def assert_rpc_error_code(
        self,
        service: str,
        method: str,
        params: Any,
        expected_code: int,
        message_contains: str | None = None,
    ) -> None:
        payload = self.rpc_call_by_service(service, method, params)
        error = payload.get("error")
        if not isinstance(error, dict):
            raise ScenarioError(
                f"assert_rpc_error_code failed: expected error response, got payload={payload}"
            )

        code = error.get("code")
        if int(code) != int(expected_code):
            raise ScenarioError(
                f"assert_rpc_error_code failed: code={code}, expected_code={expected_code}, error={error}"
            )

        if message_contains:
            text = str(error.get("message", ""))
            if message_contains not in text:
                raise ScenarioError(
                    f"assert_rpc_error_code failed: error message does not contain expected text '{message_contains}', message='{text}'"
                )

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
        stable_height = sync_status.get("balance_history_stable_height")
        if synced_height is None or int(synced_height) < expected_height:
            raise ScenarioError(
                f"Synced block height too low: got={synced_height}, expected_at_least={expected_height}"
            )
        if stable_height is None or int(stable_height) < expected_height:
            raise ScenarioError(
                f"Balance-history stable height too low: got={stable_height}, expected_at_least={expected_height}"
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

    def run_scenario_file(self, path: Path) -> None:
        if not path.exists():
            raise ScenarioError(f"Scenario file does not exist: {path}")
        if not path.is_file():
            raise ScenarioError(f"Scenario file path is not a file: {path}")

        try:
            scenario = json.loads(path.read_text(encoding="utf-8"))
        except Exception as e:  # noqa: BLE001
            raise ScenarioError(f"Failed to load scenario file {path}: {e}") from e

        if not isinstance(scenario, dict):
            raise ScenarioError(f"Scenario file must be a JSON object: {path}")

        scenario_name = scenario.get("name", path.name)
        steps = scenario.get("steps")
        if not isinstance(steps, list):
            raise ScenarioError(
                f"Scenario file requires list field 'steps': path={path}"
            )

        self.log(f"Running scenario file: name={scenario_name}, steps={len(steps)}")

        for idx, step in enumerate(steps, start=1):
            if not isinstance(step, dict):
                raise ScenarioError(
                    f"Invalid step format at index={idx}: expected object, got={step}"
                )
            step_type = step.get("type")
            if not isinstance(step_type, str):
                raise ScenarioError(f"Missing step type at index={idx}: step={step}")

            if step_type == "log":
                message = str(self.resolve_value(step.get("message", "")))
                self.log(f"scenario-step[{idx}] log: {message}")
                continue

            if step_type == "set_var":
                var_name = step.get("var")
                if not isinstance(var_name, str) or not var_name:
                    raise ScenarioError(
                        f"set_var requires non-empty string field 'var': step={step}"
                    )
                value = self.resolve_value(step.get("value"))
                self.vars[var_name] = value
                self.log(f"scenario-step[{idx}] set_var var={var_name}")
                continue

            if step_type == "wait_balance_history_synced":
                height = self.to_int(
                    self.resolve_value(step.get("height")), "wait_balance_history_synced.height"
                )
                self.log(f"scenario-step[{idx}] wait_balance_history_synced height={height}")
                self.wait_balance_history_synced(height)
                continue

            if step_type == "wait_usdb_synced":
                height = self.to_int(
                    self.resolve_value(step.get("height")), "wait_usdb_synced.height"
                )
                self.log(f"scenario-step[{idx}] wait_usdb_synced height={height}")
                self.wait_usdb_synced(height)
                continue

            if step_type == "assert_usdb_state":
                height = self.to_int(
                    self.resolve_value(step.get("height")), "assert_usdb_state.height"
                )
                expected_total = self.to_int(
                    self.resolve_value(step.get("expected_total_balance", 0)),
                    "assert_usdb_state.expected_total_balance",
                )
                expected_count = self.to_int(
                    self.resolve_value(step.get("expected_active_count", 0)),
                    "assert_usdb_state.expected_active_count",
                )
                self.log(
                    "scenario-step[%s] assert_usdb_state height=%s total=%s active_count=%s"
                    % (idx, height, expected_total, expected_count)
                )
                self.assert_usdb_state_at_height(height, expected_total, expected_count)
                continue

            if step_type == "mine_blocks":
                blocks = self.to_int(
                    self.resolve_value(step.get("blocks", 1)), "mine_blocks.blocks"
                )
                var_name = step.get("var")
                if var_name is not None and not isinstance(var_name, str):
                    raise ScenarioError(
                        f"mine_blocks.var must be string when provided: step={step}"
                    )
                self.log(
                    f"scenario-step[{idx}] mine_blocks blocks={blocks} var={var_name}"
                )
                self.mine_blocks(blocks, var_name)
                continue

            if step_type == "btc_cli":
                raw_args = step.get("args")
                if not isinstance(raw_args, list):
                    raise ScenarioError(f"btc_cli step requires list field 'args': {step}")
                cli_args = [str(self.resolve_value(item)) for item in raw_args]
                parse_json = bool(self.resolve_value(step.get("parse_json", False)))
                var_name = step.get("var")
                if var_name is not None and not isinstance(var_name, str):
                    raise ScenarioError(
                        f"btc_cli.var must be string when provided: step={step}"
                    )
                self.log(
                    f"scenario-step[{idx}] btc_cli args={cli_args} parse_json={parse_json} var={var_name}"
                )
                self.run_btc_cli_step(cli_args, parse_json, var_name)
                continue

            if step_type == "rpc_call":
                service = str(self.resolve_value(step.get("service", "usdb")))
                method_raw = step.get("method")
                if method_raw is None:
                    raise ScenarioError(f"rpc_call step requires field 'method': {step}")
                method = str(self.resolve_value(method_raw))
                params = self.resolve_value(step.get("params", []))
                result_only = bool(self.resolve_value(step.get("result_only", False)))
                var_name = step.get("var")
                if var_name is not None and not isinstance(var_name, str):
                    raise ScenarioError(
                        f"rpc_call.var must be string when provided: step={step}"
                    )
                self.log(
                    f"scenario-step[{idx}] rpc_call service={service} method={method} result_only={result_only} var={var_name}"
                )
                payload = self.rpc_call_by_service(service, method, params)
                value = self.rpc_result(payload, method) if result_only else payload
                if var_name:
                    self.vars[var_name] = value
                    self.log(f"Stored scenario variable: {var_name}")
                continue

            if step_type == "send_and_confirm":
                amount_btc = str(self.resolve_value(step.get("amount_btc", "0")))
                mine_blocks = self.to_int(
                    self.resolve_value(step.get("mine_blocks", 1)),
                    "send_and_confirm.mine_blocks",
                )
                var_name = step.get("var")
                if var_name is not None and not isinstance(var_name, str):
                    raise ScenarioError(
                        f"send_and_confirm.var must be string when provided: step={step}"
                    )
                self.log(
                    f"scenario-step[{idx}] send_and_confirm amount_btc={amount_btc} mine_blocks={mine_blocks} var={var_name}"
                )
                self.send_to_new_address_and_confirm(amount_btc, mine_blocks, var_name)
                continue

            if step_type == "assert_balance_history_balance":
                script_hash = str(
                    self.resolve_value(step.get("script_hash"))
                )
                height = self.to_int(
                    self.resolve_value(step.get("height")),
                    "assert_balance_history_balance.height",
                )

                if "expected_sat" in step:
                    expected_sat = self.to_int(
                        self.resolve_value(step.get("expected_sat")),
                        "assert_balance_history_balance.expected_sat",
                    )
                elif "expected_amount_btc" in step:
                    expected_sat = self.btc_amount_to_sat(
                        str(self.resolve_value(step.get("expected_amount_btc")))
                    )
                else:
                    raise ScenarioError(
                        "assert_balance_history_balance requires expected_sat or expected_amount_btc"
                    )

                self.log(
                    f"scenario-step[{idx}] assert_balance_history_balance script_hash={script_hash} height={height} expected_sat={expected_sat}"
                )
                self.assert_balance_history_balance(script_hash, height, expected_sat)
                continue

            if step_type == "assert_eq":
                left = self.resolve_value(step.get("left"))
                right = self.resolve_value(step.get("right"))
                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(f"scenario-step[{idx}] assert_eq")
                self.assert_eq(left, right, message)
                continue

            if step_type == "assert_gt":
                left = self.resolve_value(step.get("left"))
                right = self.resolve_value(step.get("right"))
                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(f"scenario-step[{idx}] assert_gt")
                self.assert_gt(left, right, message)
                continue

            if step_type == "assert_ge":
                left = self.resolve_value(step.get("left"))
                right = self.resolve_value(step.get("right"))
                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(f"scenario-step[{idx}] assert_ge")
                self.assert_ge(left, right, message)
                continue

            if step_type == "assert_pass_energy_eq":
                inscription_id = str(self.resolve_value(step.get("inscription_id")))
                block_height = self.to_int(
                    self.resolve_value(step.get("block_height")),
                    "assert_pass_energy_eq.block_height",
                )
                expected_energy = self.to_int(
                    self.resolve_value(step.get("expected_energy")),
                    "assert_pass_energy_eq.expected_energy",
                )
                mode = str(self.resolve_value(step.get("mode", "at_or_before")))
                expected_state = (
                    str(self.resolve_value(step.get("expected_state")))
                    if step.get("expected_state") is not None
                    else None
                )
                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(
                    f"scenario-step[{idx}] assert_pass_energy_eq inscription_id={inscription_id} block_height={block_height} expected_energy={expected_energy} mode={mode}"
                )
                self.assert_pass_energy_eq(
                    inscription_id,
                    block_height,
                    expected_energy,
                    mode,
                    expected_state,
                    message,
                )
                continue

            if step_type == "assert_pass_energy_ge":
                inscription_id = str(self.resolve_value(step.get("inscription_id")))
                block_height = self.to_int(
                    self.resolve_value(step.get("block_height")),
                    "assert_pass_energy_ge.block_height",
                )
                expected_min_energy = self.to_int(
                    self.resolve_value(step.get("expected_min_energy")),
                    "assert_pass_energy_ge.expected_min_energy",
                )
                mode = str(self.resolve_value(step.get("mode", "at_or_before")))
                expected_state = (
                    str(self.resolve_value(step.get("expected_state")))
                    if step.get("expected_state") is not None
                    else None
                )
                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(
                    f"scenario-step[{idx}] assert_pass_energy_ge inscription_id={inscription_id} block_height={block_height} expected_min_energy={expected_min_energy} mode={mode}"
                )
                self.assert_pass_energy_ge(
                    inscription_id,
                    block_height,
                    expected_min_energy,
                    mode,
                    expected_state,
                    message,
                )
                continue

            if step_type == "assert_pass_energy_delta":
                inscription_id = str(self.resolve_value(step.get("inscription_id")))
                from_height = self.to_int(
                    self.resolve_value(step.get("from_height")),
                    "assert_pass_energy_delta.from_height",
                )
                to_height = self.to_int(
                    self.resolve_value(step.get("to_height")),
                    "assert_pass_energy_delta.to_height",
                )
                mode = str(self.resolve_value(step.get("mode", "at_or_before")))

                expected_delta = (
                    self.to_int(
                        self.resolve_value(step.get("expected_delta")),
                        "assert_pass_energy_delta.expected_delta",
                    )
                    if step.get("expected_delta") is not None
                    else None
                )
                min_delta = (
                    self.to_int(
                        self.resolve_value(step.get("min_delta")),
                        "assert_pass_energy_delta.min_delta",
                    )
                    if step.get("min_delta") is not None
                    else None
                )
                max_delta = (
                    self.to_int(
                        self.resolve_value(step.get("max_delta")),
                        "assert_pass_energy_delta.max_delta",
                    )
                    if step.get("max_delta") is not None
                    else None
                )
                if expected_delta is None and min_delta is None and max_delta is None:
                    raise ScenarioError(
                        "assert_pass_energy_delta requires at least one of expected_delta/min_delta/max_delta"
                    )

                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(
                    f"scenario-step[{idx}] assert_pass_energy_delta inscription_id={inscription_id} from_height={from_height} to_height={to_height} mode={mode} expected_delta={expected_delta} min_delta={min_delta} max_delta={max_delta}"
                )
                self.assert_pass_energy_delta(
                    inscription_id,
                    from_height,
                    to_height,
                    mode,
                    expected_delta,
                    min_delta,
                    max_delta,
                    message,
                )
                continue

            if step_type == "assert_len":
                value = self.resolve_value(step.get("value"))
                expected_len = self.to_int(
                    self.resolve_value(step.get("expected_len")),
                    "assert_len.expected_len",
                )
                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(f"scenario-step[{idx}] assert_len expected_len={expected_len}")
                self.assert_len(value, expected_len, message)
                continue

            if step_type == "assert_contains":
                container = self.resolve_value(step.get("container"))
                item = self.resolve_value(step.get("item"))
                message = (
                    str(self.resolve_value(step.get("message")))
                    if step.get("message") is not None
                    else None
                )
                self.log(f"scenario-step[{idx}] assert_contains")
                self.assert_contains(container, item, message)
                continue

            if step_type == "assert_rpc_error_code":
                service = str(self.resolve_value(step.get("service", "usdb")))
                method_raw = step.get("method")
                if method_raw is None:
                    raise ScenarioError(
                        f"assert_rpc_error_code requires field 'method': {step}"
                    )
                method = str(self.resolve_value(method_raw))
                params = self.resolve_value(step.get("params", []))
                expected_code = self.to_int(
                    self.resolve_value(step.get("expected_code")),
                    "assert_rpc_error_code.expected_code",
                )
                message_contains = (
                    str(self.resolve_value(step.get("message_contains")))
                    if step.get("message_contains") is not None
                    else None
                )
                self.log(
                    f"scenario-step[{idx}] assert_rpc_error_code service={service} method={method} expected_code={expected_code}"
                )
                self.assert_rpc_error_code(
                    service, method, params, expected_code, message_contains
                )
                continue

            raise ScenarioError(f"Unsupported scenario step type: {step_type}")

        self.log(f"Scenario file completed: name={scenario_name}")

    def run(self) -> None:
        effective_target_height = self.args.target_height
        if (
            (self.args.enable_transfer_check or self.args.scenario_file is not None)
            and self.args.target_height < self.args.min_spendable_block_height
        ):
            effective_target_height = self.args.min_spendable_block_height
            self.log(
                f"target_height={self.args.target_height} is lower than min_spendable_block_height={self.args.min_spendable_block_height}; "
                f"using effective_target_height={effective_target_height}"
            )

        self.vars["target_height"] = self.args.target_height
        self.vars["effective_target_height"] = effective_target_height
        self.vars["min_spendable_block_height"] = self.args.min_spendable_block_height
        self.vars["send_amount_btc"] = self.args.send_amount_btc
        if self.args.mining_address:
            self.vars["mining_address"] = self.args.mining_address

        self.wait_rpc_ready(
            "balance-history", self.args.balance_history_rpc_url, "get_network_type"
        )
        self.wait_rpc_ready("usdb-indexer", self.args.usdb_rpc_url, "get_network_type")
        self.assert_networks()
        self.assert_usdb_rpc_info()

        self.wait_balance_history_synced(effective_target_height)
        self.wait_usdb_synced(effective_target_height)
        self.vars["synced_height"] = effective_target_height
        if not self.args.skip_initial_usdb_state_assert:
            self.assert_usdb_state_at_height(
                effective_target_height, expected_total_balance=0, expected_active_count=0
            )

        if self.args.scenario_file:
            self.run_scenario_file(Path(self.args.scenario_file))
            self.log("Scenario finished from scenario file.")
            return

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
    parser.add_argument("--scenario-file")
    parser.add_argument("--skip-initial-usdb-state-assert", action="store_true")
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
        scenario_file=parsed.scenario_file,
        skip_initial_usdb_state_assert=parsed.skip_initial_usdb_state_assert,
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
