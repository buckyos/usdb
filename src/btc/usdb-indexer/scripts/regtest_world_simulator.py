#!/usr/bin/env python3

from __future__ import annotations

import argparse
import hashlib
import json
import os
import random
import re
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from decimal import Decimal
from pathlib import Path
from typing import Any
from urllib import request


class WorldSimError(Exception):
    pass


@dataclass
class Agent:
    agent_id: int
    wallet_name: str
    receive_address: str
    owner_script_hash: str
    persona: str
    owned_passes: set[str] = field(default_factory=set)
    active_pass_id: str | None = None
    invalid_passes: set[str] = field(default_factory=set)
    last_action: str = "init"
    cooldown: int = 0
    scripted_index: int = 0
    # Per-agent oracle baseline used by self-check diagnostics.
    oracle_last_checked_height: int | None = None
    oracle_last_pass_id: str | None = None
    oracle_last_state: str | None = None
    oracle_last_energy: int | None = None
    oracle_last_owner_balance: int | None = None
    oracle_last_record_block_height: int | None = None


@dataclass
class ActionExpectation:
    action: str
    actor_id: int
    actor_pre_balance: int | None = None
    amount_sat: int | None = None
    inscription_id: str | None = None
    target_id: int | None = None
    target_had_active_before: bool | None = None
    prev_inscription_id: str | None = None
    expect_invalid: bool = False


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
    initial_active_agents: int
    agent_growth_interval_blocks: int
    agent_growth_step: int
    policy_mode: str
    scripted_cycle: list[str]
    report_file: str | None
    report_flush_every: int
    agent_self_check_enabled: bool
    agent_self_check_interval_blocks: int
    agent_self_check_sample_size: int

    @property
    def rpc_timeout_sec(self) -> float:
        return 8.0


class RegtestWorldSimulator:
    INSCRIPTION_ID_PATTERN = re.compile(r"([0-9a-f]{64}i\d+)")
    TXID_PATTERN = re.compile(r"\b([0-9a-f]{64})\b")
    U64_MAX = 2**64 - 1
    ENERGY_BALANCE_THRESHOLD = 100_000
    ENERGY_GROWTH_MULTIPLIER = 10_000
    ENERGY_PENALTY_MULTIPLIER = 43_200_000
    SUPPORTED_ACTIONS = {
        "mint",
        "invalid_mint",
        "transfer",
        "remint",
        "send_balance",
        "spend_balance",
        "noop",
    }
    ORD_TRANSIENT_ERROR_PATTERNS = (
        "output in wallet but not in ord server",
    )

    def __init__(self, args: Args) -> None:
        if len(args.agent_wallets) != len(args.agent_addresses):
            raise WorldSimError(
                "agent_wallets and agent_addresses length mismatch: "
                f"{len(args.agent_wallets)} != {len(args.agent_addresses)}"
            )
        if len(args.agent_wallets) == 0:
            raise WorldSimError("at least one agent is required")

        self.args = args
        if self.args.policy_mode not in {"adaptive", "scripted"}:
            raise WorldSimError(
                f"unsupported policy_mode={self.args.policy_mode}, expected adaptive or scripted"
            )
        if not self.args.scripted_cycle:
            raise WorldSimError("scripted_cycle must not be empty")
        unknown_actions = [
            action for action in self.args.scripted_cycle if action not in self.SUPPORTED_ACTIONS
        ]
        if unknown_actions:
            raise WorldSimError(
                f"unsupported action(s) in scripted_cycle: {unknown_actions}"
            )

        self.rng = random.Random(args.seed)
        self.temp_dir = Path(args.temp_dir)
        self.temp_dir.mkdir(parents=True, exist_ok=True)

        self.agents: list[Agent] = []
        self._init_agents()
        self.total_agents = len(self.agents)
        self.active_agent_count = min(
            self.total_agents, max(1, self.args.initial_active_agents)
        )

        # Global pass ownership index used for candidate selection.
        self.pass_owner_by_id: dict[str, int] = {}

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
            "verify_ok": 0,
            "verify_fail": 0,
            "agent_self_check_ok": 0,
            "agent_self_check_fail": 0,
            "skip": 0,
        }

        self.report_path: Path | None = None
        self.report_fp: Any | None = None
        self.report_event_since_flush = 0
        self._init_reporter()

    def _init_reporter(self) -> None:
        report_file = self.args.report_file
        if report_file is None or str(report_file).strip() == "":
            return
        self.report_path = Path(report_file)
        self.report_path.parent.mkdir(parents=True, exist_ok=True)
        self.report_fp = self.report_path.open("a", encoding="utf-8")
        self.emit_report(
            "session_start",
            {
                "seed": self.args.seed,
                "blocks": self.args.blocks,
                "total_agents": self.total_agents,
                "initial_active_agents": self.active_agent_count,
                "policy_mode": self.args.policy_mode,
                "scripted_cycle": self.args.scripted_cycle,
                "agent_self_check_enabled": self.args.agent_self_check_enabled,
                "agent_self_check_interval_blocks": self.args.agent_self_check_interval_blocks,
                "agent_self_check_sample_size": self.args.agent_self_check_sample_size,
            },
        )

    def emit_report(self, event_type: str, payload: dict[str, Any]) -> None:
        if self.report_fp is None:
            return
        line = {
            "event": event_type,
            "ts_ms": int(time.time() * 1000),
        }
        line.update(payload)
        self.report_fp.write(json.dumps(line, separators=(",", ":")) + "\n")
        self.report_event_since_flush += 1
        flush_every = max(1, self.args.report_flush_every)
        if self.report_event_since_flush >= flush_every:
            self.report_fp.flush()
            self.report_event_since_flush = 0

    def close_report(self) -> None:
        if self.report_fp is None:
            return
        self.report_fp.flush()
        self.report_fp.close()
        self.report_fp = None

    @staticmethod
    def log(message: str) -> None:
        print(f"[usdb-world-sim] {message}", flush=True)

    def _persona_for_agent(self, index: int) -> str:
        if index % 7 == 0:
            return "adversary"
        if index % 3 == 0:
            return "trader"
        if index % 2 == 0:
            return "farmer"
        return "holder"

    def _init_agents(self) -> None:
        for idx, (wallet, address) in enumerate(
            zip(self.args.agent_wallets, self.args.agent_addresses)
        ):
            script_hash = self.address_to_script_hash(address)
            agent = Agent(
                agent_id=idx,
                wallet_name=wallet,
                receive_address=address,
                owner_script_hash=script_hash,
                persona=self._persona_for_agent(idx),
            )
            self.agents.append(agent)

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

        max_attempts = 4
        for attempt in range(1, max_attempts + 1):
            proc = subprocess.run(cmd, capture_output=True, text=True)
            output = f"{proc.stdout}\n{proc.stderr}".strip()
            if proc.returncode == 0:
                return output

            output_lower = output.lower()
            transient = any(
                pattern in output_lower
                for pattern in self.ORD_TRANSIENT_ERROR_PATTERNS
            )
            if transient and attempt < max_attempts:
                backoff_sec = 0.3 * attempt
                self.log(
                    "WARN ord wallet transient error, retrying: "
                    f"wallet={wallet_name}, args={ord_args}, attempt={attempt}/{max_attempts}, "
                    f"backoff_sec={backoff_sec:.1f}, error={output}"
                )
                time.sleep(backoff_sec)
                continue

            raise WorldSimError(
                "ord wallet command failed: "
                f"wallet={wallet_name}, args={ord_args}, output={output}"
            )

        raise WorldSimError(
            "ord wallet command failed after retries: "
            f"wallet={wallet_name}, args={ord_args}"
        )

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

    def rpc_usdb(self, method: str, params: Any) -> Any:
        return self.rpc_result(self.rpc_call(self.args.usdb_rpc_url, method, params), method)

    def rpc_balance_history(self, method: str, params: Any) -> Any:
        return self.rpc_result(
            self.rpc_call(self.args.balance_history_rpc_url, method, params), method
        )

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

    def address_to_script_hash(self, address: str) -> str:
        # `validateaddress` is a non-wallet RPC, avoiding wallet selection errors.
        address_info = json.loads(self.run_btc_cli(None, ["validateaddress", address]))
        script_pubkey = address_info.get("scriptPubKey")
        if not isinstance(script_pubkey, str) or not script_pubkey:
            raise WorldSimError(
                f"validateaddress missing scriptPubKey: address={address}, payload={address_info}"
            )
        script_bytes = bytes.fromhex(script_pubkey)
        return hashlib.sha256(script_bytes).digest()[::-1].hex()

    @staticmethod
    def btc_to_sat(amount_btc: str) -> int:
        amount = Decimal(amount_btc)
        return int((amount * Decimal("100000000")).to_integral_value())

    @classmethod
    def sat_add_u64(cls, left: int, right: int) -> int:
        total = int(left) + int(right)
        return total if total <= cls.U64_MAX else cls.U64_MAX

    @classmethod
    def sat_sub_u64(cls, left: int, right: int) -> int:
        diff = int(left) - int(right)
        return diff if diff > 0 else 0

    @classmethod
    def calc_growth_delta(cls, owner_balance: int, r: int) -> int:
        if owner_balance < cls.ENERGY_BALANCE_THRESHOLD:
            return 0
        raw = int(owner_balance) * cls.ENERGY_GROWTH_MULTIPLIER * int(r)
        return raw if raw <= cls.U64_MAX else cls.U64_MAX

    @classmethod
    def calc_penalty_from_delta(cls, owner_delta: int) -> int:
        if owner_delta >= 0:
            return 0
        raw = abs(int(owner_delta)) * cls.ENERGY_PENALTY_MULTIPLIER
        return raw if raw <= cls.U64_MAX else cls.U64_MAX

    def get_balance_at_height(self, script_hash: str, block_height: int) -> int:
        rows = self.rpc_balance_history(
            "get_address_balance",
            [{"script_hash": script_hash, "block_height": block_height, "block_range": None}],
        )
        if not rows:
            return 0
        return int(rows[0].get("balance", 0))

    def get_owner_active_pass_snapshot(
        self, owner_script_hash: str, at_height: int
    ) -> dict[str, Any] | None:
        result = self.rpc_usdb(
            "get_owner_active_pass_at_height",
            [{"owner": owner_script_hash, "at_height": at_height}],
        )
        return result if isinstance(result, dict) else None

    def get_pass_snapshot(self, inscription_id: str, at_height: int) -> dict[str, Any] | None:
        result = self.rpc_usdb(
            "get_pass_snapshot",
            [{"inscription_id": inscription_id, "at_height": at_height}],
        )
        return result if isinstance(result, dict) else None

    def get_pass_energy_snapshot(
        self, inscription_id: str, block_height: int, mode: str = "at_or_before"
    ) -> dict[str, Any] | None:
        result = self.rpc_usdb(
            "get_pass_energy",
            [
                {
                    "inscription_id": inscription_id,
                    "block_height": block_height,
                    "mode": mode,
                }
            ],
        )
        return result if isinstance(result, dict) else None

    def wait_service_synced(self, target_height: int) -> None:
        start = time.time()
        while True:
            bh_height = int(self.rpc_balance_history("get_block_height", []) or 0)
            usdb_height = self.rpc_usdb("get_synced_block_height", [])
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

    def maybe_grow_agents(self, tick: int) -> None:
        if self.active_agent_count >= self.total_agents:
            return
        if self.args.agent_growth_interval_blocks <= 0:
            return
        if tick % self.args.agent_growth_interval_blocks != 0:
            return

        before = self.active_agent_count
        self.active_agent_count = min(
            self.total_agents, self.active_agent_count + max(1, self.args.agent_growth_step)
        )
        if self.active_agent_count != before:
            self.log(
                "Agent pool expanded: "
                f"tick={tick}, from={before}, to={self.active_agent_count}, total={self.total_agents}"
            )

    def get_active_agent_ids(self) -> list[int]:
        return [agent.agent_id for agent in self.agents[: self.active_agent_count]]

    def choose_actor(self, available_agent_ids: set[int]) -> int:
        # Traders and adversaries act more often.
        weighted: list[tuple[int, float]] = []
        for agent_id in sorted(available_agent_ids):
            agent = self.agents[agent_id]
            weight = 1.0
            if agent.persona == "trader":
                weight *= 1.3
            elif agent.persona == "adversary":
                weight *= 1.15
            elif agent.persona == "holder":
                weight *= 0.9
            weighted.append((agent_id, weight))

        total = sum(weight for _, weight in weighted)
        if total <= 0:
            return self.rng.choice(sorted(available_agent_ids))

        x = self.rng.random() * total
        cursor = 0.0
        for agent_id, weight in weighted:
            cursor += weight
            if x <= cursor:
                return agent_id
        return weighted[-1][0]

    def action_weight_map(
        self, agent: Agent, available_agent_ids: set[int], pre_height: int
    ) -> dict[str, float]:
        global_prob = {
            "mint": self.args.mint_probability,
            "invalid_mint": self.args.invalid_mint_probability,
            "transfer": self.args.transfer_probability,
            "remint": self.args.remint_probability,
            "send_balance": self.args.send_probability,
            "spend_balance": self.args.spend_probability,
        }
        noop_base = max(0.0001, 1.0 - sum(global_prob.values()))

        # Persona weights keep behavioral diversity across agents.
        persona_bias = {
            "holder": {
                "mint": 1.35,
                "invalid_mint": 0.20,
                "transfer": 0.60,
                "remint": 1.00,
                "send_balance": 0.95,
                "spend_balance": 0.80,
                "noop": 1.20,
            },
            "trader": {
                "mint": 0.90,
                "invalid_mint": 0.30,
                "transfer": 1.60,
                "remint": 1.25,
                "send_balance": 1.05,
                "spend_balance": 1.15,
                "noop": 0.70,
            },
            "farmer": {
                "mint": 1.00,
                "invalid_mint": 0.20,
                "transfer": 0.75,
                "remint": 0.90,
                "send_balance": 1.45,
                "spend_balance": 1.30,
                "noop": 0.85,
            },
            "adversary": {
                "mint": 0.95,
                "invalid_mint": 2.20,
                "transfer": 1.10,
                "remint": 1.20,
                "send_balance": 0.90,
                "spend_balance": 1.00,
                "noop": 0.50,
            },
        }[agent.persona]

        weights: dict[str, float] = {
            "mint": global_prob["mint"] * persona_bias["mint"],
            "invalid_mint": global_prob["invalid_mint"] * persona_bias["invalid_mint"],
            "transfer": global_prob["transfer"] * persona_bias["transfer"],
            "remint": global_prob["remint"] * persona_bias["remint"],
            "send_balance": global_prob["send_balance"] * persona_bias["send_balance"],
            "spend_balance": global_prob["spend_balance"] * persona_bias["spend_balance"],
            "noop": noop_base * persona_bias["noop"],
        }

        has_pass = len(agent.owned_passes) > 0
        if not has_pass:
            weights["transfer"] = 0.0
            weights["remint"] = 0.0
            weights["mint"] *= 1.3

        if len(available_agent_ids) < 2:
            weights["transfer"] = 0.0

        if agent.cooldown > 0:
            weights["transfer"] *= 0.65
            weights["remint"] *= 0.65
            weights["mint"] *= 0.75

        # Markov-style transition preference based on last action.
        if agent.last_action == "mint":
            weights["transfer"] *= 1.25
            weights["send_balance"] *= 1.20
            weights["spend_balance"] *= 1.15
        elif agent.last_action == "transfer":
            weights["remint"] *= 1.35
            weights["mint"] *= 1.15
        elif agent.last_action == "spend_balance":
            weights["send_balance"] *= 1.35
            weights["noop"] *= 0.70

        # Basic spendability check avoids pointless spend spam.
        try:
            balance_now = self.get_balance_at_height(agent.owner_script_hash, pre_height)
            if balance_now < 200_000:
                weights["spend_balance"] *= 0.30
        except Exception:
            # Keep simulation moving even if one balance query is transiently unavailable.
            weights["spend_balance"] *= 0.60

        return weights

    def choose_action_for_agent(
        self, agent: Agent, available_agent_ids: set[int], pre_height: int
    ) -> str:
        if self.args.policy_mode == "scripted":
            return self.choose_scripted_action_for_agent(
                agent, available_agent_ids, pre_height
            )

        weights = self.action_weight_map(agent, available_agent_ids, pre_height)
        positive = [(name, w) for name, w in weights.items() if w > 0]
        if not positive:
            return "noop"

        total = sum(w for _, w in positive)
        x = self.rng.random() * total
        cursor = 0.0
        for action, weight in positive:
            cursor += weight
            if x <= cursor:
                return action
        return positive[-1][0]

    def is_action_viable(
        self, agent: Agent, action: str, available_agent_ids: set[int], pre_height: int
    ) -> bool:
        if action == "transfer":
            return len(agent.owned_passes) > 0 and len(available_agent_ids) >= 2
        if action == "remint":
            return len(self.pass_owner_by_id) > 0
        if action == "spend_balance":
            try:
                return self.get_balance_at_height(agent.owner_script_hash, pre_height) >= 200_000
            except Exception:
                return False
        return True

    def choose_scripted_action_for_agent(
        self, agent: Agent, available_agent_ids: set[int], pre_height: int
    ) -> str:
        cycle = self.args.scripted_cycle
        cycle_len = len(cycle)
        start_idx = agent.scripted_index

        for offset in range(cycle_len):
            idx = (start_idx + offset) % cycle_len
            candidate = cycle[idx]
            if self.is_action_viable(agent, candidate, available_agent_ids, pre_height):
                agent.scripted_index = (idx + 1) % cycle_len
                return candidate

        agent.scripted_index = (start_idx + 1) % cycle_len
        return "noop"

    def op_mint(
        self,
        actor: Agent,
        pre_height: int,
        invalid_eth: bool,
        prev: list[str] | None,
        count_as_mint: bool = True,
    ) -> tuple[str, ActionExpectation]:
        content_path = self.write_mint_content(
            eth_main=self.random_eth_address(),
            prev=prev or [],
            invalid_eth=invalid_eth,
        )
        output = self.run_ord_wallet(
            actor.wallet_name,
            [
                "inscribe",
                "--fee-rate",
                str(self.args.fee_rate),
                "--destination",
                actor.receive_address,
                "--file",
                str(content_path),
            ],
        )
        inscription_id = self.extract_inscription_id(output)
        self.pass_owner_by_id[inscription_id] = actor.agent_id
        actor.owned_passes.add(inscription_id)
        if invalid_eth:
            actor.invalid_passes.add(inscription_id)
            self.metrics["invalid_mint_ok"] += 1
            pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
            return (
                f"invalid_mint:{inscription_id}:owner={actor.wallet_name}",
                ActionExpectation(
                    action="invalid_mint",
                    actor_id=actor.agent_id,
                    inscription_id=inscription_id,
                    expect_invalid=True,
                    actor_pre_balance=pre_balance,
                ),
            )

        if count_as_mint:
            self.metrics["mint_ok"] += 1
        pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
        return (
            (
                f"remint_like_mint:{inscription_id}:owner={actor.wallet_name}:prev={prev[0]}"
                if prev
                else f"mint:{inscription_id}:owner={actor.wallet_name}"
            ),
            ActionExpectation(
                action="remint" if prev else "mint",
                actor_id=actor.agent_id,
                inscription_id=inscription_id,
                prev_inscription_id=prev[0] if prev else None,
                actor_pre_balance=pre_balance,
            ),
        )

    def op_transfer(
        self,
        actor: Agent,
        available_agent_ids: set[int],
        pre_height: int,
    ) -> tuple[str, ActionExpectation, set[int]]:
        if not actor.owned_passes:
            self.metrics["skip"] += 1
            return "transfer:skip:no_pass", ActionExpectation("noop", actor.agent_id), {
                actor.agent_id
            }

        target_candidates = [
            self.agents[agent_id]
            for agent_id in sorted(available_agent_ids)
            if agent_id != actor.agent_id
        ]
        if not target_candidates:
            self.metrics["skip"] += 1
            return (
                "transfer:skip:no_target",
                ActionExpectation("noop", actor.agent_id),
                {actor.agent_id},
            )

        inscription_id = self.rng.choice(sorted(actor.owned_passes))
        target = self.rng.choice(target_candidates)
        target_active_before = (
            self.get_owner_active_pass_snapshot(target.owner_script_hash, pre_height) is not None
        )

        output = self.run_ord_wallet(
            actor.wallet_name,
            [
                "send",
                "--fee-rate",
                str(self.args.fee_rate),
                target.receive_address,
                inscription_id,
            ],
        )
        txid = self.extract_txid(output)

        # Update local ownership view immediately; chain finality is validated post-block.
        actor.owned_passes.discard(inscription_id)
        target.owned_passes.add(inscription_id)
        self.pass_owner_by_id[inscription_id] = target.agent_id

        self.metrics["transfer_ok"] += 1
        return (
            (
                f"transfer:{inscription_id}:from={actor.wallet_name}:"
                f"to={target.wallet_name}:txid={txid[:12]}"
            ),
            ActionExpectation(
                action="transfer",
                actor_id=actor.agent_id,
                inscription_id=inscription_id,
                target_id=target.agent_id,
                target_had_active_before=target_active_before,
            ),
            {actor.agent_id, target.agent_id},
        )

    def op_send_balance(self, actor: Agent, pre_height: int) -> tuple[str, ActionExpectation]:
        amount_btc = self.random_btc_amount("0.01000000", "0.25000000")
        txid = self.run_btc_cli(
            self.args.miner_wallet,
            ["sendtoaddress", actor.receive_address, amount_btc],
        )
        amount_sat = self.btc_to_sat(amount_btc)
        pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
        self.metrics["send_ok"] += 1
        return (
            f"send_balance:{amount_btc}:to={actor.wallet_name}:txid={txid[:12]}",
            ActionExpectation(
                action="send_balance",
                actor_id=actor.agent_id,
                actor_pre_balance=pre_balance,
                amount_sat=amount_sat,
            ),
        )

    def op_spend_balance(
        self, actor: Agent, pre_height: int
    ) -> tuple[str, ActionExpectation] | None:
        pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
        if pre_balance < 200_000:
            self.metrics["skip"] += 1
            return None

        # Cap spend amount by current balance with a conservative upper bound.
        max_sat = min(pre_balance // 2, 5_000_000)
        min_sat = min(100_000, max_sat)
        if max_sat <= 0 or min_sat <= 0:
            self.metrics["skip"] += 1
            return None
        if max_sat < min_sat:
            amount_sat = max_sat
        else:
            amount_sat = self.rng.randint(min_sat, max_sat)

        amount_btc = f"{(Decimal(amount_sat) / Decimal('100000000')):.8f}"
        txid = self.run_btc_cli(
            actor.wallet_name,
            ["sendtoaddress", self.args.mining_address, amount_btc],
        )
        self.metrics["spend_ok"] += 1
        return (
            (
                f"spend_balance:{amount_btc}:from={actor.wallet_name}:"
                f"txid={txid[:12]}"
            ),
            ActionExpectation(
                action="spend_balance",
                actor_id=actor.agent_id,
                actor_pre_balance=pre_balance,
                amount_sat=amount_sat,
            ),
        )

    def choose_prev_for_remint(self) -> str | None:
        if not self.pass_owner_by_id:
            return None
        # Prefer non-invalid prev candidates to better reflect valid remint flow.
        non_invalid = [
            inscription_id
            for inscription_id, owner_id in self.pass_owner_by_id.items()
            if inscription_id not in self.agents[owner_id].invalid_passes
        ]
        candidates = non_invalid if non_invalid else list(self.pass_owner_by_id.keys())
        if not candidates:
            return None
        return self.rng.choice(sorted(candidates))

    def execute_agent_action(
        self,
        actor: Agent,
        action: str,
        available_agent_ids: set[int],
        pre_height: int,
    ) -> tuple[str, ActionExpectation | None, set[int]]:
        if action == "noop":
            self.metrics["skip"] += 1
            return "noop", None, {actor.agent_id}

        if action == "mint":
            detail, expectation = self.op_mint(
                actor=actor,
                pre_height=pre_height,
                invalid_eth=False,
                prev=None,
            )
            return detail, expectation, {actor.agent_id}

        if action == "invalid_mint":
            detail, expectation = self.op_mint(
                actor=actor,
                pre_height=pre_height,
                invalid_eth=True,
                prev=None,
            )
            return detail, expectation, {actor.agent_id}

        if action == "transfer":
            return self.op_transfer(actor, available_agent_ids, pre_height)

        if action == "remint":
            prev = self.choose_prev_for_remint()
            if prev is None:
                self.metrics["skip"] += 1
                return "remint:skip:no_prev", None, {actor.agent_id}
            detail, expectation = self.op_mint(
                actor=actor,
                pre_height=pre_height,
                invalid_eth=False,
                prev=[prev],
                count_as_mint=False,
            )
            self.metrics["remint_ok"] += 1
            return f"remint:prev={prev}:{detail}", expectation, {actor.agent_id}

        if action == "send_balance":
            detail, expectation = self.op_send_balance(actor, pre_height)
            return detail, expectation, {actor.agent_id}

        if action == "spend_balance":
            result = self.op_spend_balance(actor, pre_height)
            if result is None:
                return "spend_balance:skip:low_balance", None, {actor.agent_id}
            detail, expectation = result
            return detail, expectation, {actor.agent_id}

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

    def verify_expectation(self, expectation: ActionExpectation, block_height: int) -> None:
        actor = self.agents[expectation.actor_id]

        if expectation.action == "send_balance":
            after_balance = self.get_balance_at_height(actor.owner_script_hash, block_height)
            expected_min = int(expectation.actor_pre_balance or 0) + int(
                expectation.amount_sat or 0
            )
            if after_balance < expected_min:
                raise WorldSimError(
                    "send_balance verification failed: "
                    f"agent={actor.wallet_name}, pre={expectation.actor_pre_balance}, "
                    f"amount={expectation.amount_sat}, after={after_balance}"
                )
            return

        if expectation.action == "spend_balance":
            after_balance = self.get_balance_at_height(actor.owner_script_hash, block_height)
            pre_balance = int(expectation.actor_pre_balance or 0)
            if after_balance >= pre_balance:
                # A spend action can coincide with incoming transfers in the same block,
                # or wallet coin selection can return change to the tracked address.
                # In both cases, strict `after < pre` is not guaranteed.
                self.log(
                    "WARN spend_balance verification relaxed: "
                    f"agent={actor.wallet_name}, pre={pre_balance}, after={after_balance}"
                )
            return

        if expectation.action in {"mint", "invalid_mint", "remint"}:
            inscription_id = expectation.inscription_id
            if inscription_id is None:
                raise WorldSimError("mint-like expectation missing inscription_id")
            snapshot = self.get_pass_snapshot(inscription_id, block_height)
            if snapshot is None:
                raise WorldSimError(
                    f"pass snapshot not found after mint: inscription_id={inscription_id}, height={block_height}"
                )

            state = str(snapshot.get("state"))
            owner = str(snapshot.get("owner"))
            if owner != actor.owner_script_hash:
                raise WorldSimError(
                    "mint-like owner mismatch: "
                    f"inscription_id={inscription_id}, expected_owner={actor.owner_script_hash}, got={owner}"
                )

            if expectation.expect_invalid:
                if state != "invalid":
                    raise WorldSimError(
                        "invalid_mint verification failed: "
                        f"inscription_id={inscription_id}, state={state}"
                    )
                return

            if state not in {"active", "dormant"}:
                raise WorldSimError(
                    "mint/remint verification failed: "
                    f"inscription_id={inscription_id}, state={state}"
                )

            if expectation.action == "remint" and expectation.prev_inscription_id:
                prev = snapshot.get("prev") or []
                if expectation.prev_inscription_id not in prev:
                    raise WorldSimError(
                        "remint verification failed: "
                        f"inscription_id={inscription_id}, prev={prev}, "
                        f"expected_prev={expectation.prev_inscription_id}"
                    )
            return

        if expectation.action == "transfer":
            inscription_id = expectation.inscription_id
            target_id = expectation.target_id
            if inscription_id is None or target_id is None:
                raise WorldSimError("transfer expectation missing fields")

            target = self.agents[target_id]
            snapshot = self.get_pass_snapshot(inscription_id, block_height)
            if snapshot is None:
                raise WorldSimError(
                    f"transfer snapshot missing: inscription_id={inscription_id}, height={block_height}"
                )

            owner = str(snapshot.get("owner"))
            state = str(snapshot.get("state"))
            if owner != target.owner_script_hash:
                raise WorldSimError(
                    "transfer owner mismatch: "
                    f"inscription_id={inscription_id}, expected_owner={target.owner_script_hash}, got={owner}"
                )
            if state not in {"active", "dormant"}:
                raise WorldSimError(
                    "transfer state invalid: "
                    f"inscription_id={inscription_id}, state={state}"
                )
            return

        # noop/unknown do not require verification.

    def refresh_agent_state(self, agent: Agent, block_height: int) -> None:
        active_snapshot = self.get_owner_active_pass_snapshot(
            agent.owner_script_hash, block_height
        )
        if active_snapshot is None:
            agent.active_pass_id = None
            return

        inscription_id = str(active_snapshot.get("inscription_id"))
        agent.active_pass_id = inscription_id
        agent.owned_passes.add(inscription_id)
        self.pass_owner_by_id[inscription_id] = agent.agent_id

    def select_agents_for_self_check(self, active_agent_ids: list[int], tick: int) -> list[int]:
        if not self.args.agent_self_check_enabled:
            return []
        if self.args.agent_self_check_interval_blocks <= 0:
            return []
        if tick % self.args.agent_self_check_interval_blocks != 0:
            return []
        if not active_agent_ids:
            return []

        ordered = sorted(active_agent_ids)
        sample_size = self.args.agent_self_check_sample_size
        if sample_size <= 0 or sample_size >= len(ordered):
            return ordered
        return sorted(self.rng.sample(ordered, sample_size))

    def run_agent_self_check(self, agent: Agent, block_height: int) -> None:
        active_pass_id = agent.active_pass_id
        if active_pass_id is None:
            # No active pass at this height, reset the oracle baseline.
            agent.oracle_last_checked_height = block_height
            agent.oracle_last_pass_id = None
            agent.oracle_last_state = None
            agent.oracle_last_energy = None
            agent.oracle_last_owner_balance = None
            agent.oracle_last_record_block_height = None
            return

        energy_snapshot = self.get_pass_energy_snapshot(
            active_pass_id, block_height, mode="at_or_before"
        )
        if energy_snapshot is None:
            raise WorldSimError(
                "agent self-check missing pass energy snapshot: "
                f"agent={agent.wallet_name}, inscription_id={active_pass_id}, block_height={block_height}"
            )

        query_height = int(energy_snapshot.get("query_block_height", block_height))
        record_block_height = int(
            energy_snapshot.get("record_block_height", query_height)
        )
        state = str(energy_snapshot.get("state", ""))
        owner_address = str(energy_snapshot.get("owner_address", ""))
        owner_balance = int(energy_snapshot.get("owner_balance", 0))
        owner_delta = int(energy_snapshot.get("owner_delta", 0))
        energy = int(energy_snapshot.get("energy", 0))

        if query_height != block_height:
            raise WorldSimError(
                "agent self-check query height mismatch: "
                f"agent={agent.wallet_name}, inscription_id={active_pass_id}, "
                f"expected_query_height={block_height}, got={query_height}"
            )
        if owner_address != agent.owner_script_hash:
            raise WorldSimError(
                "agent self-check owner mismatch: "
                f"agent={agent.wallet_name}, inscription_id={active_pass_id}, "
                f"expected_owner={agent.owner_script_hash}, got_owner={owner_address}"
            )
        if state != "active":
            raise WorldSimError(
                "agent self-check expected active state for owner active pass: "
                f"agent={agent.wallet_name}, inscription_id={active_pass_id}, state={state}, "
                f"record_block_height={record_block_height}, block_height={block_height}"
            )

        prev_height = agent.oracle_last_checked_height
        prev_pass_id = agent.oracle_last_pass_id
        prev_state = agent.oracle_last_state
        prev_energy = agent.oracle_last_energy
        prev_owner_balance = agent.oracle_last_owner_balance

        # Strict numeric oracle when check cadence is consecutive and active pass is stable.
        if (
            prev_height is not None
            and prev_pass_id == active_pass_id
            and prev_state == "active"
            and prev_energy is not None
            and prev_owner_balance is not None
            and block_height == prev_height + 1
        ):
            expected_energy = self.sat_add_u64(
                prev_energy, self.calc_growth_delta(prev_owner_balance, 1)
            )
            if record_block_height == block_height and owner_delta < 0:
                expected_energy = self.sat_sub_u64(
                    expected_energy, self.calc_penalty_from_delta(owner_delta)
                )

            if energy != expected_energy:
                raise WorldSimError(
                    "agent self-check energy mismatch: "
                    f"agent={agent.wallet_name}, inscription_id={active_pass_id}, "
                    f"block_height={block_height}, prev_height={prev_height}, "
                    f"prev_energy={prev_energy}, prev_owner_balance={prev_owner_balance}, "
                    f"record_block_height={record_block_height}, owner_delta={owner_delta}, "
                    f"expected_energy={expected_energy}, actual_energy={energy}"
                )

        agent.oracle_last_checked_height = block_height
        agent.oracle_last_pass_id = active_pass_id
        agent.oracle_last_state = state
        agent.oracle_last_energy = energy
        agent.oracle_last_owner_balance = owner_balance
        agent.oracle_last_record_block_height = record_block_height

    def mine_one_block(self) -> int:
        self.run_btc_cli(
            self.args.miner_wallet,
            ["generatetoaddress", "1", self.args.mining_address],
        )
        return int(self.run_btc_cli(None, ["getblockcount"]))

    def collect_summary(self, block_height: int) -> dict[str, Any]:
        sync_status = self.rpc_usdb("get_sync_status", [])
        pass_stats = self.rpc_usdb(
            "get_pass_stats_at_height",
            [{"at_height": block_height}],
        )
        latest_balance = self.rpc_usdb("get_latest_active_balance_snapshot", [])
        leaderboard_top = self.rpc_usdb(
            "get_pass_energy_leaderboard",
            [{"at_height": block_height, "page": 0, "page_size": 1}],
        )

        top_item = None
        if isinstance(leaderboard_top, dict):
            items = leaderboard_top.get("items") or []
            if items:
                top_item = items[0]

        exact_snapshot = self.rpc_call(
            self.args.usdb_rpc_url,
            "get_active_balance_snapshot",
            [{"block_height": block_height}],
            retries=1,
            sleep_sec=0.1,
        )

        return {
            "sync_status": sync_status,
            "pass_stats": pass_stats,
            "latest_balance": latest_balance,
            "top_item": top_item,
            "active_balance_exact": exact_snapshot.get("result"),
            "active_balance_error": exact_snapshot.get("error"),
        }

    def format_top_energy(self, top_item: dict[str, Any] | None) -> str:
        if not top_item:
            return "-"
        inscription_id = str(top_item.get("inscription_id", "-"))
        energy = top_item.get("energy", "-")
        return f"{inscription_id[:12]}..:{energy}"

    def run(self) -> None:
        self.log(
            "World simulation started: "
            f"seed={self.args.seed}, blocks={self.args.blocks}, total_agents={self.total_agents}, "
            f"initial_active_agents={self.active_agent_count}, policy_mode={self.args.policy_mode}, "
            f"scripted_cycle={self.args.scripted_cycle}, "
            f"agent_self_check_enabled={self.args.agent_self_check_enabled}, "
            f"agent_self_check_interval_blocks={self.args.agent_self_check_interval_blocks}, "
            f"agent_self_check_sample_size={self.args.agent_self_check_sample_size}"
        )
        if self.report_path is not None:
            self.log(f"Structured tick report enabled: path={self.report_path}")

        tick = 0
        while True:
            if self.args.blocks > 0 and tick >= self.args.blocks:
                break

            tick += 1
            self.maybe_grow_agents(tick)
            pre_height = int(self.run_btc_cli(None, ["getblockcount"]))

            active_agent_ids = self.get_active_agent_ids()
            available_ids: set[int] = set(active_agent_ids)
            max_slots = min(self.args.max_actions_per_block, len(available_ids))
            action_slots = self.rng.randint(0, max(0, max_slots))

            action_results: list[str] = []
            expectations: list[ActionExpectation] = []
            action_failed = 0
            action_fail_samples: list[str] = []
            verify_failed = 0
            verify_fail_samples: list[str] = []
            self_check_failed = 0
            self_check_fail_samples: list[str] = []
            self_checked_count = 0
            refresh_failed_agent_ids: set[int] = set()

            for _ in range(action_slots):
                if not available_ids:
                    break

                actor_id = self.choose_actor(available_ids)
                actor = self.agents[actor_id]
                action = self.choose_action_for_agent(actor, available_ids, pre_height)

                try:
                    detail, expectation, used_ids = self.execute_agent_action(
                        actor=actor,
                        action=action,
                        available_agent_ids=available_ids,
                        pre_height=pre_height,
                    )
                    action_results.append(detail)
                    if expectation is not None and expectation.action != "noop":
                        expectations.append(expectation)
                    available_ids -= used_ids
                    actor.last_action = action
                    actor.cooldown = max(0, actor.cooldown - 1)
                except Exception as e:  # noqa: BLE001
                    action_failed += 1
                    self.on_action_failed(action)
                    action_fail_samples.append(
                        f"actor={actor.wallet_name},action={action},error={e}"
                    )
                    self.log(
                        f"WARN action failed: tick={tick}, actor={actor.wallet_name}, action={action}, error={e}"
                    )
                    available_ids.discard(actor_id)
                    actor.last_action = "failed"
                    actor.cooldown = 1
                    if self.args.fail_fast:
                        raise

            block_height = self.mine_one_block()
            self.wait_service_synced(block_height)

            for expectation in expectations:
                try:
                    self.verify_expectation(expectation, block_height)
                    self.metrics["verify_ok"] += 1
                except Exception as e:  # noqa: BLE001
                    self.metrics["verify_fail"] += 1
                    verify_failed += 1
                    verify_fail_samples.append(
                        f"action={expectation.action},actor_id={expectation.actor_id},error={e}"
                    )
                    self.log(
                        "WARN verification failed: "
                        f"tick={tick}, action={expectation.action}, error={e}"
                    )
                    if self.args.fail_fast:
                        raise

            # Refresh views only for active agent pool to keep per-block cost bounded.
            for agent_id in active_agent_ids:
                try:
                    self.refresh_agent_state(self.agents[agent_id], block_height)
                except Exception as e:  # noqa: BLE001
                    refresh_failed_agent_ids.add(agent_id)
                    self.log(
                        f"WARN refresh_agent_state failed: tick={tick}, agent_id={agent_id}, error={e}"
                    )
                    if self.args.fail_fast:
                        raise

            for agent_id in self.select_agents_for_self_check(active_agent_ids, tick):
                if agent_id in refresh_failed_agent_ids:
                    continue
                self_checked_count += 1
                agent = self.agents[agent_id]
                try:
                    self.run_agent_self_check(agent, block_height)
                    self.metrics["agent_self_check_ok"] += 1
                except Exception as e:  # noqa: BLE001
                    self.metrics["agent_self_check_fail"] += 1
                    self_check_failed += 1
                    self_check_fail_samples.append(
                        f"agent={agent.wallet_name},agent_id={agent.agent_id},error={e}"
                    )
                    self.log(
                        "WARN agent self-check failed: "
                        f"tick={tick}, block_height={block_height}, agent={agent.wallet_name}, error={e}"
                    )
                    if self.args.fail_fast:
                        raise

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
                f"active_agent_count={self.active_agent_count}, actions={action_slots}, action_failed={action_failed}, "
                f"agent_self_checked={self_checked_count}, agent_self_check_failed={self_check_failed}, "
                f"known_passes={len(self.pass_owner_by_id)}, pass_total={total_count}, pass_active={active_count}, "
                f"pass_invalid={invalid_count}, active_addresses={active_addresses}, "
                f"active_total_balance={total_balance}, top_energy={top_energy}"
            )

            if action_results:
                self.log(
                    "tick_actions: "
                    + "; ".join(action_results[:6])
                    + ("; ..." if len(action_results) > 6 else "")
                )

            self.emit_report(
                "tick",
                {
                    "tick": tick,
                    "block_height": block_height,
                    "synced_height": synced_height,
                    "active_agent_count": self.active_agent_count,
                    "actions": action_slots,
                    "action_failed": action_failed,
                    "verify_failed": verify_failed,
                    "agent_self_checked": self_checked_count,
                    "agent_self_check_failed": self_check_failed,
                    "known_passes": len(self.pass_owner_by_id),
                    "pass_total": total_count,
                    "pass_active": active_count,
                    "pass_invalid": invalid_count,
                    "active_addresses": active_addresses,
                    "active_total_balance": total_balance,
                    "top_energy": top_energy,
                    "action_results": action_results,
                    "action_fail_samples": action_fail_samples[:8],
                    "verify_fail_samples": verify_fail_samples[:8],
                    "agent_self_check_fail_samples": self_check_fail_samples[:8],
                },
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
        self.emit_report("session_end", {"final_metrics": self.metrics})


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
    parser.add_argument("--initial-active-agents", type=int, default=3)
    parser.add_argument("--agent-growth-interval-blocks", type=int, default=30)
    parser.add_argument("--agent-growth-step", type=int, default=1)
    parser.add_argument("--policy-mode", default="adaptive")
    parser.add_argument(
        "--scripted-cycle",
        default="mint,send_balance,transfer,remint,spend_balance,noop",
    )
    parser.add_argument("--report-file")
    parser.add_argument("--report-flush-every", type=int, default=1)
    parser.add_argument(
        "--disable-agent-self-check",
        action="store_true",
        help="Disable per-agent on-chain self-check diagnostics",
    )
    parser.add_argument(
        "--agent-self-check-interval-blocks",
        type=int,
        default=1,
        help="Run agent self-check every N mined blocks",
    )
    parser.add_argument(
        "--agent-self-check-sample-size",
        type=int,
        default=0,
        help="How many active agents to self-check per run tick (0 means all active agents)",
    )
    parsed = parser.parse_args()

    agent_wallets = [v for v in parsed.agent_wallets.split(",") if v]
    agent_addresses = [v for v in parsed.agent_addresses.split(",") if v]
    scripted_cycle = [v.strip() for v in parsed.scripted_cycle.split(",") if v.strip()]

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
        initial_active_agents=parsed.initial_active_agents,
        agent_growth_interval_blocks=parsed.agent_growth_interval_blocks,
        agent_growth_step=parsed.agent_growth_step,
        policy_mode=parsed.policy_mode,
        scripted_cycle=scripted_cycle,
        report_file=parsed.report_file,
        report_flush_every=parsed.report_flush_every,
        agent_self_check_enabled=(not parsed.disable_agent_self_check),
        agent_self_check_interval_blocks=parsed.agent_self_check_interval_blocks,
        agent_self_check_sample_size=parsed.agent_self_check_sample_size,
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
    finally:
        simulator.close_report()
    return 0


if __name__ == "__main__":
    sys.exit(main())
