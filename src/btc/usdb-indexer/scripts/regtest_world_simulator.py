#!/usr/bin/env python3

from __future__ import annotations

import argparse
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from decimal import Decimal
from pathlib import Path
from typing import Any
from urllib import request


class WorldSimError(Exception):
    pass


@dataclass
class Agent:
    wallet_name: str
    receive_address: str


@dataclass
class Args:
    btc_cli: str
    bitcoin_dir: str
    btc_rpc_port: int
    ord_bin: str
    ord_data_dir: str
    ord_server_url: str
    miner_wallet: str
    mining_address: str
    agent_wallets: list[str]
    agent_addresses: list[str]
    balance_history_rpc_url: str
    usdb_rpc_url: str
    sync_timeout_sec: int
    blocks: int
    seed: int
    fee_rate: int
    max_actions_per_block: int
    mint_probability: float
    invalid_mint_probability: float
    transfer_probability: float
    remint_probability: float
    send_probability: float
    spend_probability: float
    sleep_ms_between_blocks: int
    fail_fast: bool
    temp_dir: str

    @property
    def rpc_timeout_sec(self) -> float:
        return 8.0


class RegtestWorldSimulator:
    INSCRIPTION_ID_PATTERN = re.compile(r"([0-9a-f]{64}i\d+)")
    TXID_PATTERN = re.compile(r"\b([0-9a-f]{64})\b")

    def __init__(self, args: Args) -> None:
        if len(args.agent_wallets) != len(args.agent_addresses):
            raise WorldSimError(
                "agent_wallets and agent_addresses length mismatch: "
                f"{len(args.agent_wallets)} != {len(args.agent_addresses)}"
            )
        if len(args.agent_wallets) == 0:
            raise WorldSimError("at least one agent is required")

        self.args = args
        self.rng = random.Random(args.seed)
        self.agents = [
            Agent(wallet_name=w, receive_address=a)
            for w, a in zip(args.agent_wallets, args.agent_addresses)
        ]
        self.pass_owner_by_id: dict[str, str] = {}
        self.metrics = {
            "mint_ok": 0,
            "mint_fail": 0,
            "invalid_mint_ok": 0,
            "invalid_mint_fail": 0,
            "transfer_ok": 0,
            "transfer_fail": 0,
            "remint_ok": 0,
            "remint_fail": 0,
            "send_ok": 0,
            "send_fail": 0,
            "spend_ok": 0,
            "spend_fail": 0,
            "skip": 0,
        }
        self.temp_dir = Path(args.temp_dir)
        self.temp_dir.mkdir(parents=True, exist_ok=True)

    @staticmethod
    def log(message: str) -> None:
        print(f"[usdb-world-sim] {message}", flush=True)

    def run_cmd(self, cmd: list[str]) -> str:
        proc = subprocess.run(cmd, capture_output=True, text=True)
        if proc.returncode != 0:
            raise WorldSimError(
                "command failed: "
                f"cmd={' '.join(cmd)}, exit={proc.returncode}, stderr={proc.stderr.strip()}"
            )
        return proc.stdout.strip()

    def run_btc_cli(self, wallet: str | None, rpc_args: list[str]) -> str:
        cmd = [
            self.args.btc_cli,
            "-regtest",
            f"-datadir={self.args.bitcoin_dir}",
            f"-rpcport={self.args.btc_rpc_port}",
        ]
        if wallet:
            cmd.append(f"-rpcwallet={wallet}")
        cmd.extend(rpc_args)
        return self.run_cmd(cmd)

    def run_ord_wallet(self, wallet_name: str, ord_args: list[str]) -> str:
        cmd = [
            self.args.ord_bin,
            "--regtest",
            "--bitcoin-rpc-url",
            f"http://127.0.0.1:{self.args.btc_rpc_port}",
            "--cookie-file",
            f"{self.args.bitcoin_dir}/regtest/.cookie",
            "--bitcoin-data-dir",
            self.args.bitcoin_dir,
            "--data-dir",
            self.args.ord_data_dir,
            "wallet",
            "--no-sync",
            "--server-url",
            self.args.ord_server_url,
            "--name",
            wallet_name,
        ]
        cmd.extend(ord_args)
        proc = subprocess.run(cmd, capture_output=True, text=True)
        output = f"{proc.stdout}\n{proc.stderr}".strip()
        if proc.returncode != 0:
            raise WorldSimError(
                "ord wallet command failed: "
                f"wallet={wallet_name}, args={ord_args}, output={output}"
            )
        return output

    def rpc_call(
        self,
        url: str,
        method: str,
        params: Any,
        retries: int = 40,
        sleep_sec: float = 0.25,
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
                    body = resp.read().decode("utf-8")
                parsed = json.loads(body)
                if not isinstance(parsed, dict):
                    raise WorldSimError(
                        f"invalid rpc response object: method={method}, body={body}"
                    )
                return parsed
            except Exception as e:  # noqa: PERF203
                last_error = str(e)
                time.sleep(sleep_sec)

        raise WorldSimError(
            f"rpc call failed after retries: url={url}, method={method}, error={last_error}"
        )

    @staticmethod
    def rpc_result(payload: dict[str, Any], method: str) -> Any:
        error = payload.get("error")
        if error is not None:
            raise WorldSimError(f"{method} returned error: {error}")
        return payload.get("result")

    def extract_inscription_id(self, output: str) -> str:
        match = self.INSCRIPTION_ID_PATTERN.search(output)
        if not match:
            raise WorldSimError(f"failed to parse inscription id from output: {output}")
        return match.group(1)

    def extract_txid(self, output: str) -> str:
        match = self.TXID_PATTERN.search(output)
        if not match:
            raise WorldSimError(f"failed to parse txid from output: {output}")
        return match.group(1)

    def wait_service_synced(self, target_height: int) -> None:
        start = time.time()
        while True:
            bh_height = int(
                self.rpc_result(
                    self.rpc_call(
                        self.args.balance_history_rpc_url, "get_block_height", []
                    ),
                    "get_block_height",
                )
                or 0
            )
            usdb_height = self.rpc_result(
                self.rpc_call(self.args.usdb_rpc_url, "get_synced_block_height", []),
                "get_synced_block_height",
            )
            usdb_height_num = 0 if usdb_height is None else int(usdb_height)

            if bh_height >= target_height and usdb_height_num >= target_height:
                return

            if time.time() - start > self.args.sync_timeout_sec:
                raise WorldSimError(
                    "sync timeout: "
                    f"target_height={target_height}, bh_height={bh_height}, usdb_height={usdb_height_num}"
                )
            time.sleep(0.8)

    def random_eth_address(self) -> str:
        return "0x" + "".join(self.rng.choice("0123456789abcdef") for _ in range(40))

    def random_btc_amount(self, min_btc: str, max_btc: str) -> str:
        min_sat = int((Decimal(min_btc) * Decimal("100000000")).to_integral_value())
        max_sat = int((Decimal(max_btc) * Decimal("100000000")).to_integral_value())
        if max_sat <= min_sat:
            sat = min_sat
        else:
            sat = self.rng.randint(min_sat, max_sat)
        amount = Decimal(sat) / Decimal("100000000")
        return f"{amount:.8f}"

    def write_mint_content(
        self, eth_main: str, prev: list[str], invalid_eth: bool = False
    ) -> Path:
        payload = {
            "p": "usdb",
            "op": "mint",
            "eth_main": "0x123" if invalid_eth else eth_main,
            "prev": prev,
        }
        fd, path = tempfile.mkstemp(
            prefix="usdb-world-mint-", suffix=".json", dir=self.temp_dir
        )
        os.close(fd)
        content_path = Path(path)
        content_path.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
        return content_path

    def choose_action(self) -> str:
        p_mint = self.args.mint_probability
        p_invalid_mint = self.args.invalid_mint_probability
        p_transfer = self.args.transfer_probability
        p_remint = self.args.remint_probability
        p_send = self.args.send_probability
        p_spend = self.args.spend_probability
        total = p_mint + p_invalid_mint + p_transfer + p_remint + p_send + p_spend
        if total > 1.0 + 1e-9:
            raise WorldSimError(
                f"invalid operation probabilities, sum must be <= 1.0, got={total}"
            )

        x = self.rng.random()
        cursor = 0.0
        for op_name, prob in [
            ("mint", p_mint),
            ("invalid_mint", p_invalid_mint),
            ("transfer", p_transfer),
            ("remint", p_remint),
            ("send_balance", p_send),
            ("spend_balance", p_spend),
        ]:
            cursor += prob
            if x <= cursor:
                return op_name
        return "noop"

    def op_send_balance(self) -> str:
        agent = self.rng.choice(self.agents)
        amount = self.random_btc_amount("0.01000000", "0.25000000")
        txid = self.run_btc_cli(
            self.args.miner_wallet, ["sendtoaddress", agent.receive_address, amount]
        )
        self.metrics["send_ok"] += 1
        return f"send_balance:{amount}:to={agent.wallet_name}:txid={txid[:12]}"

    def op_spend_balance(self) -> str:
        agent = self.rng.choice(self.agents)
        amount = self.random_btc_amount("0.00100000", "0.05000000")
        destination = self.run_btc_cli(self.args.miner_wallet, ["getnewaddress"])
        txid = self.run_btc_cli(
            agent.wallet_name, ["sendtoaddress", destination, amount]
        )
        self.metrics["spend_ok"] += 1
        return f"spend_balance:{amount}:from={agent.wallet_name}:txid={txid[:12]}"

    def op_mint(
        self,
        invalid_eth: bool = False,
        prev: list[str] | None = None,
        count_as_mint: bool = True,
    ) -> str:
        agent = self.rng.choice(self.agents)
        eth_main = self.random_eth_address()
        content_path = self.write_mint_content(
            eth_main=eth_main,
            prev=prev or [],
            invalid_eth=invalid_eth,
        )
        output = self.run_ord_wallet(
            agent.wallet_name,
            [
                "inscribe",
                "--fee-rate",
                str(self.args.fee_rate),
                "--destination",
                agent.receive_address,
                "--file",
                str(content_path),
            ],
        )
        inscription_id = self.extract_inscription_id(output)
        self.pass_owner_by_id[inscription_id] = agent.wallet_name
        if invalid_eth:
            self.metrics["invalid_mint_ok"] += 1
            return f"invalid_mint:{inscription_id}:owner={agent.wallet_name}"
        if count_as_mint:
            self.metrics["mint_ok"] += 1
        if prev:
            return f"remint_like_mint:{inscription_id}:owner={agent.wallet_name}:prev={prev[0]}"
        return f"mint:{inscription_id}:owner={agent.wallet_name}"

    def op_transfer(self) -> str:
        candidates = list(self.pass_owner_by_id.items())
        if not candidates:
            self.metrics["skip"] += 1
            return "transfer:skip:no_pass"
        inscription_id, from_wallet = self.rng.choice(candidates)
        target_agents = [a for a in self.agents if a.wallet_name != from_wallet]
        if not target_agents:
            self.metrics["skip"] += 1
            return "transfer:skip:no_target"
        target = self.rng.choice(target_agents)
        output = self.run_ord_wallet(
            from_wallet,
            [
                "send",
                "--fee-rate",
                str(self.args.fee_rate),
                target.receive_address,
                inscription_id,
            ],
        )
        txid = self.extract_txid(output)
        self.pass_owner_by_id[inscription_id] = target.wallet_name
        self.metrics["transfer_ok"] += 1
        return (
            f"transfer:{inscription_id}:from={from_wallet}:to={target.wallet_name}:"
            f"txid={txid[:12]}"
        )

    def op_remint(self) -> str:
        if not self.pass_owner_by_id:
            self.metrics["skip"] += 1
            return "remint:skip:no_prev"
        prev_inscription_id = self.rng.choice(list(self.pass_owner_by_id.keys()))
        result = self.op_mint(
            invalid_eth=False, prev=[prev_inscription_id], count_as_mint=False
        )
        self.metrics["remint_ok"] += 1
        return f"remint:prev={prev_inscription_id}:{result}"

    def mine_one_block(self) -> int:
        self.run_btc_cli(
            self.args.miner_wallet,
            ["generatetoaddress", "1", self.args.mining_address],
        )
        return int(self.run_btc_cli(None, ["getblockcount"]))

    def collect_summary(self, block_height: int) -> dict[str, Any]:
        sync_status = self.rpc_result(
            self.rpc_call(self.args.usdb_rpc_url, "get_sync_status", []),
            "get_sync_status",
        )
        pass_stats = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url,
                "get_pass_stats_at_height",
                [{"at_height": block_height}],
            ),
            "get_pass_stats_at_height",
        )
        latest_balance = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url, "get_latest_active_balance_snapshot", []
            ),
            "get_latest_active_balance_snapshot",
        )

        leaderboard_top = self.rpc_result(
            self.rpc_call(
                self.args.usdb_rpc_url,
                "get_pass_energy_leaderboard",
                [{"at_height": block_height, "page": 0, "page_size": 1}],
            ),
            "get_pass_energy_leaderboard",
        )

        top_item = None
        if isinstance(leaderboard_top, dict):
            items = leaderboard_top.get("items") or []
            if items:
                top_item = items[0]

        active_balance_exact = None
        active_balance_error = None
        active_balance_resp = self.rpc_call(
            self.args.usdb_rpc_url,
            "get_active_balance_snapshot",
            [{"block_height": block_height}],
            retries=1,
            sleep_sec=0.1,
        )
        if active_balance_resp.get("error") is not None:
            active_balance_error = active_balance_resp["error"]
        else:
            active_balance_exact = active_balance_resp.get("result")

        return {
            "sync_status": sync_status,
            "pass_stats": pass_stats,
            "latest_balance": latest_balance,
            "top_item": top_item,
            "active_balance_exact": active_balance_exact,
            "active_balance_error": active_balance_error,
        }

    def execute_action(self, action: str) -> str:
        if action == "noop":
            self.metrics["skip"] += 1
            return "noop"
        if action == "send_balance":
            return self.op_send_balance()
        if action == "spend_balance":
            return self.op_spend_balance()
        if action == "mint":
            return self.op_mint(invalid_eth=False, prev=[])
        if action == "invalid_mint":
            return self.op_mint(invalid_eth=True, prev=[])
        if action == "transfer":
            return self.op_transfer()
        if action == "remint":
            return self.op_remint()
        raise WorldSimError(f"unsupported action: {action}")

    def on_action_failed(self, action: str) -> None:
        if action == "send_balance":
            self.metrics["send_fail"] += 1
            return
        if action == "spend_balance":
            self.metrics["spend_fail"] += 1
            return
        if action == "mint":
            self.metrics["mint_fail"] += 1
            return
        if action == "invalid_mint":
            self.metrics["invalid_mint_fail"] += 1
            return
        if action == "transfer":
            self.metrics["transfer_fail"] += 1
            return
        if action == "remint":
            self.metrics["remint_fail"] += 1
            return

    def format_top_energy(self, top_item: dict[str, Any] | None) -> str:
        if not top_item:
            return "-"
        inscription_id = str(top_item.get("inscription_id", "-"))
        energy = top_item.get("energy", "-")
        return f"{inscription_id[:12]}..:{energy}"

    def run(self) -> None:
        self.log(
            "World simulation started: "
            f"seed={self.args.seed}, blocks={self.args.blocks}, agents={len(self.agents)}"
        )
        self.log(
            "Action probabilities: "
            f"mint={self.args.mint_probability}, invalid_mint={self.args.invalid_mint_probability}, "
            f"transfer={self.args.transfer_probability}, remint={self.args.remint_probability}, "
            f"send={self.args.send_probability}, spend={self.args.spend_probability}"
        )

        tick = 0
        while True:
            if self.args.blocks > 0 and tick >= self.args.blocks:
                break

            tick += 1
            action_count = self.rng.randint(0, max(0, self.args.max_actions_per_block))
            action_results: list[str] = []
            action_failed = 0
            for _ in range(action_count):
                action = self.choose_action()
                try:
                    result = self.execute_action(action)
                    action_results.append(result)
                except Exception as e:  # noqa: BLE001
                    action_failed += 1
                    self.on_action_failed(action)
                    self.log(
                        f"WARN action failed: tick={tick}, action={action}, error={e}"
                    )
                    if self.args.fail_fast:
                        raise

            block_height = self.mine_one_block()
            self.wait_service_synced(block_height)
            summary = self.collect_summary(block_height)

            pass_stats = summary["pass_stats"] or {}
            latest_balance = summary["latest_balance"] or {}
            top_energy = self.format_top_energy(summary["top_item"])
            synced_height = (summary["sync_status"] or {}).get("synced_block_height")
            active_count = int(pass_stats.get("active_count", 0))
            total_count = int(pass_stats.get("total_count", 0))
            invalid_count = int(pass_stats.get("invalid_count", 0))
            total_balance = int(latest_balance.get("total_balance", 0))
            active_addresses = int(latest_balance.get("active_address_count", 0))
            self.log(
                "tick_summary: "
                f"tick={tick}, block_height={block_height}, synced_height={synced_height}, "
                f"actions={action_count}, action_failed={action_failed}, "
                f"known_passes={len(self.pass_owner_by_id)}, pass_total={total_count}, "
                f"pass_active={active_count}, pass_invalid={invalid_count}, "
                f"active_addresses={active_addresses}, active_total_balance={total_balance}, "
                f"top_energy={top_energy}"
            )
            if action_results:
                self.log(
                    "tick_actions: "
                    + "; ".join(action_results[:6])
                    + ("; ..." if len(action_results) > 6 else "")
                )

            exact_balance = summary["active_balance_exact"]
            if isinstance(exact_balance, dict):
                exact_active = int(exact_balance.get("active_address_count", 0))
                if exact_active != active_count:
                    self.log(
                        "WARN invariant mismatch: "
                        f"block_height={block_height}, active_pass_count={active_count}, "
                        f"active_balance_address_count={exact_active}"
                    )

            if self.args.sleep_ms_between_blocks > 0:
                time.sleep(self.args.sleep_ms_between_blocks / 1000.0)

        self.log("World simulation completed.")
        self.log(f"final_metrics={json.dumps(self.metrics, sort_keys=True)}")


def parse_args() -> Args:
    parser = argparse.ArgumentParser(
        prog="regtest_world_simulator",
        description="Run continuous random protocol simulation on regtest",
    )
    parser.add_argument("--btc-cli", required=True)
    parser.add_argument("--bitcoin-dir", required=True)
    parser.add_argument("--btc-rpc-port", required=True, type=int)
    parser.add_argument("--ord-bin", required=True)
    parser.add_argument("--ord-data-dir", required=True)
    parser.add_argument("--ord-server-url", required=True)
    parser.add_argument("--miner-wallet", required=True)
    parser.add_argument("--mining-address", required=True)
    parser.add_argument("--agent-wallets", required=True)
    parser.add_argument("--agent-addresses", required=True)
    parser.add_argument("--balance-history-rpc-url", required=True)
    parser.add_argument("--usdb-rpc-url", required=True)
    parser.add_argument("--sync-timeout-sec", type=int, default=300)
    parser.add_argument("--blocks", type=int, default=200)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--fee-rate", type=int, default=1)
    parser.add_argument("--max-actions-per-block", type=int, default=2)
    parser.add_argument("--mint-probability", type=float, default=0.20)
    parser.add_argument("--invalid-mint-probability", type=float, default=0.02)
    parser.add_argument("--transfer-probability", type=float, default=0.20)
    parser.add_argument("--remint-probability", type=float, default=0.10)
    parser.add_argument("--send-probability", type=float, default=0.30)
    parser.add_argument("--spend-probability", type=float, default=0.15)
    parser.add_argument("--sleep-ms-between-blocks", type=int, default=0)
    parser.add_argument("--fail-fast", action="store_true")
    parser.add_argument("--temp-dir", required=True)
    parsed = parser.parse_args()

    agent_wallets = [v for v in parsed.agent_wallets.split(",") if v]
    agent_addresses = [v for v in parsed.agent_addresses.split(",") if v]

    return Args(
        btc_cli=parsed.btc_cli,
        bitcoin_dir=parsed.bitcoin_dir,
        btc_rpc_port=parsed.btc_rpc_port,
        ord_bin=parsed.ord_bin,
        ord_data_dir=parsed.ord_data_dir,
        ord_server_url=parsed.ord_server_url,
        miner_wallet=parsed.miner_wallet,
        mining_address=parsed.mining_address,
        agent_wallets=agent_wallets,
        agent_addresses=agent_addresses,
        balance_history_rpc_url=parsed.balance_history_rpc_url,
        usdb_rpc_url=parsed.usdb_rpc_url,
        sync_timeout_sec=parsed.sync_timeout_sec,
        blocks=parsed.blocks,
        seed=parsed.seed,
        fee_rate=parsed.fee_rate,
        max_actions_per_block=parsed.max_actions_per_block,
        mint_probability=parsed.mint_probability,
        invalid_mint_probability=parsed.invalid_mint_probability,
        transfer_probability=parsed.transfer_probability,
        remint_probability=parsed.remint_probability,
        send_probability=parsed.send_probability,
        spend_probability=parsed.spend_probability,
        sleep_ms_between_blocks=parsed.sleep_ms_between_blocks,
        fail_fast=parsed.fail_fast,
        temp_dir=parsed.temp_dir,
    )


def main() -> int:
    args = parse_args()
    simulator = RegtestWorldSimulator(args)
    try:
        simulator.run()
    except KeyboardInterrupt:
        RegtestWorldSimulator.log("Interrupted by user.")
        return 0
    except WorldSimError as e:
        RegtestWorldSimulator.log(f"Simulation failed: {e}")
        return 1
    except Exception as e:  # noqa: BLE001
        RegtestWorldSimulator.log(f"Unexpected exception: {e}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
