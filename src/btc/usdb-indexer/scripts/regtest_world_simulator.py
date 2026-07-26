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
    usdb_main_address: str
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
    oracle_last_active_block_height: int | None = None


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
    pass_kind: str | None = None
    leader_ref_kind: str | None = None
    leader_ref_value: str | None = None
    expect_invalid: bool = False
    action_id: str | None = None


@dataclass(frozen=True)
class PassIdentity:
    pass_kind: str
    leader_ref_kind: str | None = None
    leader_ref_value: str | None = None


@dataclass(frozen=True)
class MintPlan:
    pass_kind: str
    prev_inscription_id: str | None = None
    leader_ref_kind: str | None = None
    leader_ref_value: str | None = None
    leader_agent_id: int | None = None


@dataclass
class ValidatorSampleCandidate:
    inscription_id: str
    owner: str
    state: str
    pass_kind: str
    raw_energy: int
    collab_contribution: int
    effective_energy: int
    level: int
    difficulty_factor_bps: int


@dataclass
class ValidatorSample:
    sample_id: str
    mode: str
    tick: int
    block_height: int
    inscription_id: str
    owner: str
    state: str
    pass_kind: str
    raw_energy: int
    collab_contribution: int
    effective_energy: int
    level: int
    difficulty_factor_bps: int
    snapshot_id: str
    stable_block_hash: str
    local_state_commit: str
    system_state_id: str
    balance_history_api_version: str
    balance_history_semantics_version: str
    activation_registry_id: str
    active_version_set_id: str
    candidates: list[ValidatorSampleCandidate] = field(default_factory=list)
    winner_inscription_id: str | None = None
    expected_consensus_error: str | None = None
    invalidated_by_reorg_tick: int | None = None
    validated: bool = False
    validated_tick: int | None = None


@dataclass
class PlannedAction:
    slot_index: int
    actor_id: int
    action: str
    action_id: str
    probe_state: dict[str, Any] | None = None


@dataclass
class ActionReceipt:
    action_id: str
    action: str
    actor_id: int
    detail: str
    used_agent_ids: list[int]
    expectation: dict[str, Any] | None = None
    metric_deltas: dict[str, int] = field(default_factory=dict)
    local_patch: dict[str, Any] | None = None


@dataclass
class Args:
    btc_cli: str
    bitcoin_dir: str
    btc_rpc_host: str
    btc_rpc_port: int
    btc_auth_mode: str
    btc_cookie_file: str | None
    btc_rpc_user: str | None
    btc_rpc_password: str | None
    ord_bin: str
    ord_data_dir: str
    ord_server_url: str
    miner_wallet: str
    mining_address: str
    agent_wallets: list[str]
    agent_addresses: list[str]
    identity_seed: str | None
    usdb_chain_miner_address: str | None
    usdb_chain_miner_agent_id: int
    balance_history_rpc_url: str
    usdb_indexer_rpc_url: str
    sync_timeout_sec: int
    blocks: int
    seed: int
    fee_rate: int
    max_actions_per_block: int
    standard_mint_probability: float
    fixed_collab_mint_probability: float
    address_collab_mint_probability: float
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
    recovery_state_file: str | None
    agent_self_check_enabled: bool
    agent_self_check_interval_blocks: int
    agent_self_check_sample_size: int
    global_cross_check_enabled: bool
    global_cross_check_interval_blocks: int
    global_cross_check_leaderboard_top_n: int
    global_cross_check_owner_sample_size: int
    economic_page_limit: int
    economic_bootstrap_enabled: bool
    reorg_interval_blocks: int
    reorg_depth: int
    reorg_max_events: int
    validator_sample_enabled: bool
    validator_sample_mode: str
    validator_sample_tamper_enabled: bool
    validator_sample_interval_blocks: int
    validator_sample_size: int
    validator_sample_min_head_advance: int

    @property
    def rpc_timeout_sec(self) -> float:
        return 8.0


class RegtestWorldSimulator:
    INSCRIPTION_ID_PATTERN = re.compile(r"([0-9a-f]{64}i\d+)")
    TXID_PATTERN = re.compile(r"\b([0-9a-f]{64})\b")
    RECOVERY_STATE_VERSION = 3
    ENERGY_MAX = 2**128 - 1
    UNIT_SATS = 100_000
    ENERGY_PER_UNIT_BLOCK = 1
    PENALTY_LAMBDA_NUM = 3
    PENALTY_LAMBDA_DEN = 2
    COLLAB_WEIGHT_BPS = 5_000
    BPS_DENOMINATOR = 10_000
    ECONOMIC_VIEW_VERSION = "uip-0006-usdb-economic-state-view:v1"
    CANDIDATE_SELECTION_RULE = "uip-0006:effective-energy-desc-pass-id-asc:v1"
    SUPPORTED_ACTIONS = {
        "standard_mint",
        "fixed_collab_mint",
        "address_collab_mint",
        "invalid_mint",
        "transfer",
        "standard_remint",
        "fixed_collab_remint",
        "address_collab_remint",
        "send_balance",
        "spend_balance",
        "noop",
    }
    MINT_ACTIONS = {
        "standard_mint",
        "fixed_collab_mint",
        "address_collab_mint",
        "invalid_mint",
    }
    REMINT_ACTIONS = {
        "standard_remint",
        "fixed_collab_remint",
        "address_collab_remint",
    }
    FIXED_COLLAB_ACTIONS = {"fixed_collab_mint", "fixed_collab_remint"}
    ADDRESS_COLLAB_ACTIONS = {"address_collab_mint", "address_collab_remint"}
    ORD_TRANSIENT_ERROR_PATTERNS = (
        "output in wallet but not in ord server",
    )

    @staticmethod
    def choose_candidate_set_winner(
        candidates: list[ValidatorSampleCandidate],
    ) -> ValidatorSampleCandidate:
        if not candidates:
            raise WorldSimError("candidate-set winner selection requires at least one candidate")
        return min(candidates, key=lambda item: (-int(item.effective_energy), item.inscription_id))

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
        if self.args.validator_sample_mode not in {"single", "candidate_set"}:
            raise WorldSimError(
                "unsupported validator_sample_mode="
                f"{self.args.validator_sample_mode}, expected single or candidate_set"
            )
        if not self.args.scripted_cycle:
            raise WorldSimError("scripted_cycle must not be empty")
        if self.args.economic_page_limit <= 0:
            raise WorldSimError("economic_page_limit must be greater than zero")
        unknown_actions = [
            action for action in self.args.scripted_cycle if action not in self.SUPPORTED_ACTIONS
        ]
        if unknown_actions:
            raise WorldSimError(
                f"unsupported action(s) in scripted_cycle: {unknown_actions}"
            )

        self.action_seed = int(args.seed)
        self.diagnostic_seed = int(args.seed) ^ 0xA5A5A5A5
        self.temp_dir = Path(args.temp_dir)
        self.temp_dir.mkdir(parents=True, exist_ok=True)

        self.address_script_hash_cache: dict[str, str] = {}
        self.agents: list[Agent] = []
        self._init_agents()
        self.total_agents = len(self.agents)
        if self.args.usdb_chain_miner_address and not (
            0 <= self.args.usdb_chain_miner_agent_id < self.total_agents
        ):
            raise WorldSimError(
                "usdb_chain_miner_agent_id is out of range for configured agents: "
                f"{self.args.usdb_chain_miner_agent_id} not in [0, {self.total_agents - 1}]"
            )
        self.active_agent_count = min(
            self.total_agents, max(1, self.args.initial_active_agents)
        )
        if self.args.economic_bootstrap_enabled:
            if self.total_agents < 4:
                raise WorldSimError(
                    "economic bootstrap requires at least four configured agents"
                )
            self.active_agent_count = max(self.active_agent_count, 4)

        # Global pass ownership index used for candidate selection.
        self.pass_owner_by_id: dict[str, int] = {}
        self.pass_identity_by_id: dict[str, PassIdentity] = {}

        self.metrics = {
            "standard_mint_ok": 0,
            "standard_mint_fail": 0,
            "fixed_collab_mint_ok": 0,
            "fixed_collab_mint_fail": 0,
            "address_collab_mint_ok": 0,
            "address_collab_mint_fail": 0,
            "invalid_mint_ok": 0,
            "invalid_mint_fail": 0,
            "transfer_ok": 0,
            "transfer_fail": 0,
            "standard_remint_ok": 0,
            "standard_remint_fail": 0,
            "fixed_collab_remint_ok": 0,
            "fixed_collab_remint_fail": 0,
            "address_collab_remint_ok": 0,
            "address_collab_remint_fail": 0,
            "send_ok": 0,
            "send_fail": 0,
            "spend_ok": 0,
            "spend_fail": 0,
            "verify_ok": 0,
            "verify_fail": 0,
            "agent_self_check_ok": 0,
            "agent_self_check_fail": 0,
            "global_cross_check_ok": 0,
            "global_cross_check_fail": 0,
            "reorg_ok": 0,
            "reorg_fail": 0,
            "validator_sample_ok": 0,
            "validator_sample_fail": 0,
            "validator_sample_tamper_ok": 0,
            "validator_sample_tamper_fail": 0,
            "skip": 0,
        }
        self.reorg_events_applied = 0
        self.validator_samples: list[ValidatorSample] = []

        self.report_path: Path | None = None
        self.report_fp: Any | None = None
        self.report_event_since_flush = 0
        self.recovery_state_path: Path | None = None
        self.resume_state: dict[str, Any] | None = None
        self._init_reporter()
        self._init_recovery_state()

    @staticmethod
    def _normalize_seed_component(component: Any) -> str:
        if isinstance(component, bool):
            return "true" if component else "false"
        return str(component)

    def derived_rng(self, namespace: str, *components: Any) -> random.Random:
        material = "::".join(
            [
                namespace,
                *(
                    self._normalize_seed_component(component)
                    for component in components
                ),
            ]
        )
        digest = hashlib.sha256(material.encode("utf-8")).digest()
        return random.Random(int.from_bytes(digest[:16], byteorder="big"))

    def action_position_rng(
        self, tick: int, slot_index: int, phase: str, *components: Any
    ) -> random.Random:
        return self.derived_rng(
            "action",
            self.action_seed,
            tick,
            slot_index,
            phase,
            *components,
        )

    def diagnostic_position_rng(self, scope: str, *components: Any) -> random.Random:
        return self.derived_rng("diag", self.diagnostic_seed, scope, *components)

    def make_action_id(
        self, tick: int, slot_index: int, actor_id: int, action: str
    ) -> str:
        digest = hashlib.sha256(
            "::".join(
                [
                    "action-id",
                    str(self.action_seed),
                    str(tick),
                    str(slot_index),
                    str(actor_id),
                    action,
                ]
            ).encode("utf-8")
        ).hexdigest()[:16]
        return f"t{tick:04d}-s{slot_index:02d}-{digest}"

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
                "action_seed": self.action_seed,
                "diagnostic_seed": self.diagnostic_seed,
                "blocks": self.args.blocks,
                "identity_seed": self.args.identity_seed,
                "usdb_chain_miner_address": self.args.usdb_chain_miner_address,
                "usdb_chain_miner_agent_id": self.args.usdb_chain_miner_agent_id,
                "total_agents": self.total_agents,
                "initial_active_agents": self.active_agent_count,
                "policy_mode": self.args.policy_mode,
                "scripted_cycle": self.args.scripted_cycle,
                "agent_self_check_enabled": self.args.agent_self_check_enabled,
                "agent_self_check_interval_blocks": self.args.agent_self_check_interval_blocks,
                "agent_self_check_sample_size": self.args.agent_self_check_sample_size,
                "global_cross_check_enabled": self.args.global_cross_check_enabled,
                "global_cross_check_interval_blocks": self.args.global_cross_check_interval_blocks,
                "global_cross_check_leaderboard_top_n": self.args.global_cross_check_leaderboard_top_n,
                "global_cross_check_owner_sample_size": self.args.global_cross_check_owner_sample_size,
                "economic_page_limit": self.args.economic_page_limit,
                "economic_bootstrap_enabled": self.args.economic_bootstrap_enabled,
                "reorg_interval_blocks": self.args.reorg_interval_blocks,
                "reorg_depth": self.args.reorg_depth,
                "reorg_max_events": self.args.reorg_max_events,
                "validator_sample_enabled": self.args.validator_sample_enabled,
                "validator_sample_mode": self.args.validator_sample_mode,
                "validator_sample_tamper_enabled": self.args.validator_sample_tamper_enabled,
                "validator_sample_interval_blocks": self.args.validator_sample_interval_blocks,
                "validator_sample_size": self.args.validator_sample_size,
                "validator_sample_min_head_advance": self.args.validator_sample_min_head_advance,
            },
        )

    def _init_recovery_state(self) -> None:
        recovery_state_file = self.args.recovery_state_file
        if recovery_state_file is None or str(recovery_state_file).strip() == "":
            return
        self.recovery_state_path = Path(recovery_state_file)
        self.recovery_state_path.parent.mkdir(parents=True, exist_ok=True)
        self.resume_state = self.load_recovery_state()

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
    def serialize_agent(agent: Agent) -> dict[str, Any]:
        return {
            "agent_id": agent.agent_id,
            "wallet_name": agent.wallet_name,
            "receive_address": agent.receive_address,
            "usdb_main_address": agent.usdb_main_address,
            "owner_script_hash": agent.owner_script_hash,
            "persona": agent.persona,
            "owned_passes": sorted(agent.owned_passes),
            "active_pass_id": agent.active_pass_id,
            "invalid_passes": sorted(agent.invalid_passes),
            "last_action": agent.last_action,
            "cooldown": agent.cooldown,
            "scripted_index": agent.scripted_index,
            "oracle_last_checked_height": agent.oracle_last_checked_height,
            "oracle_last_pass_id": agent.oracle_last_pass_id,
            "oracle_last_state": agent.oracle_last_state,
            "oracle_last_energy": agent.oracle_last_energy,
            "oracle_last_owner_balance": agent.oracle_last_owner_balance,
            "oracle_last_record_block_height": agent.oracle_last_record_block_height,
            "oracle_last_active_block_height": agent.oracle_last_active_block_height,
        }

    @staticmethod
    def apply_agent_state(agent: Agent, payload: dict[str, Any]) -> None:
        agent.usdb_main_address = str(
            payload.get("usdb_main_address", agent.usdb_main_address)
        )
        agent.owned_passes = set(payload.get("owned_passes") or [])
        agent.active_pass_id = payload.get("active_pass_id")
        agent.invalid_passes = set(payload.get("invalid_passes") or [])
        agent.last_action = str(payload.get("last_action", "init"))
        agent.cooldown = int(payload.get("cooldown", 0))
        agent.scripted_index = int(payload.get("scripted_index", 0))
        agent.oracle_last_checked_height = payload.get("oracle_last_checked_height")
        agent.oracle_last_pass_id = payload.get("oracle_last_pass_id")
        agent.oracle_last_state = payload.get("oracle_last_state")
        agent.oracle_last_energy = payload.get("oracle_last_energy")
        agent.oracle_last_owner_balance = payload.get("oracle_last_owner_balance")
        agent.oracle_last_record_block_height = payload.get(
            "oracle_last_record_block_height"
        )
        agent.oracle_last_active_block_height = payload.get(
            "oracle_last_active_block_height"
        )

    @staticmethod
    def serialize_expectation(expectation: ActionExpectation) -> dict[str, Any]:
        return {
            "action": expectation.action,
            "actor_id": expectation.actor_id,
            "actor_pre_balance": expectation.actor_pre_balance,
            "amount_sat": expectation.amount_sat,
            "inscription_id": expectation.inscription_id,
            "target_id": expectation.target_id,
            "target_had_active_before": expectation.target_had_active_before,
            "prev_inscription_id": expectation.prev_inscription_id,
            "pass_kind": expectation.pass_kind,
            "leader_ref_kind": expectation.leader_ref_kind,
            "leader_ref_value": expectation.leader_ref_value,
            "expect_invalid": expectation.expect_invalid,
            "action_id": expectation.action_id,
        }

    @staticmethod
    def deserialize_expectation(payload: dict[str, Any]) -> ActionExpectation:
        return ActionExpectation(
            action=str(payload.get("action", "noop")),
            actor_id=int(payload.get("actor_id", 0)),
            actor_pre_balance=payload.get("actor_pre_balance"),
            amount_sat=payload.get("amount_sat"),
            inscription_id=payload.get("inscription_id"),
            target_id=payload.get("target_id"),
            target_had_active_before=payload.get("target_had_active_before"),
            prev_inscription_id=payload.get("prev_inscription_id"),
            pass_kind=payload.get("pass_kind"),
            leader_ref_kind=payload.get("leader_ref_kind"),
            leader_ref_value=payload.get("leader_ref_value"),
            expect_invalid=bool(payload.get("expect_invalid", False)),
            action_id=payload.get("action_id"),
        )

    @staticmethod
    def serialize_validator_sample_candidate(
        candidate: ValidatorSampleCandidate,
    ) -> dict[str, Any]:
        return {
            "inscription_id": candidate.inscription_id,
            "owner": candidate.owner,
            "state": candidate.state,
            "pass_kind": candidate.pass_kind,
            "raw_energy": candidate.raw_energy,
            "collab_contribution": candidate.collab_contribution,
            "effective_energy": candidate.effective_energy,
            "level": candidate.level,
            "difficulty_factor_bps": candidate.difficulty_factor_bps,
        }

    @staticmethod
    def deserialize_validator_sample_candidate(
        payload: dict[str, Any],
    ) -> ValidatorSampleCandidate:
        return ValidatorSampleCandidate(
            inscription_id=str(payload.get("inscription_id", "")),
            owner=str(payload.get("owner", "")),
            state=str(payload.get("state", "")),
            pass_kind=str(payload.get("pass_kind", "")),
            raw_energy=int(payload.get("raw_energy", 0)),
            collab_contribution=int(payload.get("collab_contribution", 0)),
            effective_energy=int(payload.get("effective_energy", 0)),
            level=int(payload.get("level", 0)),
            difficulty_factor_bps=int(payload.get("difficulty_factor_bps", 0)),
        )

    def serialize_validator_sample(self, sample: ValidatorSample) -> dict[str, Any]:
        return {
            "sample_id": sample.sample_id,
            "mode": sample.mode,
            "tick": sample.tick,
            "block_height": sample.block_height,
            "inscription_id": sample.inscription_id,
            "owner": sample.owner,
            "state": sample.state,
            "pass_kind": sample.pass_kind,
            "raw_energy": sample.raw_energy,
            "collab_contribution": sample.collab_contribution,
            "effective_energy": sample.effective_energy,
            "level": sample.level,
            "difficulty_factor_bps": sample.difficulty_factor_bps,
            "snapshot_id": sample.snapshot_id,
            "stable_block_hash": sample.stable_block_hash,
            "local_state_commit": sample.local_state_commit,
            "system_state_id": sample.system_state_id,
            "balance_history_api_version": sample.balance_history_api_version,
            "balance_history_semantics_version": sample.balance_history_semantics_version,
            "activation_registry_id": sample.activation_registry_id,
            "active_version_set_id": sample.active_version_set_id,
            "candidates": [
                self.serialize_validator_sample_candidate(candidate)
                for candidate in sample.candidates
            ],
            "winner_inscription_id": sample.winner_inscription_id,
            "expected_consensus_error": sample.expected_consensus_error,
            "invalidated_by_reorg_tick": sample.invalidated_by_reorg_tick,
            "validated": sample.validated,
            "validated_tick": sample.validated_tick,
        }

    @staticmethod
    def serialize_planned_action(plan: PlannedAction) -> dict[str, Any]:
        return {
            "slot_index": plan.slot_index,
            "actor_id": plan.actor_id,
            "action": plan.action,
            "action_id": plan.action_id,
            "probe_state": plan.probe_state,
        }

    @staticmethod
    def deserialize_planned_action(payload: dict[str, Any]) -> PlannedAction:
        return PlannedAction(
            slot_index=int(payload.get("slot_index", 0)),
            actor_id=int(payload.get("actor_id", 0)),
            action=str(payload.get("action", "noop")),
            action_id=str(payload.get("action_id", "")),
            probe_state=payload.get("probe_state"),
        )

    def deserialize_validator_sample(
        self, payload: dict[str, Any]
    ) -> ValidatorSample:
        return ValidatorSample(
            sample_id=str(payload.get("sample_id", "")),
            mode=str(payload.get("mode", "")),
            tick=int(payload.get("tick", 0)),
            block_height=int(payload.get("block_height", 0)),
            inscription_id=str(payload.get("inscription_id", "")),
            owner=str(payload.get("owner", "")),
            state=str(payload.get("state", "")),
            pass_kind=str(payload.get("pass_kind", "")),
            raw_energy=int(payload.get("raw_energy", 0)),
            collab_contribution=int(payload.get("collab_contribution", 0)),
            effective_energy=int(payload.get("effective_energy", 0)),
            level=int(payload.get("level", 0)),
            difficulty_factor_bps=int(payload.get("difficulty_factor_bps", 0)),
            snapshot_id=str(payload.get("snapshot_id", "")),
            stable_block_hash=str(payload.get("stable_block_hash", "")),
            local_state_commit=str(payload.get("local_state_commit", "")),
            system_state_id=str(payload.get("system_state_id", "")),
            balance_history_api_version=str(
                payload.get("balance_history_api_version", "")
            ),
            balance_history_semantics_version=str(
                payload.get("balance_history_semantics_version", "")
            ),
            activation_registry_id=str(
                payload.get("activation_registry_id", "")
            ),
            active_version_set_id=str(
                payload.get("active_version_set_id", "")
            ),
            candidates=[
                self.deserialize_validator_sample_candidate(candidate)
                for candidate in (payload.get("candidates") or [])
            ],
            winner_inscription_id=payload.get("winner_inscription_id"),
            expected_consensus_error=payload.get("expected_consensus_error"),
            invalidated_by_reorg_tick=payload.get("invalidated_by_reorg_tick"),
            validated=bool(payload.get("validated", False)),
            validated_tick=payload.get("validated_tick"),
        )

    @staticmethod
    def serialize_action_receipt(receipt: ActionReceipt) -> dict[str, Any]:
        return {
            "action_id": receipt.action_id,
            "action": receipt.action,
            "actor_id": receipt.actor_id,
            "detail": receipt.detail,
            "used_agent_ids": list(receipt.used_agent_ids),
            "expectation": receipt.expectation,
            "metric_deltas": dict(receipt.metric_deltas),
            "local_patch": receipt.local_patch,
        }

    @staticmethod
    def deserialize_action_receipt(payload: dict[str, Any]) -> ActionReceipt:
        return ActionReceipt(
            action_id=str(payload.get("action_id", "")),
            action=str(payload.get("action", "noop")),
            actor_id=int(payload.get("actor_id", 0)),
            detail=str(payload.get("detail", "")),
            used_agent_ids=[int(agent_id) for agent_id in (payload.get("used_agent_ids") or [])],
            expectation=payload.get("expectation"),
            metric_deltas={
                str(key): int(value)
                for key, value in (payload.get("metric_deltas") or {}).items()
            },
            local_patch=payload.get("local_patch"),
        )

    def build_recovery_snapshot(
        self,
        *,
        status: str,
        batch_seed: int,
        tick: int,
        next_slot_index: int,
        action_slots: int,
        pre_height: int,
        active_agent_count: int,
        available_ids: set[int],
        action_results: list[str],
        action_trace_samples: list[dict[str, Any]],
        tick_action_type_counts: dict[str, int],
        current_slot_plan: PlannedAction | None,
        current_slot_receipt: ActionReceipt | None,
        expectations: list[ActionExpectation],
        action_failed: int,
        action_fail_samples: list[str],
    ) -> dict[str, Any]:
        return {
            "version": self.RECOVERY_STATE_VERSION,
            "status": status,
            "seed": self.action_seed,
            "batch_seed": batch_seed,
            "batch_blocks": self.args.blocks,
            "tick": tick,
            "next_slot_index": next_slot_index,
            "action_slots": action_slots,
            "pre_height": pre_height,
            "active_agent_count": active_agent_count,
            "available_ids": sorted(available_ids),
            "action_results": list(action_results),
            "action_trace_samples": list(action_trace_samples),
            "tick_action_type_counts": dict(tick_action_type_counts),
            "current_slot_plan": (
                self.serialize_planned_action(current_slot_plan)
                if current_slot_plan is not None
                else None
            ),
            "current_slot_receipt": (
                self.serialize_action_receipt(current_slot_receipt)
                if current_slot_receipt is not None
                else None
            ),
            "expectations": [
                self.serialize_expectation(expectation)
                for expectation in expectations
            ],
            "action_failed": action_failed,
            "action_fail_samples": list(action_fail_samples),
            "metrics": dict(self.metrics),
            "reorg_events_applied": self.reorg_events_applied,
            "pass_owner_by_id": {
                inscription_id: int(owner_id)
                for inscription_id, owner_id in self.pass_owner_by_id.items()
            },
            "pass_identity_by_id": {
                inscription_id: {
                    "pass_kind": identity.pass_kind,
                    "leader_ref_kind": identity.leader_ref_kind,
                    "leader_ref_value": identity.leader_ref_value,
                }
                for inscription_id, identity in self.pass_identity_by_id.items()
            },
            "agents": [self.serialize_agent(agent) for agent in self.agents],
            "validator_samples": [
                self.serialize_validator_sample(sample)
                for sample in self.validator_samples
            ],
        }

    def build_between_ticks_snapshot(
        self, *, batch_seed: int, next_tick: int, current_height: int
    ) -> dict[str, Any]:
        return {
            "version": 1,
            "status": "between_ticks",
            "seed": self.action_seed,
            "batch_seed": batch_seed,
            "batch_blocks": self.args.blocks,
            "next_tick": next_tick,
            "current_height": current_height,
            "active_agent_count": self.active_agent_count,
            "metrics": dict(self.metrics),
            "reorg_events_applied": self.reorg_events_applied,
            "pass_owner_by_id": {
                inscription_id: int(owner_id)
                for inscription_id, owner_id in self.pass_owner_by_id.items()
            },
            "pass_identity_by_id": {
                inscription_id: {
                    "pass_kind": identity.pass_kind,
                    "leader_ref_kind": identity.leader_ref_kind,
                    "leader_ref_value": identity.leader_ref_value,
                }
                for inscription_id, identity in self.pass_identity_by_id.items()
            },
            "agents": [self.serialize_agent(agent) for agent in self.agents],
            "validator_samples": [
                self.serialize_validator_sample(sample)
                for sample in self.validator_samples
            ],
        }

    def write_recovery_state(self, payload: dict[str, Any]) -> None:
        if self.recovery_state_path is None:
            return
        payload = dict(payload)
        payload["updated_at"] = int(time.time())
        self.recovery_state_path.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    def clear_recovery_state(self) -> None:
        if self.recovery_state_path is None:
            return
        try:
            self.recovery_state_path.unlink()
        except FileNotFoundError:
            pass

    def external_result_path(self, action_id: str) -> Path | None:
        if self.recovery_state_path is None:
            return None
        return self.recovery_state_path.parent / f"external-{action_id}.json"

    def write_external_action_result(
        self,
        *,
        action_id: str,
        action: str,
        raw_output: str,
    ) -> None:
        result_path = self.external_result_path(action_id)
        if result_path is None:
            return
        payload = {
            "version": 1,
            "action_id": action_id,
            "action": action,
            "raw_output": raw_output,
            "updated_at": int(time.time()),
        }
        result_path.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )

    def list_wallet_transactions(
        self, wallet: str, limit: int = 200
    ) -> list[dict[str, Any]]:
        payload = self.run_btc_cli(
            wallet,
            [
                "listtransactions",
                "*",
                str(limit),
                "0",
                "true",
            ],
        )
        parsed = json.loads(payload)
        if not isinstance(parsed, list):
            raise WorldSimError(
                f"listtransactions returned non-list payload for wallet={wallet}: {parsed}"
            )
        return [entry for entry in parsed if isinstance(entry, dict)]

    def list_wallet_txids(self, wallet: str, limit: int = 200) -> set[str]:
        txids: set[str] = set()
        for entry in self.list_wallet_transactions(wallet, limit):
            txid = entry.get("txid")
            if isinstance(txid, str) and txid:
                txids.add(txid)
        return txids

    def find_wallet_transaction_by_comment(
        self, wallet: str, comment: str, limit: int = 200
    ) -> dict[str, Any] | None:
        for entry in self.list_wallet_transactions(wallet, limit):
            if str(entry.get("comment", "")) == comment:
                txid = entry.get("txid")
                if isinstance(txid, str) and txid:
                    return entry
        return None

    def find_new_wallet_transaction(
        self,
        wallet: str,
        baseline_txids: set[str],
        limit: int = 200,
    ) -> dict[str, Any] | None:
        for entry in self.list_wallet_transactions(wallet, limit):
            txid = entry.get("txid")
            if isinstance(txid, str) and txid and txid not in baseline_txids:
                return entry
        return None

    def list_ord_wallet_inscription_ids(self, wallet_name: str) -> set[str]:
        payload = self.run_ord_wallet(wallet_name, ["inscriptions"])
        parsed = json.loads(payload)
        if not isinstance(parsed, list):
            raise WorldSimError(
                f"ord wallet inscriptions returned non-list payload for wallet={wallet_name}: {parsed}"
            )
        inscription_ids: set[str] = set()
        for entry in parsed:
            if not isinstance(entry, dict):
                continue
            inscription_id = entry.get("inscription")
            if isinstance(inscription_id, str) and inscription_id:
                inscription_ids.add(inscription_id)
        return inscription_ids

    def build_action_probe_state(self, actor: Agent, action: str) -> dict[str, Any] | None:
        if action in self.MINT_ACTIONS | self.REMINT_ACTIONS | {"transfer"}:
            return {
                "wallet_name": actor.wallet_name,
                "wallet_txids": sorted(self.list_wallet_txids(actor.wallet_name)),
                "wallet_inscriptions": sorted(
                    self.list_ord_wallet_inscription_ids(actor.wallet_name)
                ),
            }
        if action == "send_balance":
            return {
                "wallet_name": self.args.miner_wallet,
                "wallet_txids": sorted(self.list_wallet_txids(self.args.miner_wallet)),
            }
        if action == "spend_balance":
            return {
                "wallet_name": actor.wallet_name,
                "wallet_txids": sorted(self.list_wallet_txids(actor.wallet_name)),
            }
        return None

    def probe_inflight_external_action_result(
        self,
        *,
        plan: PlannedAction,
        pre_height: int,
        available_ids: set[int],
        tick: int,
        slot_index: int,
    ) -> dict[str, Any] | None:
        probe_state = plan.probe_state or {}
        wallet_name = str(probe_state.get("wallet_name", "")).strip()

        if plan.action in {"send_balance", "spend_balance"}:
            if not wallet_name:
                return None
            entry = self.find_wallet_transaction_by_comment(
                wallet_name,
                f"usdb-world-sim:{plan.action_id}",
            )
            if entry is None:
                return None
            txid = entry.get("txid")
            if not isinstance(txid, str) or not txid:
                raise WorldSimError(
                    "wallet transaction probe missing txid: "
                    f"wallet={wallet_name}, action_id={plan.action_id}, entry={entry}"
                )
            return {
                "source": "bitcoin-wallet-comment",
                "raw_output": txid,
            }

        if plan.action == "transfer":
            if not wallet_name:
                return None
            baseline_txids = {
                str(txid)
                for txid in (probe_state.get("wallet_txids") or [])
                if isinstance(txid, str) and txid
            }
            entry = self.find_new_wallet_transaction(wallet_name, baseline_txids)
            if entry is None:
                return None
            txid = entry.get("txid")
            if not isinstance(txid, str) or not txid:
                raise WorldSimError(
                    "wallet transaction delta probe missing txid: "
                    f"wallet={wallet_name}, action_id={plan.action_id}, entry={entry}"
                )
            return {
                "source": "bitcoin-wallet-delta",
                "raw_output": txid,
            }

        if plan.action in self.MINT_ACTIONS | self.REMINT_ACTIONS:
            if not wallet_name:
                return None
            baseline_inscriptions = {
                str(inscription_id)
                for inscription_id in (probe_state.get("wallet_inscriptions") or [])
                if isinstance(inscription_id, str) and inscription_id
            }
            current_inscriptions = self.list_ord_wallet_inscription_ids(wallet_name)
            new_inscriptions = sorted(current_inscriptions - baseline_inscriptions)
            if not new_inscriptions:
                baseline_txids = {
                    str(txid)
                    for txid in (probe_state.get("wallet_txids") or [])
                    if isinstance(txid, str) and txid
                }
                current_txids = self.list_wallet_txids(wallet_name)
                new_txids = sorted(current_txids - baseline_txids)
                if new_txids:
                    raise WorldSimError(
                        "mint-like external probe found wallet tx delta without inscription delta: "
                        f"wallet={wallet_name}, action_id={plan.action_id}, new_txids={new_txids}"
                    )
                return None
            if len(new_inscriptions) != 1:
                raise WorldSimError(
                    "mint-like external probe found ambiguous inscription delta: "
                    f"wallet={wallet_name}, action_id={plan.action_id}, new_inscriptions={new_inscriptions}"
                )
            return {
                "source": "ord-wallet-inscription-delta",
                "raw_output": new_inscriptions[0],
            }

        return None

    def wait_for_inflight_external_action_result(
        self,
        *,
        plan: PlannedAction,
        pre_height: int,
        available_ids: set[int],
        tick: int,
        slot_index: int,
    ) -> dict[str, Any] | None:
        deadline = time.time() + max(5, min(self.args.sync_timeout_sec, 30))
        last_error: str | None = None

        while time.time() < deadline:
            try:
                payload = self.probe_inflight_external_action_result(
                    plan=plan,
                    pre_height=pre_height,
                    available_ids=available_ids,
                    tick=tick,
                    slot_index=slot_index,
                )
                if payload is not None:
                    return payload
            except Exception as e:  # noqa: BLE001
                last_error = str(e)

            time.sleep(0.5)

        if last_error is not None:
            raise WorldSimError(
                "failed to resolve inflight action outcome after external probes: "
                f"action_id={plan.action_id}, last_error={last_error}"
            )
        return None

    def load_external_action_result(self, action_id: str) -> dict[str, Any] | None:
        result_path = self.external_result_path(action_id)
        if result_path is None or not result_path.exists():
            return None
        payload = json.loads(result_path.read_text(encoding="utf-8"))
        if int(payload.get("version", 0)) != 1:
            raise WorldSimError(
                f"unsupported external action result version: {payload.get('version')}"
            )
        if str(payload.get("action_id", "")) != action_id:
            raise WorldSimError(
                "external action result id mismatch: "
                f"expected={action_id}, got={payload.get('action_id')}"
            )
        return payload

    def clear_external_action_result(self, action_id: str) -> None:
        result_path = self.external_result_path(action_id)
        if result_path is None:
            return
        try:
            result_path.unlink()
        except FileNotFoundError:
            pass

    def load_recovery_state(self) -> dict[str, Any] | None:
        if self.recovery_state_path is None or not self.recovery_state_path.exists():
            return None
        payload = json.loads(self.recovery_state_path.read_text(encoding="utf-8"))
        if int(payload.get("version", 0)) != self.RECOVERY_STATE_VERSION:
            raise WorldSimError(
                f"unsupported recovery state version: {payload.get('version')}"
            )
        if int(payload.get("seed", self.action_seed)) != self.action_seed:
            self.log(
                "Ignoring recovery state with mismatched action seed: "
                f"expected={self.action_seed}, got={payload.get('seed')}"
            )
            return None
        if int(payload.get("batch_blocks", self.args.blocks)) != self.args.blocks:
            self.log(
                "Ignoring recovery state with mismatched batch block count: "
                f"expected={self.args.blocks}, got={payload.get('batch_blocks')}"
            )
            return None
        return payload

    def apply_recovery_snapshot(self, payload: dict[str, Any]) -> None:
        self.active_agent_count = int(
            payload.get("active_agent_count", self.active_agent_count)
        )
        restored_metrics = {
            key: int(value) for key, value in (payload.get("metrics") or {}).items()
        }
        for key in self.metrics:
            restored_metrics.setdefault(key, 0)
        self.metrics = restored_metrics
        self.reorg_events_applied = int(payload.get("reorg_events_applied", 0))
        self.pass_owner_by_id = {
            str(inscription_id): int(owner_id)
            for inscription_id, owner_id in (payload.get("pass_owner_by_id") or {}).items()
        }
        self.pass_identity_by_id = {
            str(inscription_id): PassIdentity(
                pass_kind=str(identity.get("pass_kind", "")),
                leader_ref_kind=identity.get("leader_ref_kind"),
                leader_ref_value=identity.get("leader_ref_value"),
            )
            for inscription_id, identity in (payload.get("pass_identity_by_id") or {}).items()
            if isinstance(identity, dict)
        }
        agent_payloads = payload.get("agents") or []
        if len(agent_payloads) != len(self.agents):
            raise WorldSimError(
                "recovery snapshot agent count mismatch: "
                f"expected={len(self.agents)}, got={len(agent_payloads)}"
            )
        for agent, agent_payload in zip(self.agents, agent_payloads, strict=True):
            self.apply_agent_state(agent, agent_payload)
        self.validator_samples = [
            self.deserialize_validator_sample(sample)
            for sample in (payload.get("validator_samples") or [])
        ]

    @staticmethod
    def metric_delta(before: dict[str, int], after: dict[str, int]) -> dict[str, int]:
        deltas: dict[str, int] = {}
        for key, after_value in after.items():
            delta = int(after_value) - int(before.get(key, 0))
            if delta != 0:
                deltas[key] = delta
        return deltas

    def build_action_receipt(
        self,
        *,
        planned_action: PlannedAction,
        detail: str,
        expectation: ActionExpectation | None,
        used_ids: set[int],
        metrics_before: dict[str, int],
    ) -> ActionReceipt:
        local_patch: dict[str, Any] | None = None
        if expectation is not None:
            if expectation.action in self.MINT_ACTIONS | self.REMINT_ACTIONS:
                if expectation.inscription_id is None:
                    raise WorldSimError(
                        f"mint-like receipt missing inscription_id for action_id={planned_action.action_id}"
                    )
                local_patch = {
                    "kind": "mint_like",
                    "inscription_id": expectation.inscription_id,
                    "owner_agent_id": expectation.actor_id,
                    "invalid": expectation.expect_invalid,
                    "pass_kind": expectation.pass_kind,
                    "leader_ref_kind": expectation.leader_ref_kind,
                    "leader_ref_value": expectation.leader_ref_value,
                }
            elif expectation.action == "transfer":
                if expectation.inscription_id is None or expectation.target_id is None:
                    raise WorldSimError(
                        f"transfer receipt missing fields for action_id={planned_action.action_id}"
                    )
                local_patch = {
                    "kind": "transfer",
                    "inscription_id": expectation.inscription_id,
                    "from_agent_id": expectation.actor_id,
                    "to_agent_id": expectation.target_id,
                }

        return ActionReceipt(
            action_id=planned_action.action_id,
            action=planned_action.action,
            actor_id=planned_action.actor_id,
            detail=detail,
            used_agent_ids=sorted(used_ids),
            expectation=(
                self.serialize_expectation(expectation)
                if expectation is not None and expectation.action != "noop"
                else None
            ),
            metric_deltas=self.metric_delta(metrics_before, self.metrics),
            local_patch=local_patch,
        )

    def apply_action_receipt(
        self, plan: PlannedAction, receipt: ActionReceipt
    ) -> tuple[str, ActionExpectation | None, set[int]]:
        if receipt.action_id != plan.action_id:
            raise WorldSimError(
                "action receipt id mismatch: "
                f"plan={plan.action_id}, receipt={receipt.action_id}"
            )
        if receipt.action != plan.action:
            raise WorldSimError(
                "action receipt action mismatch: "
                f"plan={plan.action}, receipt={receipt.action}"
            )
        if receipt.actor_id != plan.actor_id:
            raise WorldSimError(
                "action receipt actor mismatch: "
                f"plan={plan.actor_id}, receipt={receipt.actor_id}"
            )

        for key, delta in receipt.metric_deltas.items():
            self.metrics[key] = int(self.metrics.get(key, 0)) + int(delta)

        local_patch = receipt.local_patch
        if isinstance(local_patch, dict):
            patch_kind = str(local_patch.get("kind", ""))
            if patch_kind == "mint_like":
                inscription_id = str(local_patch.get("inscription_id", ""))
                owner_agent_id = int(local_patch.get("owner_agent_id", -1))
                if inscription_id and owner_agent_id >= 0:
                    owner = self.agents[owner_agent_id]
                    owner.owned_passes.add(inscription_id)
                    self.pass_owner_by_id[inscription_id] = owner_agent_id
                    self.pass_identity_by_id[inscription_id] = PassIdentity(
                        pass_kind=str(local_patch.get("pass_kind", "standard")),
                        leader_ref_kind=local_patch.get("leader_ref_kind"),
                        leader_ref_value=local_patch.get("leader_ref_value"),
                    )
                    if bool(local_patch.get("invalid", False)):
                        owner.invalid_passes.add(inscription_id)
            elif patch_kind == "transfer":
                inscription_id = str(local_patch.get("inscription_id", ""))
                from_agent_id = int(local_patch.get("from_agent_id", -1))
                to_agent_id = int(local_patch.get("to_agent_id", -1))
                if inscription_id and from_agent_id >= 0 and to_agent_id >= 0:
                    source = self.agents[from_agent_id]
                    target = self.agents[to_agent_id]
                    source.owned_passes.discard(inscription_id)
                    target.owned_passes.add(inscription_id)
                    self.pass_owner_by_id[inscription_id] = to_agent_id
            else:
                raise WorldSimError(f"unsupported action receipt patch kind: {patch_kind}")

        expectation = (
            self.deserialize_expectation(receipt.expectation)
            if isinstance(receipt.expectation, dict)
            else None
        )
        return receipt.detail, expectation, set(receipt.used_agent_ids)

    def rebuild_receipt_from_external_result(
        self,
        *,
        plan: PlannedAction,
        payload: dict[str, Any],
        pre_height: int,
        available_ids: set[int],
        tick: int,
        slot_index: int,
    ) -> ActionReceipt:
        raw_output = str(payload.get("raw_output", "")).strip()
        if not raw_output:
            raise WorldSimError(
                f"external action result missing raw_output for action_id={plan.action_id}"
            )

        actor = self.agents[plan.actor_id]
        rng = self.action_position_rng(
            tick,
            slot_index,
            "execute",
            plan.actor_id,
            plan.action,
            pre_height,
        )

        if plan.action in self.MINT_ACTIONS | self.REMINT_ACTIONS:
            mint_plan = self.build_mint_plan(
                actor=actor,
                action=plan.action,
                available_agent_ids=available_ids,
                pre_height=pre_height,
                rng=rng,
            )
            inscription_id = self.extract_inscription_id(raw_output)
            pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
            invalid_usdb_main = plan.action == "invalid_mint"
            leader_detail = (
                f":{mint_plan.leader_ref_kind}={mint_plan.leader_ref_value}"
                if mint_plan.leader_ref_kind and mint_plan.leader_ref_value
                else ""
            )
            detail = (
                f"invalid_mint:{inscription_id}:owner={actor.wallet_name}"
                if invalid_usdb_main
                else (
                    f"{plan.action}:{inscription_id}:owner={actor.wallet_name}:prev={mint_plan.prev_inscription_id}{leader_detail}"
                    if mint_plan.prev_inscription_id
                    else f"{plan.action}:{inscription_id}:owner={actor.wallet_name}{leader_detail}"
                )
            )
            expectation = ActionExpectation(
                action=plan.action,
                actor_id=actor.agent_id,
                inscription_id=inscription_id,
                prev_inscription_id=mint_plan.prev_inscription_id,
                expect_invalid=invalid_usdb_main,
                actor_pre_balance=pre_balance,
                pass_kind=mint_plan.pass_kind,
                leader_ref_kind=mint_plan.leader_ref_kind,
                leader_ref_value=mint_plan.leader_ref_value,
            )
            used_agent_ids = [actor.agent_id]
            if mint_plan.leader_agent_id is not None:
                used_agent_ids.append(mint_plan.leader_agent_id)
            return ActionReceipt(
                action_id=plan.action_id,
                action=plan.action,
                actor_id=actor.agent_id,
                detail=detail,
                used_agent_ids=sorted(used_agent_ids),
                expectation=self.serialize_expectation(expectation),
                metric_deltas={f"{plan.action}_ok": 1},
                local_patch={
                    "kind": "mint_like",
                    "inscription_id": inscription_id,
                    "owner_agent_id": actor.agent_id,
                    "invalid": invalid_usdb_main,
                    "pass_kind": mint_plan.pass_kind,
                    "leader_ref_kind": mint_plan.leader_ref_kind,
                    "leader_ref_value": mint_plan.leader_ref_value,
                },
            )

        if plan.action == "transfer":
            transfer_candidates = self.load_actor_remint_candidates(actor, pre_height)
            if not transfer_candidates:
                raise WorldSimError(
                    f"cannot rebuild transfer receipt without owned passes for action_id={plan.action_id}"
                )
            target_candidates = [
                self.agents[agent_id]
                for agent_id in sorted(available_ids)
                if agent_id != actor.agent_id
            ]
            if not target_candidates:
                raise WorldSimError(
                    f"cannot rebuild transfer receipt without target candidates for action_id={plan.action_id}"
                )
            inscription_id = str(
                rng.choice(transfer_candidates).get("inscription_id", "")
            )
            if not inscription_id:
                raise WorldSimError(
                    f"cannot rebuild transfer receipt from empty inscription id for action_id={plan.action_id}"
                )
            target = rng.choice(target_candidates)
            target_active_before = (
                self.get_owner_active_pass_snapshot(target.owner_script_hash, pre_height) is not None
            )
            txid = self.extract_txid(raw_output)
            detail = (
                f"transfer:{inscription_id}:from={actor.wallet_name}:"
                f"to={target.wallet_name}:txid={txid[:12]}"
            )
            expectation = ActionExpectation(
                action="transfer",
                actor_id=actor.agent_id,
                inscription_id=inscription_id,
                target_id=target.agent_id,
                target_had_active_before=target_active_before,
            )
            return ActionReceipt(
                action_id=plan.action_id,
                action=plan.action,
                actor_id=actor.agent_id,
                detail=detail,
                used_agent_ids=sorted({actor.agent_id, target.agent_id}),
                expectation=self.serialize_expectation(expectation),
                metric_deltas={"transfer_ok": 1},
                local_patch={
                    "kind": "transfer",
                    "inscription_id": inscription_id,
                    "from_agent_id": actor.agent_id,
                    "to_agent_id": target.agent_id,
                },
            )

        if plan.action == "send_balance":
            amount_btc = self.random_btc_amount(rng, "0.01000000", "0.25000000")
            amount_sat = self.btc_to_sat(amount_btc)
            pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
            txid = self.extract_txid(raw_output)
            expectation = ActionExpectation(
                action="send_balance",
                actor_id=actor.agent_id,
                actor_pre_balance=pre_balance,
                amount_sat=amount_sat,
            )
            return ActionReceipt(
                action_id=plan.action_id,
                action=plan.action,
                actor_id=actor.agent_id,
                detail=f"send_balance:{amount_btc}:to={actor.wallet_name}:txid={txid[:12]}",
                used_agent_ids=[actor.agent_id],
                expectation=self.serialize_expectation(expectation),
                metric_deltas={"send_ok": 1},
                local_patch=None,
            )

        if plan.action == "spend_balance":
            pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
            max_sat = min(pre_balance // 2, 5_000_000)
            min_sat = min(100_000, max_sat)
            if max_sat <= 0 or min_sat <= 0:
                raise WorldSimError(
                    f"cannot rebuild spend receipt without positive spend amount for action_id={plan.action_id}"
                )
            amount_sat = max_sat if max_sat < min_sat else rng.randint(min_sat, max_sat)
            amount_btc = f"{(Decimal(amount_sat) / Decimal('100000000')):.8f}"
            txid = self.extract_txid(raw_output)
            expectation = ActionExpectation(
                action="spend_balance",
                actor_id=actor.agent_id,
                actor_pre_balance=pre_balance,
                amount_sat=amount_sat,
            )
            return ActionReceipt(
                action_id=plan.action_id,
                action=plan.action,
                actor_id=actor.agent_id,
                detail=f"spend_balance:{amount_btc}:from={actor.wallet_name}:txid={txid[:12]}",
                used_agent_ids=[actor.agent_id],
                expectation=self.serialize_expectation(expectation),
                metric_deltas={"spend_ok": 1},
                local_patch=None,
            )

        raise WorldSimError(
            f"unsupported action for external receipt rebuild: {plan.action}"
        )

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

    def default_identity_seed(self) -> str:
        if self.args.identity_seed:
            return self.args.identity_seed
        return f"sim-seed:{self.args.seed}"

    def derived_evm_address(self, namespace: str, *components: Any) -> str:
        digest = hashlib.sha256(
            "::".join(
                [
                    "usdb-world-sim-usdb-main-v1",
                    namespace,
                    self.default_identity_seed(),
                    *(
                        self._normalize_seed_component(component)
                        for component in components
                    ),
                ]
            ).encode("utf-8")
        ).hexdigest()
        return "0x" + digest[:40]

    def _init_agents(self) -> None:
        for idx, (wallet, address) in enumerate(
            zip(self.args.agent_wallets, self.args.agent_addresses)
        ):
            script_hash = self.address_to_script_hash(address)
            usdb_main_address = self.derived_evm_address("agent", idx, wallet, address)
            if (
                self.args.usdb_chain_miner_address
                and idx == self.args.usdb_chain_miner_agent_id
            ):
                usdb_main_address = self.args.usdb_chain_miner_address
            agent = Agent(
                agent_id=idx,
                wallet_name=wallet,
                receive_address=address,
                usdb_main_address=usdb_main_address,
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
            f"-rpcconnect={self.args.btc_rpc_host}",
            f"-rpcport={self.args.btc_rpc_port}",
        ]
        if self.args.btc_auth_mode == "cookie":
            if not self.args.btc_cookie_file:
                raise WorldSimError("btc cookie auth requires --btc-cookie-file")
            cmd.append(f"-rpccookiefile={self.args.btc_cookie_file}")
        elif self.args.btc_auth_mode == "userpass":
            if not self.args.btc_rpc_user or not self.args.btc_rpc_password:
                raise WorldSimError(
                    "btc userpass auth requires --btc-rpc-user and --btc-rpc-password"
                )
            cmd.append(f"-rpcuser={self.args.btc_rpc_user}")
            cmd.append(f"-rpcpassword={self.args.btc_rpc_password}")
        else:
            raise WorldSimError(
                f"unsupported BTC auth mode: {self.args.btc_auth_mode}"
            )
        if wallet:
            cmd.append(f"-rpcwallet={wallet}")
        cmd.extend(rpc_args)
        return self.run_cmd(cmd)

    def run_ord_wallet(self, wallet_name: str, ord_args: list[str]) -> str:
        cmd = [
            self.args.ord_bin,
            "--regtest",
            "--bitcoin-rpc-url",
            f"http://{self.args.btc_rpc_host}:{self.args.btc_rpc_port}",
        ]
        if self.args.btc_auth_mode == "cookie":
            if not self.args.btc_cookie_file:
                raise WorldSimError("btc cookie auth requires --btc-cookie-file")
            cmd.extend(["--cookie-file", self.args.btc_cookie_file])
        elif self.args.btc_auth_mode == "userpass":
            if not self.args.btc_rpc_user or not self.args.btc_rpc_password:
                raise WorldSimError(
                    "btc userpass auth requires --btc-rpc-user and --btc-rpc-password"
                )
            cmd.extend(
                [
                    "--bitcoin-rpc-username",
                    self.args.btc_rpc_user,
                    "--bitcoin-rpc-password",
                    self.args.btc_rpc_password,
                ]
            )
        else:
            raise WorldSimError(
                f"unsupported BTC auth mode: {self.args.btc_auth_mode}"
            )
        cmd.extend(
            [
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
        )
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
                self.wait_for_ord_wallet_recovery(wallet_name)
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

    def get_bitcoin_block_height(self) -> int:
        return int(self.run_btc_cli(None, ["getblockcount"]).strip())

    def get_ord_server_block_height(self) -> int:
        with request.urlopen(  # noqa: S310
            f"{self.args.ord_server_url}/blockcount",
            timeout=self.args.rpc_timeout_sec,
        ) as response:
            return int(response.read().decode("utf-8").strip() or "0")

    def wait_for_ord_wallet_recovery(self, wallet_name: str) -> None:
        deadline = time.time() + max(5, self.args.sync_timeout_sec)
        target_height = self.get_bitcoin_block_height()
        last_error = ""

        while time.time() < deadline:
            try:
                ord_height = self.get_ord_server_block_height()
            except Exception as e:  # noqa: BLE001
                last_error = f"ord_blockcount_error={e}"
                time.sleep(0.5)
                continue

            if ord_height < target_height:
                last_error = (
                    f"ord_height={ord_height} behind target_height={target_height}"
                )
                time.sleep(0.5)
                continue

            balance_cmd = [
                self.args.ord_bin,
                "--regtest",
                "--bitcoin-rpc-url",
                f"http://{self.args.btc_rpc_host}:{self.args.btc_rpc_port}",
            ]
            if self.args.btc_auth_mode == "cookie":
                if not self.args.btc_cookie_file:
                    raise WorldSimError("btc cookie auth requires --btc-cookie-file")
                balance_cmd.extend(["--cookie-file", self.args.btc_cookie_file])
            else:
                if not self.args.btc_rpc_user or not self.args.btc_rpc_password:
                    raise WorldSimError(
                        "btc userpass auth requires --btc-rpc-user and --btc-rpc-password"
                    )
                balance_cmd.extend(
                    [
                        "--bitcoin-rpc-username",
                        self.args.btc_rpc_user,
                        "--bitcoin-rpc-password",
                        self.args.btc_rpc_password,
                    ]
                )
            balance_cmd.extend(
                [
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
                    "balance",
                ]
            )
            proc = subprocess.run(balance_cmd, capture_output=True, text=True)
            if proc.returncode == 0:
                return

            last_error = f"{proc.stdout}\n{proc.stderr}".strip()
            time.sleep(0.5)

        raise WorldSimError(
            "ord wallet transient recovery timeout: "
            f"wallet={wallet_name}, target_height={target_height}, last_error={last_error}"
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
        return self.rpc_result(self.rpc_call(self.args.usdb_indexer_rpc_url, method, params), method)

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
        cached = self.address_script_hash_cache.get(address)
        if cached is not None:
            return cached
        # `validateaddress` is a non-wallet RPC, avoiding wallet selection errors.
        address_info = json.loads(self.run_btc_cli(None, ["validateaddress", address]))
        script_pubkey = address_info.get("scriptPubKey")
        if not isinstance(script_pubkey, str) or not script_pubkey:
            raise WorldSimError(
                f"validateaddress missing scriptPubKey: address={address}, payload={address_info}"
            )
        script_bytes = bytes.fromhex(script_pubkey)
        script_hash = hashlib.sha256(script_bytes).digest()[::-1].hex()
        self.address_script_hash_cache[address] = script_hash
        return script_hash

    @staticmethod
    def btc_to_sat(amount_btc: str) -> int:
        amount = Decimal(amount_btc)
        return int((amount * Decimal("100000000")).to_integral_value())

    @classmethod
    def saturating_energy_add(cls, left: int, right: int) -> int:
        return min(int(left) + int(right), cls.ENERGY_MAX)

    @staticmethod
    def saturating_energy_sub(left: int, right: int) -> int:
        return max(int(left) - int(right), 0)

    @classmethod
    def balance_units(cls, owner_balance: int) -> int:
        return max(int(owner_balance), 0) // cls.UNIT_SATS

    @classmethod
    def calc_growth_delta(cls, owner_balance: int, block_delta: int) -> int:
        raw = (
            cls.balance_units(owner_balance)
            * cls.ENERGY_PER_UNIT_BLOCK
            * max(int(block_delta), 0)
        )
        return min(raw, cls.ENERGY_MAX)

    @classmethod
    def calc_balance_penalty(
        cls,
        balance_before: int,
        balance_after: int,
        active_block_height: int,
        event_block_height: int,
    ) -> int:
        units_before = cls.balance_units(balance_before)
        units_after = cls.balance_units(balance_after)
        lost_units = max(units_before - units_after, 0)
        if units_before == 0 or lost_units == 0:
            return 0
        age_blocks = max(int(event_block_height) - int(active_block_height), 0)
        base = min(
            lost_units * age_blocks * cls.ENERGY_PER_UNIT_BLOCK,
            cls.ENERGY_MAX,
        )
        return min(
            base * cls.PENALTY_LAMBDA_NUM // cls.PENALTY_LAMBDA_DEN,
            cls.ENERGY_MAX,
        )

    @classmethod
    def calc_collab_contribution(cls, raw_energy: int) -> int:
        raw = min(max(int(raw_energy), 0), cls.ENERGY_MAX)
        quotient, remainder = divmod(raw, cls.BPS_DENOMINATOR)
        return min(
            quotient * cls.COLLAB_WEIGHT_BPS
            + remainder * cls.COLLAB_WEIGHT_BPS // cls.BPS_DENOMINATOR,
            cls.ENERGY_MAX,
        )

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

    def get_state_ref_at_height(
        self, block_height: int, context: dict[str, Any] | None = None
    ) -> dict[str, Any] | None:
        params: dict[str, Any] = {"block_height": block_height}
        if context is not None:
            params["context"] = context
        result = self.rpc_usdb("get_state_ref_at_height", [params])
        return result if isinstance(result, dict) else None

    def raw_usdb_rpc(self, method: str, params: Any) -> dict[str, Any]:
        payload = self.rpc_call(self.args.usdb_indexer_rpc_url, method, params)
        if not isinstance(payload, dict):
            raise WorldSimError(f"{method} returned non-dict payload: {payload}")
        return payload

    @staticmethod
    def build_consensus_context_from_state_ref(state_ref: dict[str, Any]) -> dict[str, Any]:
        snapshot_info = state_ref.get("snapshot_info") or {}
        consensus_identity = snapshot_info.get("consensus_identity") or {}
        local_state_info = state_ref.get("local_state_commit_info") or {}
        system_state_info = state_ref.get("system_state_info") or {}
        return {
            "requested_height": int(state_ref.get("block_height", 0)),
            "expected_state": {
                "snapshot_id": snapshot_info.get("snapshot_id"),
                "stable_block_hash": snapshot_info.get("stable_block_hash"),
                "local_state_commit": local_state_info.get("local_state_commit"),
                "system_state_id": system_state_info.get("system_state_id"),
                "balance_history_api_version": consensus_identity.get(
                    "balance_history_api_version"
                ),
                "balance_history_semantics_version": consensus_identity.get(
                    "balance_history_semantics_version"
                ),
                "activation_registry_id": local_state_info.get(
                    "activation_registry_id"
                ),
                "active_version_set_id": local_state_info.get(
                    "active_version_set_id"
                ),
            },
        }

    def load_pass_economic_profile_at_height(
        self,
        inscription_id: str,
        block_height: int,
        context: dict[str, Any],
        *,
        expected_external_state: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        profile_view = self.rpc_usdb(
            "get_pass_economic_profile",
            [
                {
                    "view_version": self.ECONOMIC_VIEW_VERSION,
                    "pass_id": inscription_id,
                    "block_height": block_height,
                    "context": context,
                }
            ],
        )
        if not isinstance(profile_view, dict):
            raise WorldSimError(
                "economic profile returned non-object: "
                f"block_height={block_height}, inscription_id={inscription_id}, "
                f"profile_view={profile_view}"
            )
        if profile_view.get("view_version") != self.ECONOMIC_VIEW_VERSION:
            raise WorldSimError(
                "economic profile view version mismatch: "
                f"expected={self.ECONOMIC_VIEW_VERSION}, got={profile_view.get('view_version')}"
            )
        external_state = profile_view.get("external_state") or {}
        if int(external_state.get("btc_height", -1)) != block_height:
            raise WorldSimError(
                "economic profile external-state height mismatch: "
                f"expected={block_height}, got={external_state.get('btc_height')}"
            )
        if expected_external_state is not None and external_state != expected_external_state:
            raise WorldSimError(
                "economic profile external-state identity mismatch: "
                f"inscription_id={inscription_id}, expected={expected_external_state}, "
                f"got={external_state}"
            )
        profile = profile_view.get("pass") or {}
        if not isinstance(profile, dict) or profile.get("pass_id") != inscription_id:
            raise WorldSimError(
                "economic profile is missing requested pass: "
                f"expected={inscription_id}, profile={profile}"
            )
        return profile

    @staticmethod
    def assert_zero_economic_profile(
        profile: dict[str, Any], *, inscription_id: str, expected_state: str
    ) -> None:
        expected = {
            "state": expected_state,
            "raw_energy": "0",
            "collab_contribution": "0",
            "effective_energy": "0",
            "level": 0,
            "difficulty_factor_bps": 10_000,
            "collab_breakdown_count": 0,
        }
        for field_name, expected_value in expected.items():
            actual_value = profile.get(field_name)
            if actual_value != expected_value:
                raise WorldSimError(
                    "terminal economic profile mismatch: "
                    f"inscription_id={inscription_id}, field={field_name}, "
                    f"expected={expected_value}, got={actual_value}"
                )

    @staticmethod
    def candidate_from_view_item(item: dict[str, Any]) -> ValidatorSampleCandidate:
        return ValidatorSampleCandidate(
            inscription_id=str(item.get("pass_id", "")),
            owner=str(item.get("owner_script_hash", "")),
            state=str(item.get("state", "")),
            pass_kind=str(item.get("pass_kind", "")),
            raw_energy=int(item.get("raw_energy", 0)),
            collab_contribution=int(item.get("collab_contribution", 0)),
            effective_energy=int(item.get("effective_energy", 0)),
            level=int(item.get("level", 0)),
            difficulty_factor_bps=int(item.get("difficulty_factor_bps", 0)),
        )

    def load_candidate_set_view_at_height(
        self, block_height: int, context: dict[str, Any]
    ) -> dict[str, Any]:
        cursor: str | None = None
        aggregate: dict[str, Any] | None = None
        page_count = 0
        page_limit = self.args.economic_page_limit

        while True:
            params: dict[str, Any] = {
                "view_version": self.ECONOMIC_VIEW_VERSION,
                "selection_rule": self.CANDIDATE_SELECTION_RULE,
                "cursor": cursor,
                "limit": page_limit,
            }
            if cursor is None:
                params["block_height"] = block_height
                params["context"] = context

            page = self.rpc_usdb("get_candidate_set_view", [params])
            if not isinstance(page, dict):
                raise WorldSimError(
                    "validator sample candidate view returned non-object: "
                    f"block_height={block_height}, page={page}"
                )
            if page.get("view_version") != self.ECONOMIC_VIEW_VERSION:
                raise WorldSimError(
                    "validator sample candidate view version mismatch: "
                    f"expected={self.ECONOMIC_VIEW_VERSION}, got={page.get('view_version')}"
                )
            if page.get("selection_rule") != self.CANDIDATE_SELECTION_RULE:
                raise WorldSimError(
                    "validator sample candidate selection rule mismatch: "
                    f"expected={self.CANDIDATE_SELECTION_RULE}, got={page.get('selection_rule')}"
                )
            if int(page.get("limit", 0)) != page_limit:
                raise WorldSimError(
                    "validator sample candidate page limit mismatch: "
                    f"expected={page_limit}, got={page.get('limit')}"
                )
            if int(page.get("max_limit", 0)) < page_limit:
                raise WorldSimError(
                    "validator sample candidate max_limit is below requested limit: "
                    f"requested={page_limit}, max_limit={page.get('max_limit')}"
                )

            external_state = page.get("external_state") or {}
            if int(external_state.get("btc_height", -1)) != block_height:
                raise WorldSimError(
                    "validator sample candidate external-state height mismatch: "
                    f"expected={block_height}, got={external_state.get('btc_height')}"
                )
            items = page.get("items") or []
            if not isinstance(items, list):
                raise WorldSimError(
                    "validator sample candidate items must be an array: "
                    f"block_height={block_height}, items={items}"
                )

            if aggregate is None:
                aggregate = {
                    "view_version": page["view_version"],
                    "external_state": external_state,
                    "selection_rule": page["selection_rule"],
                    "total": int(page.get("total", 0)),
                    "limit": int(page["limit"]),
                    "max_limit": int(page["max_limit"]),
                    "items": [],
                }
            else:
                for field in (
                    "view_version",
                    "external_state",
                    "selection_rule",
                    "total",
                    "limit",
                    "max_limit",
                ):
                    expected = aggregate[field]
                    actual = int(page[field]) if field in {"total", "limit", "max_limit"} else page[field]
                    if actual != expected:
                        raise WorldSimError(
                            "validator sample candidate continuation changed immutable field: "
                            f"field={field}, expected={expected}, got={actual}"
                        )

            aggregate["items"].extend(items)
            cursor_value = page.get("next_cursor")
            if cursor_value is None:
                break
            if not isinstance(cursor_value, str) or not cursor_value:
                raise WorldSimError(
                    "validator sample candidate continuation cursor is invalid: "
                    f"cursor={cursor_value}"
                )
            cursor = cursor_value
            page_count += 1
            if page_count > 10_000:
                raise WorldSimError("validator sample candidate pagination exceeded safety limit")

        if aggregate is None:
            raise WorldSimError("validator sample candidate pagination produced no page")
        candidates = [self.candidate_from_view_item(item) for item in aggregate["items"]]
        if len(candidates) != aggregate["total"]:
            raise WorldSimError(
                "validator sample candidate pagination is incomplete: "
                f"expected_total={aggregate['total']}, rows={len(candidates)}"
            )
        candidate_ids = [candidate.inscription_id for candidate in candidates]
        if any(not candidate_id for candidate_id in candidate_ids):
            raise WorldSimError("validator sample candidate view contains an empty pass id")
        if len(candidate_ids) != len(set(candidate_ids)):
            raise WorldSimError("validator sample candidate view contains duplicate pass ids")
        if any(
            candidate.state != "active" or candidate.pass_kind != "standard"
            for candidate in candidates
        ):
            raise WorldSimError(
                "validator sample candidate view contains non-active or non-standard pass"
            )
        expected = sorted(
            candidates,
            key=lambda candidate: (-candidate.effective_energy, candidate.inscription_id),
        )
        if candidates != expected:
            raise WorldSimError(
                "validator sample candidate view violated effective-energy ordering"
            )
        return aggregate

    def load_collab_breakdown_at_height(
        self,
        leader_pass_id: str,
        block_height: int,
        context: dict[str, Any],
        *,
        sort: str,
        expected_external_state: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        cursor: str | None = None
        aggregate: dict[str, Any] | None = None
        seen_cursors: set[str] = set()
        page_limit = self.args.economic_page_limit

        while True:
            params: dict[str, Any] = {
                "view_version": self.ECONOMIC_VIEW_VERSION,
                "leader_pass_id": leader_pass_id,
                "sort": sort,
                "cursor": cursor,
                "limit": page_limit,
            }
            if cursor is None:
                params["block_height"] = block_height
                params["context"] = context

            page = self.rpc_usdb("get_collab_breakdown", [params])
            if not isinstance(page, dict):
                raise WorldSimError(
                    "collab breakdown returned non-object: "
                    f"leader={leader_pass_id}, block_height={block_height}, page={page}"
                )
            if page.get("view_version") != self.ECONOMIC_VIEW_VERSION:
                raise WorldSimError(
                    "collab breakdown view version mismatch: "
                    f"expected={self.ECONOMIC_VIEW_VERSION}, got={page.get('view_version')}"
                )
            if page.get("leader_pass_id") != leader_pass_id:
                raise WorldSimError(
                    "collab breakdown Leader mismatch: "
                    f"expected={leader_pass_id}, got={page.get('leader_pass_id')}"
                )
            if page.get("sort") != sort:
                raise WorldSimError(
                    f"collab breakdown sort mismatch: expected={sort}, got={page.get('sort')}"
                )
            if int(page.get("limit", 0)) != page_limit:
                raise WorldSimError(
                    "collab breakdown page limit mismatch: "
                    f"expected={page_limit}, got={page.get('limit')}"
                )
            if int(page.get("max_limit", 0)) < page_limit:
                raise WorldSimError(
                    "collab breakdown max_limit is below requested limit: "
                    f"requested={page_limit}, max_limit={page.get('max_limit')}"
                )

            external_state = page.get("external_state") or {}
            if int(external_state.get("btc_height", -1)) != block_height:
                raise WorldSimError(
                    "collab breakdown external-state height mismatch: "
                    f"expected={block_height}, got={external_state.get('btc_height')}"
                )
            if expected_external_state is not None and external_state != expected_external_state:
                raise WorldSimError(
                    "collab breakdown external-state identity mismatch: "
                    f"leader={leader_pass_id}, expected={expected_external_state}, "
                    f"got={external_state}"
                )
            items = page.get("items") or []
            if not isinstance(items, list):
                raise WorldSimError(
                    "collab breakdown items must be an array: "
                    f"leader={leader_pass_id}, items={items}"
                )

            if aggregate is None:
                aggregate = {
                    "view_version": page["view_version"],
                    "external_state": external_state,
                    "leader_pass_id": page["leader_pass_id"],
                    "leader_state": page.get("leader_state"),
                    "leader_pass_kind": page.get("leader_pass_kind"),
                    "sort": page["sort"],
                    "total": int(page.get("total", 0)),
                    "aggregate_collab_contribution": str(
                        page.get("aggregate_collab_contribution", "")
                    ),
                    "limit": int(page["limit"]),
                    "max_limit": int(page["max_limit"]),
                    "items": [],
                }
            else:
                immutable_fields = {
                    "view_version": page.get("view_version"),
                    "external_state": external_state,
                    "leader_pass_id": page.get("leader_pass_id"),
                    "leader_state": page.get("leader_state"),
                    "leader_pass_kind": page.get("leader_pass_kind"),
                    "sort": page.get("sort"),
                    "total": int(page.get("total", 0)),
                    "aggregate_collab_contribution": str(
                        page.get("aggregate_collab_contribution", "")
                    ),
                    "limit": int(page.get("limit", 0)),
                    "max_limit": int(page.get("max_limit", 0)),
                }
                for field_name, actual in immutable_fields.items():
                    if aggregate[field_name] != actual:
                        raise WorldSimError(
                            "collab breakdown continuation changed immutable field: "
                            f"leader={leader_pass_id}, field={field_name}, "
                            f"expected={aggregate[field_name]}, got={actual}"
                        )

            aggregate["items"].extend(items)
            next_cursor = page.get("next_cursor")
            if next_cursor is None:
                break
            if not isinstance(next_cursor, str) or not next_cursor:
                raise WorldSimError(
                    f"collab breakdown returned invalid cursor: {next_cursor}"
                )
            if next_cursor in seen_cursors:
                raise WorldSimError("collab breakdown repeated a continuation cursor")
            seen_cursors.add(next_cursor)
            cursor = next_cursor
            if len(seen_cursors) > 10_000:
                raise WorldSimError("collab breakdown pagination exceeded safety limit")

        if aggregate is None:
            raise WorldSimError("collab breakdown pagination produced no page")
        items = aggregate["items"]
        if len(items) != aggregate["total"]:
            raise WorldSimError(
                "collab breakdown pagination is incomplete: "
                f"leader={leader_pass_id}, expected={aggregate['total']}, got={len(items)}"
            )
        item_ids = [str(item.get("collab_pass_id", "")) for item in items]
        if any(not item_id for item_id in item_ids) or len(item_ids) != len(set(item_ids)):
            raise WorldSimError(
                f"collab breakdown contains empty or duplicate pass ids: leader={leader_pass_id}"
            )

        recomputed_aggregate = 0
        for item in items:
            raw_energy = int(item.get("collab_raw_energy", 0))
            contribution = int(item.get("collab_contribution", 0))
            expected_contribution = self.calc_collab_contribution(raw_energy)
            if int(item.get("collab_weight_bps", -1)) != self.COLLAB_WEIGHT_BPS:
                raise WorldSimError(
                    "collab breakdown weight mismatch: "
                    f"pass={item.get('collab_pass_id')}, expected={self.COLLAB_WEIGHT_BPS}, "
                    f"got={item.get('collab_weight_bps')}"
                )
            if contribution != expected_contribution:
                raise WorldSimError(
                    "collab breakdown contribution formula mismatch: "
                    f"pass={item.get('collab_pass_id')}, raw={raw_energy}, "
                    f"expected={expected_contribution}, got={contribution}"
                )
            recomputed_aggregate = self.saturating_energy_add(
                recomputed_aggregate, contribution
            )

        if recomputed_aggregate != int(aggregate["aggregate_collab_contribution"]):
            raise WorldSimError(
                "collab breakdown aggregate is not reproducible from rows: "
                f"leader={leader_pass_id}, expected={recomputed_aggregate}, "
                f"got={aggregate['aggregate_collab_contribution']}"
            )
        if sort == "collab_pass_id_asc":
            expected_items = sorted(items, key=lambda item: str(item["collab_pass_id"]))
        elif sort == "contribution_desc_pass_id_asc":
            expected_items = sorted(
                items,
                key=lambda item: (
                    -int(item["collab_contribution"]),
                    str(item["collab_pass_id"]),
                ),
            )
        else:
            raise WorldSimError(f"unsupported collab breakdown sort: {sort}")
        if items != expected_items:
            raise WorldSimError(
                f"collab breakdown ordering mismatch: leader={leader_pass_id}, sort={sort}"
            )
        return aggregate

    def resolve_expected_collab_leader(
        self,
        collab_snapshot: dict[str, Any],
        active_standard_by_id: dict[str, dict[str, Any]],
        active_standard_by_owner: dict[str, dict[str, Any]],
    ) -> str | None:
        fixed_leader_id = collab_snapshot.get("leader_pass_id")
        address_leader = collab_snapshot.get("leader_btc_addr")
        if fixed_leader_id and address_leader:
            raise WorldSimError(
                "collab pass declares both Leader reference kinds: "
                f"pass={collab_snapshot.get('inscription_id')}"
            )
        if fixed_leader_id:
            fixed_id = str(fixed_leader_id)
            return fixed_id if fixed_id in active_standard_by_id else None
        if address_leader:
            owner_script_hash = self.address_to_script_hash(str(address_leader))
            leader = active_standard_by_owner.get(owner_script_hash)
            return str(leader["inscription_id"]) if leader is not None else None
        raise WorldSimError(
            "active collab pass is missing a Leader reference: "
            f"pass={collab_snapshot.get('inscription_id')}"
        )

    def cross_check_profile_with_breakdown(
        self,
        *,
        profile: dict[str, Any],
        block_height: int,
        context: dict[str, Any],
        expected_external_state: dict[str, Any] | None = None,
    ) -> int:
        pass_id = str(profile.get("pass_id", ""))
        if profile.get("state") != "active" or profile.get("pass_kind") != "standard":
            raise WorldSimError(
                "profile/breakdown cross-check requires an active standard pass: "
                f"pass={pass_id}, state={profile.get('state')}, kind={profile.get('pass_kind')}"
            )
        breakdown = self.load_collab_breakdown_at_height(
            pass_id,
            block_height,
            context,
            sort="collab_pass_id_asc",
            expected_external_state=expected_external_state,
        )
        aggregate = int(breakdown["aggregate_collab_contribution"])
        if int(profile.get("collab_contribution", 0)) != aggregate:
            raise WorldSimError(
                "profile/breakdown contribution mismatch: "
                f"pass={pass_id}, profile={profile.get('collab_contribution')}, "
                f"breakdown={aggregate}"
            )
        if int(profile.get("collab_breakdown_count", -1)) != len(breakdown["items"]):
            raise WorldSimError(
                "profile/breakdown row-count mismatch: "
                f"pass={pass_id}, profile={profile.get('collab_breakdown_count')}, "
                f"rows={len(breakdown['items'])}"
            )
        expected_effective = self.saturating_energy_add(
            int(profile.get("raw_energy", 0)), aggregate
        )
        if int(profile.get("effective_energy", 0)) != expected_effective:
            raise WorldSimError(
                "profile/breakdown effective energy mismatch: "
                f"pass={pass_id}, expected={expected_effective}, "
                f"got={profile.get('effective_energy')}"
            )
        return len(breakdown["items"])

    def should_capture_validator_sample(self, tick: int) -> bool:
        if not self.args.validator_sample_enabled:
            return False
        interval = self.args.validator_sample_interval_blocks
        if interval <= 0:
            return False
        return tick % interval == 0

    def build_validator_sample_candidate(
        self, inscription_id: str, block_height: int, context: dict[str, Any]
    ) -> ValidatorSampleCandidate:
        profile = self.load_pass_economic_profile_at_height(
            inscription_id, block_height, context
        )
        return self.candidate_from_view_item(profile)

    @staticmethod
    def assert_validator_candidate_matches(
        expected: ValidatorSampleCandidate,
        actual: ValidatorSampleCandidate,
        sample_id: str,
    ) -> None:
        for field_name in (
            "inscription_id",
            "owner",
            "state",
            "pass_kind",
            "raw_energy",
            "collab_contribution",
            "effective_energy",
            "level",
            "difficulty_factor_bps",
        ):
            expected_value = getattr(expected, field_name)
            actual_value = getattr(actual, field_name)
            if actual_value != expected_value:
                raise WorldSimError(
                    "validator sample candidate mismatch: "
                    f"sample={sample_id}, candidate={expected.inscription_id}, "
                    f"field={field_name}, expected={expected_value}, got={actual_value}"
                )

    def validate_tampered_candidate_set_sample(
        self, sample: ValidatorSample, actual_winner: ValidatorSampleCandidate, tick: int, block_height: int
    ) -> None:
        if not self.args.validator_sample_tamper_enabled:
            return

        tampered_winner = next(
            (
                candidate
                for candidate in sample.candidates
                if candidate.inscription_id != actual_winner.inscription_id
            ),
            None,
        )
        if tampered_winner is None:
            return

        if tampered_winner.inscription_id == actual_winner.inscription_id:
            self.metrics["validator_sample_tamper_fail"] += 1
            raise WorldSimError(
                "validator sample tamper check failed to produce a different winner: "
                f"sample={sample.sample_id}, winner={actual_winner.inscription_id}"
            )

        self.metrics["validator_sample_tamper_ok"] += 1
        self.emit_report(
            "validator_sample_tamper_validation",
            {
                "tick": tick,
                "head_block_height": block_height,
                "sample_id": sample.sample_id,
                "sample_block_height": sample.block_height,
                "mode": sample.mode,
                "result": "tamper_detected",
                "actual_winner_inscription_id": actual_winner.inscription_id,
                "tampered_winner_inscription_id": tampered_winner.inscription_id,
            },
        )

    def capture_validator_samples(self, block_height: int, tick: int) -> list[str]:
        state_ref = self.get_state_ref_at_height(block_height)
        if state_ref is None:
            raise WorldSimError(
                f"validator sample capture missing state ref: tick={tick}, block_height={block_height}"
            )
        context = self.build_consensus_context_from_state_ref(state_ref)
        candidate_view = self.load_candidate_set_view_at_height(block_height, context)
        all_candidates = [
            self.candidate_from_view_item(item) for item in candidate_view["items"]
        ]
        if not all_candidates:
            return []

        candidate_by_id = {
            candidate.inscription_id: candidate for candidate in all_candidates
        }
        candidate_ids = sorted(candidate_by_id)
        sample_size = self.args.validator_sample_size
        if sample_size <= 0 or sample_size >= len(candidate_ids):
            selected_ids = candidate_ids
        else:
            selected_ids = sorted(
                self.diagnostic_position_rng(
                    "validator-sample-capture", tick, block_height
                ).sample(candidate_ids, sample_size)
            )

        external_state = candidate_view["external_state"]

        captured: list[str] = []
        if self.args.validator_sample_mode == "candidate_set":
            candidates = [candidate_by_id[inscription_id] for inscription_id in selected_ids]
            winner = self.choose_candidate_set_winner(candidates)
            sample_hash = hashlib.sha256(",".join(selected_ids).encode("utf-8")).hexdigest()[:12]
            sample = ValidatorSample(
                sample_id=f"h{block_height}:candidate_set:{sample_hash}",
                mode="candidate_set",
                tick=tick,
                block_height=block_height,
                inscription_id=winner.inscription_id,
                owner=winner.owner,
                state=winner.state,
                pass_kind=winner.pass_kind,
                raw_energy=winner.raw_energy,
                collab_contribution=winner.collab_contribution,
                effective_energy=winner.effective_energy,
                level=winner.level,
                difficulty_factor_bps=winner.difficulty_factor_bps,
                snapshot_id=str(external_state.get("snapshot_id", "")),
                stable_block_hash=str(external_state.get("stable_block_hash", "")),
                local_state_commit=str(external_state.get("local_state_commit", "")),
                system_state_id=str(external_state.get("system_state_id", "")),
                balance_history_api_version=str(
                    external_state.get("balance_history_api_version", "")
                ),
                balance_history_semantics_version=str(
                    external_state.get("balance_history_semantics_version", "")
                ),
                activation_registry_id=str(
                    external_state.get("activation_registry_id", "")
                ),
                active_version_set_id=str(
                    external_state.get("active_version_set_id", "")
                ),
                candidates=candidates,
                winner_inscription_id=winner.inscription_id,
            )
            self.validator_samples.append(sample)
            captured.append(sample.sample_id)
        else:
            for inscription_id in selected_ids:
                candidate = candidate_by_id[inscription_id]
                sample = ValidatorSample(
                    sample_id=f"h{block_height}:{inscription_id}",
                    mode="single",
                    tick=tick,
                    block_height=block_height,
                    inscription_id=inscription_id,
                    owner=candidate.owner,
                    state=candidate.state,
                    pass_kind=candidate.pass_kind,
                    raw_energy=candidate.raw_energy,
                    collab_contribution=candidate.collab_contribution,
                    effective_energy=candidate.effective_energy,
                    level=candidate.level,
                    difficulty_factor_bps=candidate.difficulty_factor_bps,
                    snapshot_id=str(external_state.get("snapshot_id", "")),
                    stable_block_hash=str(external_state.get("stable_block_hash", "")),
                    local_state_commit=str(external_state.get("local_state_commit", "")),
                    system_state_id=str(external_state.get("system_state_id", "")),
                    balance_history_api_version=str(
                        external_state.get("balance_history_api_version", "")
                    ),
                    balance_history_semantics_version=str(
                        external_state.get("balance_history_semantics_version", "")
                    ),
                    activation_registry_id=str(
                        external_state.get("activation_registry_id", "")
                    ),
                    active_version_set_id=str(
                        external_state.get("active_version_set_id", "")
                    ),
                )
                self.validator_samples.append(sample)
                captured.append(sample.sample_id)

        if captured:
            self.emit_report(
                "validator_sample_capture",
                {
                    "tick": tick,
                    "block_height": block_height,
                    "mode": self.args.validator_sample_mode,
                    "sample_ids": captured,
                    "count": len(captured),
                    "sample_size": len(selected_ids),
                    "candidate_ids": selected_ids,
                    "winner_ids": [
                        sample.winner_inscription_id
                        for sample in self.validator_samples
                        if sample.sample_id in captured and sample.winner_inscription_id is not None
                    ],
                },
            )
        return captured

    def validate_pending_validator_samples(
        self, block_height: int, tick: int
    ) -> tuple[int, int, list[str]]:
        if not self.args.validator_sample_enabled:
            return 0, 0, []

        checked = 0
        failed = 0
        fail_samples: list[str] = []
        min_advance = max(1, self.args.validator_sample_min_head_advance)

        for sample in self.validator_samples:
            if sample.validated:
                continue
            if block_height < sample.block_height + min_advance:
                continue

            checked += 1
            context = {
                "requested_height": sample.block_height,
                "expected_state": {
                    "snapshot_id": sample.snapshot_id,
                    "stable_block_hash": sample.stable_block_hash,
                    "local_state_commit": sample.local_state_commit,
                    "system_state_id": sample.system_state_id,
                    "balance_history_api_version": sample.balance_history_api_version,
                    "balance_history_semantics_version": sample.balance_history_semantics_version,
                    "activation_registry_id": sample.activation_registry_id,
                    "active_version_set_id": sample.active_version_set_id,
                },
            }
            try:
                if sample.expected_consensus_error is not None:
                    state_ref_payload = self.raw_usdb_rpc(
                        "get_state_ref_at_height",
                        [{"block_height": sample.block_height, "context": context}],
                    )
                    error = state_ref_payload.get("error")
                    actual_error = (
                        str((error or {}).get("message", "")) if isinstance(error, dict) else ""
                    )
                    if actual_error != sample.expected_consensus_error:
                        raise WorldSimError(
                            "validator sample expected consensus error mismatch: "
                            f"sample={sample.sample_id}, expected={sample.expected_consensus_error}, got={actual_error or state_ref_payload}"
                        )

                    sample.validated = True
                    sample.validated_tick = tick
                    self.metrics["validator_sample_ok"] += 1
                    self.emit_report(
                        "validator_sample_validation",
                        {
                            "tick": tick,
                            "head_block_height": block_height,
                            "sample_id": sample.sample_id,
                            "sample_block_height": sample.block_height,
                            "mode": sample.mode,
                            "result": "expected_mismatch",
                            "expected_error": sample.expected_consensus_error,
                            "winner_inscription_id": sample.winner_inscription_id,
                            "invalidated_by_reorg_tick": sample.invalidated_by_reorg_tick,
                        },
                    )
                    continue

                state_ref = self.get_state_ref_at_height(sample.block_height, context)
                if state_ref is None:
                    raise WorldSimError(
                        f"missing historical state ref for sample={sample.sample_id}"
                    )
                if sample.mode == "candidate_set":
                    candidate_view = self.load_candidate_set_view_at_height(
                        sample.block_height, context
                    )
                    canonical_candidates = {
                        candidate.inscription_id: candidate
                        for candidate in (
                            self.candidate_from_view_item(item)
                            for item in candidate_view["items"]
                        )
                    }
                    actual_candidates: list[ValidatorSampleCandidate] = []
                    for expected in sample.candidates:
                        actual = canonical_candidates.get(expected.inscription_id)
                        if actual is None:
                            raise WorldSimError(
                                "validator sample candidate disappeared from canonical view: "
                                f"sample={sample.sample_id}, candidate={expected.inscription_id}"
                            )
                        profile = self.load_pass_economic_profile_at_height(
                            expected.inscription_id,
                            sample.block_height,
                            context,
                            expected_external_state=candidate_view["external_state"],
                        )
                        profile_candidate = self.candidate_from_view_item(profile)
                        self.assert_validator_candidate_matches(
                            actual, profile_candidate, sample.sample_id
                        )
                        self.assert_validator_candidate_matches(
                            expected, actual, sample.sample_id
                        )
                        self.cross_check_profile_with_breakdown(
                            profile=profile,
                            block_height=sample.block_height,
                            context=context,
                            expected_external_state=candidate_view["external_state"],
                        )
                        actual_candidates.append(actual)

                    actual_winner = self.choose_candidate_set_winner(actual_candidates)
                    if actual_winner.inscription_id != sample.winner_inscription_id:
                        raise WorldSimError(
                            "validator sample candidate-set winner mismatch: "
                            f"sample={sample.sample_id}, expected_winner={sample.winner_inscription_id}, "
                            f"got_winner={actual_winner.inscription_id}"
                        )
                    self.validate_tampered_candidate_set_sample(
                        sample, actual_winner, tick, block_height
                    )
                else:
                    profile = self.load_pass_economic_profile_at_height(
                        sample.inscription_id, sample.block_height, context
                    )
                    actual = self.candidate_from_view_item(profile)
                    expected = ValidatorSampleCandidate(
                        inscription_id=sample.inscription_id,
                        owner=sample.owner,
                        state=sample.state,
                        pass_kind=sample.pass_kind,
                        raw_energy=sample.raw_energy,
                        collab_contribution=sample.collab_contribution,
                        effective_energy=sample.effective_energy,
                        level=sample.level,
                        difficulty_factor_bps=sample.difficulty_factor_bps,
                    )
                    self.assert_validator_candidate_matches(
                        expected, actual, sample.sample_id
                    )
                    if actual.state == "active" and actual.pass_kind == "standard":
                        self.cross_check_profile_with_breakdown(
                            profile=profile,
                            block_height=sample.block_height,
                            context=context,
                        )
                    elif actual.pass_kind == "collab":
                        collab_boundary = (
                            actual.collab_contribution,
                            actual.effective_energy,
                            actual.level,
                            actual.difficulty_factor_bps,
                        )
                        if collab_boundary != (0, 0, 0, self.BPS_DENOMINATOR):
                            raise WorldSimError(
                                "historical collab profile violated zero-effective boundary: "
                                f"sample={sample.sample_id}, values={collab_boundary}"
                            )

                sample.validated = True
                sample.validated_tick = tick
                self.metrics["validator_sample_ok"] += 1
                self.emit_report(
                    "validator_sample_validation",
                    {
                        "tick": tick,
                        "head_block_height": block_height,
                        "sample_id": sample.sample_id,
                        "sample_block_height": sample.block_height,
                        "mode": sample.mode,
                        "result": "ok",
                        "winner_inscription_id": sample.winner_inscription_id,
                        "invalidated_by_reorg_tick": sample.invalidated_by_reorg_tick,
                    },
                )
            except Exception as e:  # noqa: BLE001
                failed += 1
                self.metrics["validator_sample_fail"] += 1
                fail_samples.append(f"sample={sample.sample_id},error={e}")
                self.emit_report(
                    "validator_sample_validation",
                    {
                        "tick": tick,
                        "head_block_height": block_height,
                        "sample_id": sample.sample_id,
                        "sample_block_height": sample.block_height,
                        "result": "fail",
                        "error": str(e),
                    },
                )
                if self.args.fail_fast:
                    raise

        return checked, failed, fail_samples[:8]

    def wait_service_synced(self, target_height: int) -> None:
        start = time.time()
        while True:
            bh_height = int(self.rpc_balance_history("get_block_height", []) or 0)
            usdb_height = self.rpc_usdb("get_synced_block_height", [])
            usdb_height_num = 0 if usdb_height is None else int(usdb_height)
            bh_readiness = self.rpc_balance_history("get_readiness", [])
            usdb_readiness = self.rpc_usdb("get_readiness", [])
            bh_consensus_ready = bool(
                bh_readiness.get("consensus_ready")
                if isinstance(bh_readiness, dict)
                else False
            )
            usdb_consensus_ready = bool(
                usdb_readiness.get("consensus_ready")
                if isinstance(usdb_readiness, dict)
                else False
            )

            if (
                bh_height >= target_height
                and usdb_height_num >= target_height
                and bh_consensus_ready
                and usdb_consensus_ready
            ):
                return

            if time.time() - start > self.args.sync_timeout_sec:
                raise WorldSimError(
                    "sync timeout: "
                    f"target_height={target_height}, bh_height={bh_height}, usdb_height={usdb_height_num}, "
                    f"bh_consensus_ready={bh_consensus_ready}, usdb_consensus_ready={usdb_consensus_ready}"
                )
            time.sleep(0.8)

    def wait_service_height_exact(self, target_height: int) -> None:
        start = time.time()
        while True:
            bh_height = int(self.rpc_balance_history("get_block_height", []) or 0)
            usdb_height = self.rpc_usdb("get_synced_block_height", [])
            usdb_height_num = 0 if usdb_height is None else int(usdb_height)
            bh_readiness = self.rpc_balance_history("get_readiness", [])
            usdb_readiness = self.rpc_usdb("get_readiness", [])
            bh_consensus_ready = bool(
                bh_readiness.get("consensus_ready")
                if isinstance(bh_readiness, dict)
                else False
            )
            usdb_consensus_ready = bool(
                usdb_readiness.get("consensus_ready")
                if isinstance(usdb_readiness, dict)
                else False
            )

            if (
                bh_height == target_height
                and usdb_height_num == target_height
                and bh_consensus_ready
                and usdb_consensus_ready
            ):
                return

            if time.time() - start > self.args.sync_timeout_sec:
                raise WorldSimError(
                    "exact sync timeout: "
                    f"target_height={target_height}, bh_height={bh_height}, usdb_height={usdb_height_num}, "
                    f"bh_consensus_ready={bh_consensus_ready}, usdb_consensus_ready={usdb_consensus_ready}"
                )
            time.sleep(0.8)

    def wait_balance_history_height_exact(self, target_height: int) -> None:
        start = time.time()
        while True:
            bh_height = int(self.rpc_balance_history("get_block_height", []) or 0)
            bh_readiness = self.rpc_balance_history("get_readiness", [])
            bh_consensus_ready = bool(
                bh_readiness.get("consensus_ready")
                if isinstance(bh_readiness, dict)
                else False
            )
            if bh_height == target_height and bh_consensus_ready:
                return
            if time.time() - start > self.args.sync_timeout_sec:
                raise WorldSimError(
                    "balance-history exact sync timeout: "
                    f"target_height={target_height}, bh_height={bh_height}, "
                    f"bh_consensus_ready={bh_consensus_ready}"
                )
            time.sleep(0.8)

    def wait_snapshot_hashes(self, target_height: int, target_hash: str) -> tuple[str, str]:
        start = time.time()
        while True:
            bh_snapshot = self.rpc_balance_history("get_snapshot_info", [])
            usdb_snapshot = self.rpc_usdb("get_snapshot_info", [])
            bh_stable_hash = str((bh_snapshot or {}).get("stable_block_hash", ""))
            usdb_stable_hash = str((usdb_snapshot or {}).get("stable_block_hash", ""))
            bh_stable_height = int((bh_snapshot or {}).get("stable_height", -1))
            usdb_stable_height = int((usdb_snapshot or {}).get("balance_history_stable_height", -1))

            if (
                bh_stable_height == target_height
                and usdb_stable_height == target_height
                and bh_stable_hash == target_hash
                and usdb_stable_hash == target_hash
            ):
                return bh_stable_hash, usdb_stable_hash

            if time.time() - start > self.args.sync_timeout_sec:
                raise WorldSimError(
                    "snapshot hash sync timeout after reorg: "
                    f"target_height={target_height}, target_hash={target_hash}, "
                    f"bh_stable_height={bh_stable_height}, bh_stable_hash={bh_stable_hash}, "
                    f"usdb_stable_height={usdb_stable_height}, usdb_stable_hash={usdb_stable_hash}"
                )
            time.sleep(0.8)

    def wait_ord_server_synced(self) -> None:
        start = time.time()
        status_url = self.args.ord_server_url.rstrip("/") + "/blockcount"
        while True:
            btc_height = int(self.run_btc_cli(None, ["getblockcount"]))
            try:
                with request.urlopen(status_url, timeout=self.args.rpc_timeout_sec) as resp:
                    ord_height = int(resp.read().decode("utf-8").strip())
            except Exception as e:  # noqa: BLE001
                if time.time() - start > self.args.sync_timeout_sec:
                    raise WorldSimError(
                        f"ord sync timeout: target_height={btc_height}, error={e}"
                    ) from e
                time.sleep(0.8)
                continue

            if ord_height >= btc_height:
                return

            if time.time() - start > self.args.sync_timeout_sec:
                raise WorldSimError(
                    "ord sync timeout: "
                    f"target_height={btc_height}, ord_height={ord_height}"
                )
            time.sleep(0.8)

    def random_btc_amount(self, rng: random.Random, min_btc: str, max_btc: str) -> str:
        min_sat = int((Decimal(min_btc) * Decimal("100000000")).to_integral_value())
        max_sat = int((Decimal(max_btc) * Decimal("100000000")).to_integral_value())
        if max_sat <= min_sat:
            sat = min_sat
        else:
            sat = rng.randint(min_sat, max_sat)
        amount = Decimal(sat) / Decimal("100000000")
        return f"{amount:.8f}"

    @classmethod
    def pass_identity_for_action(cls, action: str) -> PassIdentity:
        if action in {"standard_mint", "standard_remint", "invalid_mint"}:
            return PassIdentity(pass_kind="standard")
        if action in cls.FIXED_COLLAB_ACTIONS:
            return PassIdentity(pass_kind="collab", leader_ref_kind="leader_pass_id")
        if action in cls.ADDRESS_COLLAB_ACTIONS:
            return PassIdentity(pass_kind="collab", leader_ref_kind="leader_btc_addr")
        raise WorldSimError(f"action does not define a mint identity: {action}")

    @staticmethod
    def pass_identity_from_snapshot(snapshot: dict[str, Any]) -> PassIdentity:
        leader_pass_id = snapshot.get("leader_pass_id")
        leader_btc_addr = snapshot.get("leader_btc_addr")
        if leader_pass_id:
            return PassIdentity(
                pass_kind=str(snapshot.get("pass_kind", "")),
                leader_ref_kind="leader_pass_id",
                leader_ref_value=str(leader_pass_id),
            )
        if leader_btc_addr:
            return PassIdentity(
                pass_kind=str(snapshot.get("pass_kind", "")),
                leader_ref_kind="leader_btc_addr",
                leader_ref_value=str(leader_btc_addr),
            )
        return PassIdentity(pass_kind=str(snapshot.get("pass_kind", "")))

    def write_mint_content(
        self,
        *,
        usdb_main: str,
        identity: PassIdentity,
        prev: list[str],
        invalid_usdb_main: bool = False,
    ) -> Path:
        payload = {
            "p": "usdb",
            "op": "mint",
            "v": 1,
            "prev": prev,
        }
        if identity.pass_kind == "standard":
            payload["usdb_main"] = "0x123" if invalid_usdb_main else usdb_main
        elif identity.leader_ref_kind == "leader_pass_id":
            if not identity.leader_ref_value:
                raise WorldSimError("fixed collab mint is missing leader_pass_id")
            payload["leader_pass_id"] = identity.leader_ref_value
        elif identity.leader_ref_kind == "leader_btc_addr":
            if not identity.leader_ref_value:
                raise WorldSimError("address collab mint is missing leader_btc_addr")
            payload["leader_btc_addr"] = identity.leader_ref_value
        else:
            raise WorldSimError(f"unsupported mint identity: {identity}")
        fd, path = tempfile.mkstemp(
            prefix="usdb-world-mint-", suffix=".json", dir=self.temp_dir
        )
        os.close(fd)
        content_path = Path(path)
        content_path.write_text(json.dumps(payload, separators=(",", ":")), encoding="utf-8")
        return content_path

    def load_actor_remint_candidates(
        self, actor: Agent, block_height: int
    ) -> list[dict[str, Any]]:
        candidates: list[dict[str, Any]] = []
        for inscription_id in sorted(actor.owned_passes):
            snapshot = self.get_pass_snapshot(inscription_id, block_height)
            if snapshot is None:
                continue
            if str(snapshot.get("owner", "")) != actor.owner_script_hash:
                continue
            if str(snapshot.get("state", "")) not in {"active", "dormant"}:
                continue
            candidates.append(snapshot)
        return candidates

    def load_active_standard_leader_candidates(
        self,
        actor: Agent,
        available_agent_ids: set[int],
        block_height: int,
        preferred_leader_agent_id: int | None = None,
    ) -> list[tuple[Agent, dict[str, Any]]]:
        candidate_ids = sorted(available_agent_ids - {actor.agent_id})
        if preferred_leader_agent_id is not None:
            candidate_ids = [
                agent_id
                for agent_id in candidate_ids
                if agent_id == preferred_leader_agent_id
            ]

        candidates: list[tuple[Agent, dict[str, Any]]] = []
        for agent_id in candidate_ids:
            leader = self.agents[agent_id]
            snapshot = self.get_owner_active_pass_snapshot(
                leader.owner_script_hash, block_height
            )
            if snapshot is None:
                continue
            if (
                str(snapshot.get("state", "")) == "active"
                and str(snapshot.get("pass_kind", "")) == "standard"
            ):
                candidates.append((leader, snapshot))
        return candidates

    def build_mint_plan(
        self,
        *,
        actor: Agent,
        action: str,
        available_agent_ids: set[int],
        pre_height: int,
        rng: random.Random,
        preferred_leader_agent_id: int | None = None,
        preferred_prev_inscription_id: str | None = None,
    ) -> MintPlan:
        base_identity = self.pass_identity_for_action(action)
        prev_inscription_id: str | None = None
        if action in self.REMINT_ACTIONS:
            prev_candidates = self.load_actor_remint_candidates(actor, pre_height)
            if preferred_prev_inscription_id is not None:
                prev_candidates = [
                    snapshot
                    for snapshot in prev_candidates
                    if snapshot.get("inscription_id") == preferred_prev_inscription_id
                ]
            if not prev_candidates:
                raise WorldSimError(
                    "remint requires an actor-owned Active or Dormant prev: "
                    f"actor={actor.wallet_name}, action={action}, pre_height={pre_height}"
                )
            prev_snapshot = rng.choice(prev_candidates)
            prev_inscription_id = str(prev_snapshot.get("inscription_id", ""))
            if not prev_inscription_id:
                raise WorldSimError(f"remint prev snapshot has no inscription id: {prev_snapshot}")

        leader_ref_value: str | None = None
        leader_agent_id: int | None = None
        if base_identity.pass_kind == "collab":
            leader_candidates = self.load_active_standard_leader_candidates(
                actor,
                available_agent_ids,
                pre_height,
                preferred_leader_agent_id,
            )
            if not leader_candidates:
                raise WorldSimError(
                    "collab mint requires another available active standard Leader: "
                    f"actor={actor.wallet_name}, action={action}, pre_height={pre_height}"
                )
            leader, leader_snapshot = rng.choice(leader_candidates)
            leader_agent_id = leader.agent_id
            if base_identity.leader_ref_kind == "leader_pass_id":
                leader_ref_value = str(leader_snapshot.get("inscription_id", ""))
            else:
                leader_ref_value = leader.receive_address
            if not leader_ref_value:
                raise WorldSimError(
                    f"collab Leader reference is empty: action={action}, leader={leader.wallet_name}"
                )

        return MintPlan(
            pass_kind=base_identity.pass_kind,
            prev_inscription_id=prev_inscription_id,
            leader_ref_kind=base_identity.leader_ref_kind,
            leader_ref_value=leader_ref_value,
            leader_agent_id=leader_agent_id,
        )

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

    def choose_actor(self, available_agent_ids: set[int], rng: random.Random) -> int:
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
            return rng.choice(sorted(available_agent_ids))

        x = rng.random() * total
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
            "standard_mint": self.args.standard_mint_probability,
            "fixed_collab_mint": self.args.fixed_collab_mint_probability,
            "address_collab_mint": self.args.address_collab_mint_probability,
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
            "standard_mint": global_prob["standard_mint"] * persona_bias["mint"],
            "fixed_collab_mint": global_prob["fixed_collab_mint"] * persona_bias["mint"],
            "address_collab_mint": global_prob["address_collab_mint"] * persona_bias["mint"],
            "invalid_mint": global_prob["invalid_mint"] * persona_bias["invalid_mint"],
            "transfer": global_prob["transfer"] * persona_bias["transfer"],
            "standard_remint": global_prob["remint"] * persona_bias["remint"] / 3,
            "fixed_collab_remint": global_prob["remint"] * persona_bias["remint"] / 3,
            "address_collab_remint": global_prob["remint"] * persona_bias["remint"] / 3,
            "send_balance": global_prob["send_balance"] * persona_bias["send_balance"],
            "spend_balance": global_prob["spend_balance"] * persona_bias["spend_balance"],
            "noop": noop_base * persona_bias["noop"],
        }

        has_pass = len(agent.owned_passes) > 0
        if not has_pass:
            weights["transfer"] = 0.0
            for action in self.REMINT_ACTIONS:
                weights[action] = 0.0
            weights["standard_mint"] *= 1.3

        if len(available_agent_ids) < 2:
            weights["transfer"] = 0.0

        if agent.cooldown > 0:
            weights["transfer"] *= 0.65
            for action in self.REMINT_ACTIONS:
                weights[action] *= 0.65
            for action in self.MINT_ACTIONS:
                weights[action] *= 0.75

        # Markov-style transition preference based on last action.
        if agent.last_action.endswith("_mint"):
            weights["transfer"] *= 1.25
            weights["send_balance"] *= 1.20
            weights["spend_balance"] *= 1.15
        elif agent.last_action == "transfer":
            for action in self.REMINT_ACTIONS:
                weights[action] *= 1.35
            weights["standard_mint"] *= 1.15
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

        for action in list(weights):
            if action != "noop" and not self.is_action_viable(
                agent, action, available_agent_ids, pre_height
            ):
                weights[action] = 0.0

        return weights

    def choose_action_for_agent(
        self,
        agent: Agent,
        available_agent_ids: set[int],
        pre_height: int,
        rng: random.Random,
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
        x = rng.random() * total
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
            return bool(self.load_actor_remint_candidates(agent, pre_height)) and len(
                available_agent_ids
            ) >= 2
        if action in self.REMINT_ACTIONS:
            if not self.load_actor_remint_candidates(agent, pre_height):
                return False
            if action in self.FIXED_COLLAB_ACTIONS | self.ADDRESS_COLLAB_ACTIONS:
                return bool(
                    self.load_active_standard_leader_candidates(
                        agent, available_agent_ids, pre_height
                    )
                )
            return True
        if action in self.FIXED_COLLAB_ACTIONS | self.ADDRESS_COLLAB_ACTIONS:
            return bool(
                self.load_active_standard_leader_candidates(
                    agent, available_agent_ids, pre_height
                )
            )
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
        action_id: str,
        action: str,
        available_agent_ids: set[int],
        pre_height: int,
        rng: random.Random,
        preferred_leader_agent_id: int | None = None,
        preferred_prev_inscription_id: str | None = None,
    ) -> tuple[str, ActionExpectation, set[int]]:
        plan = self.build_mint_plan(
            actor=actor,
            action=action,
            available_agent_ids=available_agent_ids,
            pre_height=pre_height,
            rng=rng,
            preferred_leader_agent_id=preferred_leader_agent_id,
            preferred_prev_inscription_id=preferred_prev_inscription_id,
        )
        identity = PassIdentity(
            pass_kind=plan.pass_kind,
            leader_ref_kind=plan.leader_ref_kind,
            leader_ref_value=plan.leader_ref_value,
        )
        invalid_usdb_main = action == "invalid_mint"
        prev = [plan.prev_inscription_id] if plan.prev_inscription_id else []
        content_path = self.write_mint_content(
            usdb_main=actor.usdb_main_address,
            identity=identity,
            prev=prev,
            invalid_usdb_main=invalid_usdb_main,
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
        self.write_external_action_result(
            action_id=action_id,
            action=action,
            raw_output=output,
        )
        inscription_id = self.extract_inscription_id(output)
        self.pass_owner_by_id[inscription_id] = actor.agent_id
        self.pass_identity_by_id[inscription_id] = identity
        actor.owned_passes.add(inscription_id)
        used_agent_ids = {actor.agent_id}
        if plan.leader_agent_id is not None:
            used_agent_ids.add(plan.leader_agent_id)
        if invalid_usdb_main:
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
                    pass_kind="standard",
                ),
                used_agent_ids,
            )

        self.metrics[f"{action}_ok"] += 1
        pre_balance = self.get_balance_at_height(actor.owner_script_hash, pre_height)
        leader_detail = (
            f":{plan.leader_ref_kind}={plan.leader_ref_value}"
            if plan.leader_ref_kind and plan.leader_ref_value
            else ""
        )
        return (
            (
                f"{action}:{inscription_id}:owner={actor.wallet_name}:prev={prev[0]}{leader_detail}"
                if prev
                else f"{action}:{inscription_id}:owner={actor.wallet_name}{leader_detail}"
            ),
            ActionExpectation(
                action=action,
                actor_id=actor.agent_id,
                inscription_id=inscription_id,
                prev_inscription_id=prev[0] if prev else None,
                actor_pre_balance=pre_balance,
                pass_kind=plan.pass_kind,
                leader_ref_kind=plan.leader_ref_kind,
                leader_ref_value=plan.leader_ref_value,
            ),
            used_agent_ids,
        )

    def op_transfer(
        self,
        actor: Agent,
        action_id: str,
        available_agent_ids: set[int],
        pre_height: int,
        rng: random.Random,
    ) -> tuple[str, ActionExpectation, set[int]]:
        transfer_candidates = self.load_actor_remint_candidates(actor, pre_height)
        if not transfer_candidates:
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

        inscription_id = str(rng.choice(transfer_candidates).get("inscription_id", ""))
        if not inscription_id:
            raise WorldSimError(
                f"transfer candidate has no inscription id: actor={actor.wallet_name}"
            )
        target = rng.choice(target_candidates)
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
        self.write_external_action_result(
            action_id=action_id,
            action="transfer",
            raw_output=output,
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

    def op_send_balance(
        self, actor: Agent, action_id: str, pre_height: int, rng: random.Random
    ) -> tuple[str, ActionExpectation]:
        amount_btc = self.random_btc_amount(rng, "0.01000000", "0.25000000")
        txid = self.run_btc_cli(
            self.args.miner_wallet,
            [
                "sendtoaddress",
                actor.receive_address,
                amount_btc,
                f"usdb-world-sim:{action_id}",
                actor.wallet_name,
            ],
        )
        self.write_external_action_result(
            action_id=action_id,
            action="send_balance",
            raw_output=txid,
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
        self, actor: Agent, action_id: str, pre_height: int, rng: random.Random
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
            amount_sat = rng.randint(min_sat, max_sat)

        amount_btc = f"{(Decimal(amount_sat) / Decimal('100000000')):.8f}"
        txid = self.run_btc_cli(
            actor.wallet_name,
            [
                "sendtoaddress",
                self.args.mining_address,
                amount_btc,
                f"usdb-world-sim:{action_id}",
                self.args.miner_wallet,
            ],
        )
        self.write_external_action_result(
            action_id=action_id,
            action="spend_balance",
            raw_output=txid,
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

    def execute_agent_action(
        self,
        actor: Agent,
        action_id: str,
        action: str,
        available_agent_ids: set[int],
        pre_height: int,
        rng: random.Random,
    ) -> tuple[str, ActionExpectation | None, set[int]]:
        if action == "noop":
            self.metrics["skip"] += 1
            return "noop", None, {actor.agent_id}

        if action in self.MINT_ACTIONS | self.REMINT_ACTIONS:
            detail, expectation, used_ids = self.op_mint(
                actor=actor,
                action_id=action_id,
                action=action,
                available_agent_ids=available_agent_ids,
                pre_height=pre_height,
                rng=rng,
            )
            return detail, expectation, used_ids

        if action == "transfer":
            return self.op_transfer(actor, action_id, available_agent_ids, pre_height, rng)

        if action == "send_balance":
            detail, expectation = self.op_send_balance(actor, action_id, pre_height, rng)
            return detail, expectation, {actor.agent_id}

        if action == "spend_balance":
            result = self.op_spend_balance(actor, action_id, pre_height, rng)
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
        if action in self.MINT_ACTIONS | self.REMINT_ACTIONS:
            self.metrics[f"{action}_fail"] += 1
            return
        if action == "transfer":
            self.metrics["transfer_fail"] += 1
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

        if expectation.action in self.MINT_ACTIONS | self.REMINT_ACTIONS:
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

            expected_pass_kind = expectation.pass_kind or "standard"
            actual_pass_kind = str(snapshot.get("pass_kind", ""))
            if actual_pass_kind != expected_pass_kind:
                raise WorldSimError(
                    "mint-like pass kind mismatch: "
                    f"inscription_id={inscription_id}, expected={expected_pass_kind}, "
                    f"got={actual_pass_kind}"
                )
            if snapshot.get("leader_pass_id") != (
                expectation.leader_ref_value
                if expectation.leader_ref_kind == "leader_pass_id"
                else None
            ):
                raise WorldSimError(
                    "mint-like fixed Leader reference mismatch: "
                    f"inscription_id={inscription_id}, snapshot={snapshot.get('leader_pass_id')}, "
                    f"expectation={expectation.leader_ref_value}"
                )
            if snapshot.get("leader_btc_addr") != (
                expectation.leader_ref_value
                if expectation.leader_ref_kind == "leader_btc_addr"
                else None
            ):
                raise WorldSimError(
                    "mint-like address Leader reference mismatch: "
                    f"inscription_id={inscription_id}, snapshot={snapshot.get('leader_btc_addr')}, "
                    f"expectation={expectation.leader_ref_value}"
                )

            state_ref = self.get_state_ref_at_height(block_height)
            if state_ref is None:
                raise WorldSimError(
                    f"mint-like verification missing state ref at height={block_height}"
                )
            context = self.build_consensus_context_from_state_ref(state_ref)
            profile = self.load_pass_economic_profile_at_height(
                inscription_id, block_height, context
            )

            if expectation.expect_invalid:
                if state != "invalid":
                    raise WorldSimError(
                        "invalid_mint verification failed: "
                        f"inscription_id={inscription_id}, state={state}"
                    )
                self.assert_zero_economic_profile(
                    profile,
                    inscription_id=inscription_id,
                    expected_state="invalid",
                )
                return

            if state not in {"active", "dormant"}:
                raise WorldSimError(
                    "mint/remint verification failed: "
                    f"inscription_id={inscription_id}, state={state}"
                )

            if profile.get("state") != state or profile.get("pass_kind") != expected_pass_kind:
                raise WorldSimError(
                    "mint-like profile disagrees with pass snapshot: "
                    f"inscription_id={inscription_id}, snapshot_state={state}, "
                    f"snapshot_kind={expected_pass_kind}, profile={profile}"
                )
            if profile.get("owner_script_hash") != actor.owner_script_hash:
                raise WorldSimError(
                    "mint-like profile owner mismatch: "
                    f"inscription_id={inscription_id}, expected={actor.owner_script_hash}, "
                    f"got={profile.get('owner_script_hash')}"
                )
            if expected_pass_kind == "collab":
                collab_boundary = {
                    "collab_contribution": "0",
                    "effective_energy": "0",
                    "level": 0,
                    "difficulty_factor_bps": 10_000,
                    "collab_breakdown_count": 0,
                }
                for field_name, expected_value in collab_boundary.items():
                    if profile.get(field_name) != expected_value:
                        raise WorldSimError(
                            "active collab profile boundary mismatch: "
                            f"inscription_id={inscription_id}, field={field_name}, "
                            f"expected={expected_value}, got={profile.get(field_name)}"
                        )

            if expectation.action in self.REMINT_ACTIONS and expectation.prev_inscription_id:
                prev = snapshot.get("prev") or []
                if expectation.prev_inscription_id not in prev:
                    raise WorldSimError(
                        "remint verification failed: "
                        f"inscription_id={inscription_id}, prev={prev}, "
                        f"expected_prev={expectation.prev_inscription_id}"
                    )
                prev_snapshot = self.get_pass_snapshot(
                    expectation.prev_inscription_id, block_height
                )
                if prev_snapshot is None or prev_snapshot.get("state") != "consumed":
                    raise WorldSimError(
                        "remint did not consume prev atomically: "
                        f"child={inscription_id}, prev={expectation.prev_inscription_id}, "
                        f"prev_snapshot={prev_snapshot}"
                    )
                prev_profile = self.load_pass_economic_profile_at_height(
                    expectation.prev_inscription_id, block_height, context
                )
                self.assert_zero_economic_profile(
                    prev_profile,
                    inscription_id=expectation.prev_inscription_id,
                    expected_state="consumed",
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
        self.pass_identity_by_id[inscription_id] = self.pass_identity_from_snapshot(
            active_snapshot
        )

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
        return sorted(
            self.diagnostic_position_rng("agent-self-check", tick).sample(
                ordered, sample_size
            )
        )

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
            agent.oracle_last_active_block_height = None
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
        active_block_height = int(
            energy_snapshot.get("active_block_height", record_block_height)
        )
        state = str(energy_snapshot.get("state", ""))
        owner_address = str(energy_snapshot.get("owner_address", ""))
        owner_balance = int(energy_snapshot.get("owner_balance", 0))
        owner_delta = int(energy_snapshot.get("owner_delta", 0))
        energy = int(energy_snapshot.get("raw_energy", 0))

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
        prev_active_block_height = agent.oracle_last_active_block_height

        # Strict numeric oracle when check cadence is consecutive and active pass is stable.
        if (
            prev_height is not None
            and prev_pass_id == active_pass_id
            and prev_state == "active"
            and prev_energy is not None
            and prev_owner_balance is not None
            and prev_active_block_height is not None
            and block_height == prev_height + 1
        ):
            expected_energy = self.saturating_energy_add(
                prev_energy, self.calc_growth_delta(prev_owner_balance, 1)
            )
            if record_block_height == block_height and owner_delta < 0:
                expected_energy = self.saturating_energy_sub(
                    expected_energy,
                    self.calc_balance_penalty(
                        prev_owner_balance,
                        owner_balance,
                        prev_active_block_height,
                        block_height,
                    ),
                )

            if energy != expected_energy:
                raise WorldSimError(
                    "agent self-check energy mismatch: "
                    f"agent={agent.wallet_name}, inscription_id={active_pass_id}, "
                    f"block_height={block_height}, prev_height={prev_height}, "
                    f"prev_energy={prev_energy}, prev_owner_balance={prev_owner_balance}, "
                    f"prev_active_block_height={prev_active_block_height}, "
                    f"record_block_height={record_block_height}, owner_delta={owner_delta}, "
                    f"expected_energy={expected_energy}, actual_energy={energy}"
                )

        agent.oracle_last_checked_height = block_height
        agent.oracle_last_pass_id = active_pass_id
        agent.oracle_last_state = state
        agent.oracle_last_energy = energy
        agent.oracle_last_owner_balance = owner_balance
        agent.oracle_last_record_block_height = record_block_height
        agent.oracle_last_active_block_height = active_block_height

    def should_run_global_cross_check(self, tick: int) -> bool:
        if not self.args.global_cross_check_enabled:
            return False
        interval = self.args.global_cross_check_interval_blocks
        if interval <= 0:
            return False
        return tick % interval == 0

    def load_all_active_passes_at_height(self, block_height: int) -> list[dict[str, Any]]:
        page = 0
        page_size = 256
        rows: list[dict[str, Any]] = []

        while True:
            payload = self.rpc_usdb(
                "get_active_passes_at_height",
                [{"at_height": block_height, "page": page, "page_size": page_size}],
            )
            if not isinstance(payload, dict):
                raise WorldSimError(
                    "global cross-check invalid get_active_passes_at_height payload: "
                    f"block_height={block_height}, page={page}, payload={payload}"
                )

            resolved_height = int(payload.get("resolved_height", -1))
            if resolved_height != block_height:
                raise WorldSimError(
                    "global cross-check active-pass resolved height mismatch: "
                    f"expected={block_height}, got={resolved_height}, page={page}"
                )

            items = payload.get("items") or []
            if not isinstance(items, list):
                raise WorldSimError(
                    "global cross-check invalid active-pass items type: "
                    f"block_height={block_height}, page={page}, items={items}"
                )

            if not items:
                break
            rows.extend(items)
            if len(items) < page_size:
                break
            page += 1

        return rows

    def load_pass_energy_leaderboard_at_height(
        self, block_height: int, scope: str
    ) -> list[dict[str, Any]]:
        page = 0
        page_size = 256
        rows: list[dict[str, Any]] = []

        while True:
            payload = self.rpc_usdb(
                "get_pass_energy_leaderboard",
                [
                    {
                        "at_height": block_height,
                        "scope": scope,
                        "page": page,
                        "page_size": page_size,
                    }
                ],
            )
            if not isinstance(payload, dict):
                raise WorldSimError(
                    "invalid get_pass_energy_leaderboard payload: "
                    f"block_height={block_height}, scope={scope}, page={page}, payload={payload}"
                )

            resolved_height = int(payload.get("resolved_height", -1))
            if resolved_height != block_height:
                raise WorldSimError(
                    "leaderboard resolved height mismatch: "
                    f"expected={block_height}, got={resolved_height}, scope={scope}, page={page}"
                )

            items = payload.get("items") or []
            if not isinstance(items, list):
                raise WorldSimError(
                    "invalid leaderboard items type: "
                    f"block_height={block_height}, scope={scope}, page={page}, items={items}"
                )

            if not items:
                break
            rows.extend(items)
            if len(items) < page_size:
                break
            page += 1

        return rows

    def cross_check_economic_views(
        self,
        *,
        block_height: int,
        context: dict[str, Any],
        active_rows: list[dict[str, Any]],
    ) -> dict[str, int]:
        candidate_view = self.load_candidate_set_view_at_height(block_height, context)
        external_state = candidate_view["external_state"]
        candidate_items = candidate_view["items"]
        candidate_by_id = {
            str(item.get("pass_id", "")): item for item in candidate_items
        }

        active_standard_by_id: dict[str, dict[str, Any]] = {}
        active_standard_by_owner: dict[str, dict[str, Any]] = {}
        active_collab_by_id: dict[str, dict[str, Any]] = {}
        for row in active_rows:
            pass_id = str(row.get("inscription_id", ""))
            snapshot = self.get_pass_snapshot(pass_id, block_height)
            if snapshot is None:
                raise WorldSimError(
                    "economic cross-check missing active pass snapshot: "
                    f"pass={pass_id}, block_height={block_height}"
                )
            if snapshot.get("state") != "active":
                raise WorldSimError(
                    f"active-pass view returned non-active snapshot: pass={pass_id}, snapshot={snapshot}"
                )
            pass_kind = str(snapshot.get("pass_kind", ""))
            owner = str(snapshot.get("owner", ""))
            if pass_kind == "standard":
                active_standard_by_id[pass_id] = snapshot
                if owner in active_standard_by_owner:
                    raise WorldSimError(
                        "duplicate active standard owner during economic cross-check: "
                        f"owner={owner}"
                    )
                active_standard_by_owner[owner] = snapshot
            elif pass_kind == "collab":
                active_collab_by_id[pass_id] = snapshot
            else:
                raise WorldSimError(
                    f"active pass has unsupported kind: pass={pass_id}, kind={pass_kind}"
                )

        expected_candidate_ids = set(active_standard_by_id)
        actual_candidate_ids = set(candidate_by_id)
        if actual_candidate_ids != expected_candidate_ids:
            raise WorldSimError(
                "candidate set does not equal active standard set: "
                f"missing={sorted(expected_candidate_ids - actual_candidate_ids)}, "
                f"unexpected={sorted(actual_candidate_ids - expected_candidate_ids)}"
            )

        profiles: dict[str, dict[str, Any]] = {}
        for pass_id in sorted(active_standard_by_id):
            profile = self.load_pass_economic_profile_at_height(
                pass_id,
                block_height,
                context,
                expected_external_state=external_state,
            )
            profiles[pass_id] = profile
            candidate = self.candidate_from_view_item(candidate_by_id[pass_id])
            profile_candidate = self.candidate_from_view_item(profile)
            self.assert_validator_candidate_matches(
                candidate, profile_candidate, f"global-economic:{block_height}:{pass_id}"
            )

        for pass_id in sorted(active_collab_by_id):
            if pass_id in candidate_by_id:
                raise WorldSimError(
                    f"active collab pass leaked into candidate set: pass={pass_id}"
                )
            profile = self.load_pass_economic_profile_at_height(
                pass_id,
                block_height,
                context,
                expected_external_state=external_state,
            )
            profiles[pass_id] = profile
            expected_boundary = {
                "state": "active",
                "pass_kind": "collab",
                "collab_contribution": "0",
                "effective_energy": "0",
                "level": 0,
                "difficulty_factor_bps": self.BPS_DENOMINATOR,
                "collab_breakdown_count": 0,
            }
            for field_name, expected_value in expected_boundary.items():
                if profile.get(field_name) != expected_value:
                    raise WorldSimError(
                        "active collab profile boundary mismatch: "
                        f"pass={pass_id}, field={field_name}, expected={expected_value}, "
                        f"got={profile.get(field_name)}"
                    )

        expected_collabs_by_leader: dict[str, set[str]] = {
            pass_id: set() for pass_id in active_standard_by_id
        }
        unresolved_collab_count = 0
        for pass_id, snapshot in active_collab_by_id.items():
            leader_id = self.resolve_expected_collab_leader(
                snapshot, active_standard_by_id, active_standard_by_owner
            )
            if leader_id is None:
                unresolved_collab_count += 1
                continue
            expected_collabs_by_leader[leader_id].add(pass_id)

        breakdown_row_count = 0
        for leader_id in sorted(active_standard_by_id):
            by_pass_id = self.load_collab_breakdown_at_height(
                leader_id,
                block_height,
                context,
                sort="collab_pass_id_asc",
                expected_external_state=external_state,
            )
            by_contribution = self.load_collab_breakdown_at_height(
                leader_id,
                block_height,
                context,
                sort="contribution_desc_pass_id_asc",
                expected_external_state=external_state,
            )
            for breakdown in (by_pass_id, by_contribution):
                if breakdown.get("leader_state") != "active":
                    raise WorldSimError(
                        f"active standard Leader breakdown state mismatch: leader={leader_id}"
                    )
                if breakdown.get("leader_pass_kind") != "standard":
                    raise WorldSimError(
                        f"active Leader breakdown kind mismatch: leader={leader_id}"
                    )
            if by_pass_id["aggregate_collab_contribution"] != by_contribution[
                "aggregate_collab_contribution"
            ]:
                raise WorldSimError(
                    f"collab breakdown sorts disagree on aggregate: leader={leader_id}"
                )
            rows_by_id = {
                str(item["collab_pass_id"]): item for item in by_pass_id["items"]
            }
            contribution_rows_by_id = {
                str(item["collab_pass_id"]): item
                for item in by_contribution["items"]
            }
            if rows_by_id != contribution_rows_by_id:
                raise WorldSimError(
                    f"collab breakdown sorts disagree on row set: leader={leader_id}"
                )
            expected_collab_ids = expected_collabs_by_leader[leader_id]
            if set(rows_by_id) != expected_collab_ids:
                raise WorldSimError(
                    "collab breakdown does not match independently resolved Leader bindings: "
                    f"leader={leader_id}, missing={sorted(expected_collab_ids - set(rows_by_id))}, "
                    f"unexpected={sorted(set(rows_by_id) - expected_collab_ids)}"
                )

            for collab_id, item in rows_by_id.items():
                snapshot = active_collab_by_id[collab_id]
                profile = profiles[collab_id]
                if item.get("collab_owner_script_hash") != snapshot.get("owner"):
                    raise WorldSimError(
                        f"collab breakdown owner mismatch: pass={collab_id}"
                    )
                if int(item.get("collab_raw_energy", 0)) != int(
                    profile.get("raw_energy", 0)
                ):
                    raise WorldSimError(
                        f"collab breakdown raw energy mismatch with profile: pass={collab_id}"
                    )
                if snapshot.get("leader_pass_id"):
                    expected_ref_kind = "leader_pass_id"
                    expected_ref_value = str(snapshot["leader_pass_id"])
                else:
                    expected_ref_kind = "leader_btc_addr"
                    expected_ref_value = str(snapshot.get("leader_btc_addr", ""))
                if (
                    item.get("leader_ref_kind") != expected_ref_kind
                    or item.get("leader_ref_value") != expected_ref_value
                ):
                    raise WorldSimError(
                        "collab breakdown Leader reference mismatch: "
                        f"pass={collab_id}, expected={expected_ref_kind}:{expected_ref_value}, "
                        f"got={item.get('leader_ref_kind')}:{item.get('leader_ref_value')}"
                    )

            aggregate_contribution = int(
                by_pass_id["aggregate_collab_contribution"]
            )
            leader_profile = profiles[leader_id]
            if int(leader_profile.get("collab_contribution", 0)) != aggregate_contribution:
                raise WorldSimError(
                    "Leader profile contribution differs from breakdown aggregate: "
                    f"leader={leader_id}, profile={leader_profile.get('collab_contribution')}, "
                    f"breakdown={aggregate_contribution}"
                )
            if int(leader_profile.get("collab_breakdown_count", -1)) != len(rows_by_id):
                raise WorldSimError(
                    "Leader profile breakdown count mismatch: "
                    f"leader={leader_id}, profile={leader_profile.get('collab_breakdown_count')}, "
                    f"rows={len(rows_by_id)}"
                )
            expected_effective = self.saturating_energy_add(
                int(leader_profile.get("raw_energy", 0)), aggregate_contribution
            )
            if int(leader_profile.get("effective_energy", 0)) != expected_effective:
                raise WorldSimError(
                    "Leader effective energy cannot be reproduced: "
                    f"leader={leader_id}, expected={expected_effective}, "
                    f"got={leader_profile.get('effective_energy')}"
                )
            breakdown_row_count += len(rows_by_id)

        return {
            "candidate_count": len(candidate_items),
            "active_standard_count": len(active_standard_by_id),
            "active_collab_count": len(active_collab_by_id),
            "resolved_collab_count": sum(
                len(pass_ids) for pass_ids in expected_collabs_by_leader.values()
            ),
            "unresolved_collab_count": unresolved_collab_count,
            "breakdown_row_count": breakdown_row_count,
        }

    def reset_local_chain_view(self) -> None:
        self.pass_owner_by_id.clear()
        self.pass_identity_by_id.clear()
        for agent in self.agents:
            agent.owned_passes.clear()
            agent.active_pass_id = None
            agent.invalid_passes.clear()
            agent.last_action = "reorg_reset"
            agent.cooldown = 0
            agent.oracle_last_checked_height = None
            agent.oracle_last_pass_id = None
            agent.oracle_last_state = None
            agent.oracle_last_energy = None
            agent.oracle_last_owner_balance = None
            agent.oracle_last_record_block_height = None
            agent.oracle_last_active_block_height = None

    def rebuild_local_chain_view_from_height(self, block_height: int) -> dict[str, int]:
        self.reset_local_chain_view()

        owner_to_agent = {
            agent.owner_script_hash: agent for agent in self.agents
        }
        rows = self.load_pass_energy_leaderboard_at_height(block_height, "all")
        unknown_owner_rows = 0
        active_owner_rows = 0

        for row in rows:
            inscription_id = str(row.get("inscription_id", ""))
            owner = str(row.get("owner", ""))
            state = str(row.get("state", ""))
            if not inscription_id or not owner:
                raise WorldSimError(
                    f"invalid leaderboard row during reorg rebuild: row={row}"
                )

            agent = owner_to_agent.get(owner)
            if agent is None:
                unknown_owner_rows += 1
                continue

            agent.owned_passes.add(inscription_id)
            self.pass_owner_by_id[inscription_id] = agent.agent_id
            snapshot = self.get_pass_snapshot(inscription_id, block_height)
            if snapshot is None:
                raise WorldSimError(
                    "missing pass snapshot during reorg rebuild: "
                    f"pass={inscription_id}, block_height={block_height}"
                )
            self.pass_identity_by_id[inscription_id] = self.pass_identity_from_snapshot(
                snapshot
            )

            if state == "invalid":
                agent.invalid_passes.add(inscription_id)
            if state == "active":
                if agent.active_pass_id is not None and agent.active_pass_id != inscription_id:
                    raise WorldSimError(
                        "duplicate active pass detected during reorg rebuild: "
                        f"owner={owner}, existing={agent.active_pass_id}, new={inscription_id}"
                    )
                agent.active_pass_id = inscription_id
                active_owner_rows += 1

        if unknown_owner_rows > 0:
            raise WorldSimError(
                "reorg rebuild found rows owned by unknown script hashes: "
                f"block_height={block_height}, unknown_owner_rows={unknown_owner_rows}"
            )

        return {
            "loaded_pass_rows": len(rows),
            "unknown_owner_rows": unknown_owner_rows,
            "active_owner_rows": active_owner_rows,
        }

    def get_block_hash(self, block_height: int) -> str:
        return self.run_btc_cli(None, ["getblockhash", str(block_height)])

    def mine_one_empty_block(self) -> int:
        self.run_btc_cli(
            None,
            [
                "-named",
                "generateblock",
                f"output={self.args.mining_address}",
                "transactions=[]",
            ],
        )
        return int(self.run_btc_cli(None, ["getblockcount"]))

    def should_trigger_reorg(self, tick: int, block_height: int) -> bool:
        if self.args.reorg_interval_blocks <= 0:
            return False
        if self.args.reorg_depth <= 0:
            return False
        if tick % self.args.reorg_interval_blocks != 0:
            return False
        if self.args.reorg_max_events > 0 and self.reorg_events_applied >= self.args.reorg_max_events:
            return False
        return block_height > self.args.reorg_depth

    def perform_reorg(self, tick: int, block_height: int) -> dict[str, Any]:
        depth = min(self.args.reorg_depth, block_height - 1)
        rollback_start_height = block_height - depth + 1
        rollback_target_height = rollback_start_height - 1
        original_tip_hash = self.get_block_hash(block_height)
        rollback_start_hash = self.get_block_hash(rollback_start_height)

        self.log(
            "Applying deterministic reorg: "
            f"tick={tick}, tip_height={block_height}, depth={depth}, "
            f"rollback_start_height={rollback_start_height}, rollback_target_height={rollback_target_height}"
        )

        self.run_btc_cli(None, ["invalidateblock", rollback_start_hash])
        self.wait_ord_server_synced()
        # On regtest, balance-history rolls back immediately after invalidateblock,
        # while usdb-indexer may only converge after replacement blocks appear.
        # Waiting for both services to hit the rollback target can hang here even
        # though final replacement-tip reconciliation works. For world-sim we only
        # need the upstream rollback to be visible before mining the replacement chain.
        self.wait_balance_history_height_exact(rollback_target_height)

        for _ in range(depth):
            self.mine_one_empty_block()

        self.wait_ord_server_synced()
        self.wait_service_synced(block_height)

        replacement_tip_hash = self.get_block_hash(block_height)
        if replacement_tip_hash == original_tip_hash:
            raise WorldSimError(
                "deterministic reorg failed to change tip hash: "
                f"tick={tick}, block_height={block_height}, tip_hash={replacement_tip_hash}"
            )

        bh_stable_hash, usdb_stable_hash = self.wait_snapshot_hashes(
            block_height, replacement_tip_hash
        )
        if bh_stable_hash != replacement_tip_hash:
            raise WorldSimError(
                "balance-history stable hash mismatch after reorg: "
                f"expected={replacement_tip_hash}, got={bh_stable_hash}"
            )
        if usdb_stable_hash != replacement_tip_hash:
            raise WorldSimError(
                "usdb stable hash mismatch after reorg: "
                f"expected={replacement_tip_hash}, got={usdb_stable_hash}"
            )

        rebuild_info = self.rebuild_local_chain_view_from_height(block_height)
        cross_check_info = self.run_global_cross_check(block_height, tick)
        invalidated_sample_ids: list[str] = []
        for sample in self.validator_samples:
            if sample.validated:
                continue
            if rollback_start_height <= sample.block_height <= block_height:
                sample.expected_consensus_error = "SNAPSHOT_ID_MISMATCH"
                sample.invalidated_by_reorg_tick = tick
                invalidated_sample_ids.append(sample.sample_id)
        self.reorg_events_applied += 1
        self.metrics["reorg_ok"] += 1

        info = {
            "tick": tick,
            "depth": depth,
            "rollback_start_height": rollback_start_height,
            "rollback_target_height": rollback_target_height,
            "tip_height": block_height,
            "original_tip_hash": original_tip_hash,
            "replacement_tip_hash": replacement_tip_hash,
            "balance_history_stable_hash": bh_stable_hash,
            "usdb_stable_hash": usdb_stable_hash,
        }
        info.update(rebuild_info)
        info["global_cross_check_info"] = cross_check_info
        info["validator_sample_invalidated_ids"] = invalidated_sample_ids
        self.emit_report("reorg", info)
        return info

    def run_global_cross_check(self, block_height: int, tick: int) -> dict[str, Any]:
        start_ts = time.time()
        top_n = max(1, self.args.global_cross_check_leaderboard_top_n)
        owner_sample_size = self.args.global_cross_check_owner_sample_size

        aggregate_view = self.rpc_usdb(
            "get_miner_economic_aggregate",
            [
                {
                    "view_version": self.ECONOMIC_VIEW_VERSION,
                    "block_height": block_height,
                    "context": None,
                }
            ],
        )
        if not isinstance(aggregate_view, dict):
            raise WorldSimError(
                "global cross-check missing exact miner economic aggregate: "
                f"tick={tick}, block_height={block_height}, payload={aggregate_view}"
            )
        aggregate_state = aggregate_view.get("external_state") or {}
        miner_aggregate = aggregate_view.get("miner_aggregate") or {}
        snapshot_height = int(aggregate_state.get("btc_height", -1))
        snapshot_total_balance = int(
            miner_aggregate.get("total_miner_btc_sats", 0)
        )
        snapshot_active_count = int(
            miner_aggregate.get("active_miner_owner_count", 0)
        )
        if snapshot_height != block_height:
            raise WorldSimError(
                "global cross-check aggregate height mismatch: "
                f"tick={tick}, expected={block_height}, got={snapshot_height}"
            )

        leaderboard = self.rpc_usdb(
            "get_pass_energy_leaderboard",
            [{"at_height": block_height, "page": 0, "page_size": top_n}],
        )
        if not isinstance(leaderboard, dict):
            raise WorldSimError(
                "global cross-check invalid leaderboard payload: "
                f"tick={tick}, block_height={block_height}, payload={leaderboard}"
            )
        leaderboard_height = int(leaderboard.get("resolved_height", -1))
        if leaderboard_height != block_height:
            raise WorldSimError(
                "global cross-check leaderboard height mismatch: "
                f"tick={tick}, expected={block_height}, got={leaderboard_height}"
            )
        leaderboard_items = leaderboard.get("items") or []
        if not isinstance(leaderboard_items, list):
            raise WorldSimError(
                "global cross-check invalid leaderboard items type: "
                f"tick={tick}, block_height={block_height}, items={leaderboard_items}"
            )

        compared_top_items = 0
        for item in leaderboard_items:
            inscription_id = str(item.get("inscription_id", ""))
            owner = str(item.get("owner", ""))
            expected_energy = int(item.get("energy", 0))
            expected_state = str(item.get("state", ""))
            expected_record_height = int(item.get("record_block_height", -1))
            if not inscription_id:
                raise WorldSimError(
                    "global cross-check leaderboard item missing inscription_id: "
                    f"tick={tick}, block_height={block_height}, item={item}"
                )

            energy_snapshot = self.get_pass_energy_snapshot(
                inscription_id, block_height, mode="at_or_before"
            )
            if energy_snapshot is None:
                raise WorldSimError(
                    "global cross-check missing pass energy for leaderboard item: "
                    f"tick={tick}, block_height={block_height}, inscription_id={inscription_id}"
                )

            actual_query_height = int(
                energy_snapshot.get("query_block_height", block_height)
            )
            actual_record_height = int(
                energy_snapshot.get("record_block_height", -1)
            )
            actual_owner = str(energy_snapshot.get("owner_address", ""))
            actual_state = str(energy_snapshot.get("state", ""))
            actual_energy = int(energy_snapshot.get("raw_energy", 0))

            if actual_query_height != block_height:
                raise WorldSimError(
                    "global cross-check leaderboard query height mismatch: "
                    f"tick={tick}, block_height={block_height}, inscription_id={inscription_id}, "
                    f"actual_query_height={actual_query_height}"
                )
            if actual_record_height != expected_record_height:
                raise WorldSimError(
                    "global cross-check leaderboard record height mismatch: "
                    f"tick={tick}, inscription_id={inscription_id}, expected={expected_record_height}, got={actual_record_height}"
                )
            if actual_owner != owner:
                raise WorldSimError(
                    "global cross-check leaderboard owner mismatch: "
                    f"tick={tick}, inscription_id={inscription_id}, expected_owner={owner}, got_owner={actual_owner}"
                )
            if actual_state != expected_state:
                raise WorldSimError(
                    "global cross-check leaderboard state mismatch: "
                    f"tick={tick}, inscription_id={inscription_id}, expected_state={expected_state}, got_state={actual_state}"
                )
            if actual_energy != expected_energy:
                raise WorldSimError(
                    "global cross-check leaderboard energy mismatch: "
                    f"tick={tick}, inscription_id={inscription_id}, expected_energy={expected_energy}, got_energy={actual_energy}"
                )
            compared_top_items += 1

        active_rows = self.load_all_active_passes_at_height(block_height)
        state_ref = self.get_state_ref_at_height(block_height)
        if state_ref is None:
            raise WorldSimError(
                f"global cross-check missing state ref at height={block_height}"
            )
        economic_info = self.cross_check_economic_views(
            block_height=block_height,
            context=self.build_consensus_context_from_state_ref(state_ref),
            active_rows=active_rows,
        )
        owner_to_pass: dict[str, str] = {}
        for row in active_rows:
            owner = str(row.get("owner", ""))
            inscription_id = str(row.get("inscription_id", ""))
            if not owner or not inscription_id:
                raise WorldSimError(
                    "global cross-check invalid active-pass row: "
                    f"tick={tick}, block_height={block_height}, row={row}"
                )
            if owner in owner_to_pass:
                raise WorldSimError(
                    "global cross-check duplicate active owner from active-pass view: "
                    f"tick={tick}, block_height={block_height}, owner={owner}, "
                    f"pass_a={owner_to_pass[owner]}, pass_b={inscription_id}"
                )
            owner_to_pass[owner] = inscription_id

        if len(owner_to_pass) != snapshot_active_count:
            raise WorldSimError(
                "global cross-check active owner count mismatch: "
                f"tick={tick}, block_height={block_height}, owner_count={len(owner_to_pass)}, "
                f"snapshot_active_count={snapshot_active_count}"
            )

        owners = sorted(owner_to_pass.keys())
        if owner_sample_size <= 0 or owner_sample_size >= len(owners):
            sampled_owners = owners
        else:
            sampled_owners = sorted(
                self.diagnostic_position_rng(
                    "global-cross-check-owner-sample", tick, block_height
                ).sample(owners, owner_sample_size)
            )

        sampled_balance_sum = 0
        for owner in sampled_owners:
            inscription_id = owner_to_pass[owner]
            bh_balance = self.get_balance_at_height(owner, block_height)
            sampled_balance_sum += bh_balance

            owner_active = self.get_owner_active_pass_snapshot(owner, block_height)
            if owner_active is None:
                raise WorldSimError(
                    "global cross-check missing owner active pass snapshot: "
                    f"tick={tick}, block_height={block_height}, owner={owner}"
                )
            owner_active_id = str(owner_active.get("inscription_id", ""))
            if owner_active_id != inscription_id:
                raise WorldSimError(
                    "global cross-check owner active pass mismatch: "
                    f"tick={tick}, owner={owner}, expected_pass={inscription_id}, got_pass={owner_active_id}"
                )

            energy_snapshot = self.get_pass_energy_snapshot(
                inscription_id, block_height, mode="at_or_before"
            )
            if energy_snapshot is None:
                raise WorldSimError(
                    "global cross-check missing energy for sampled owner pass: "
                    f"tick={tick}, owner={owner}, inscription_id={inscription_id}"
                )
            owner_from_energy = str(energy_snapshot.get("owner_address", ""))
            owner_balance_from_energy = int(energy_snapshot.get("owner_balance", 0))
            if owner_from_energy != owner:
                raise WorldSimError(
                    "global cross-check sampled owner mismatch in energy snapshot: "
                    f"tick={tick}, inscription_id={inscription_id}, expected_owner={owner}, got_owner={owner_from_energy}"
                )
            if owner_balance_from_energy != bh_balance:
                raise WorldSimError(
                    "global cross-check sampled owner balance mismatch: "
                    f"tick={tick}, owner={owner}, inscription_id={inscription_id}, "
                    f"balance_history_balance={bh_balance}, energy_owner_balance={owner_balance_from_energy}"
                )

        if sampled_balance_sum > snapshot_total_balance:
            raise WorldSimError(
                "global cross-check sampled balance exceeds snapshot total: "
                f"tick={tick}, sampled_balance_sum={sampled_balance_sum}, snapshot_total={snapshot_total_balance}"
            )
        if len(sampled_owners) == len(owners) and sampled_balance_sum != snapshot_total_balance:
            raise WorldSimError(
                "global cross-check full sampled balance mismatch with snapshot total: "
                f"tick={tick}, sampled_balance_sum={sampled_balance_sum}, snapshot_total={snapshot_total_balance}"
            )

        elapsed_ms = int((time.time() - start_ts) * 1000)
        result = {
            "tick": tick,
            "block_height": block_height,
            "top_n": top_n,
            "leaderboard_compared_count": compared_top_items,
            "active_owner_count": len(owners),
            "sampled_owner_count": len(sampled_owners),
            "sampled_balance_sum": sampled_balance_sum,
            "snapshot_total_balance": snapshot_total_balance,
            "elapsed_ms": elapsed_ms,
        }
        result.update(economic_info)
        return result

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
        latest_aggregate = self.rpc_usdb(
            "get_miner_economic_aggregate",
            [
                {
                    "view_version": self.ECONOMIC_VIEW_VERSION,
                    "block_height": None,
                    "context": None,
                }
            ],
        )
        leaderboard_top = self.rpc_usdb(
            "get_pass_energy_leaderboard",
            [{"at_height": block_height, "page": 0, "page_size": 1}],
        )

        top_item = None
        if isinstance(leaderboard_top, dict):
            items = leaderboard_top.get("items") or []
            if items:
                top_item = items[0]

        exact_aggregate = self.rpc_call(
            self.args.usdb_indexer_rpc_url,
            "get_miner_economic_aggregate",
            [
                {
                    "view_version": self.ECONOMIC_VIEW_VERSION,
                    "block_height": block_height,
                    "context": None,
                }
            ],
            retries=1,
            sleep_sec=0.1,
        )

        return {
            "sync_status": sync_status,
            "pass_stats": pass_stats,
            "latest_miner_aggregate": latest_aggregate,
            "top_item": top_item,
            "miner_aggregate_exact": exact_aggregate.get("result"),
            "miner_aggregate_error": exact_aggregate.get("error"),
        }

    def format_top_energy(self, top_item: dict[str, Any] | None) -> str:
        if not top_item:
            return "-"
        inscription_id = str(top_item.get("inscription_id", "-"))
        energy = top_item.get("energy", "-")
        return f"{inscription_id[:12]}..:{energy}"

    def run_economic_bootstrap_mint(
        self,
        *,
        step: int,
        actor_id: int,
        action: str,
        preferred_leader_agent_id: int | None = None,
        preferred_prev_inscription_id: str | None = None,
    ) -> tuple[str, int, dict[str, Any]]:
        actor = self.agents[actor_id]
        pre_height = int(self.run_btc_cli(None, ["getblockcount"]))
        action_id = f"economic-bootstrap-{step:02d}-{action}-agent-{actor_id}"
        detail, expectation, _ = self.op_mint(
            actor=actor,
            action_id=action_id,
            action=action,
            available_agent_ids=set(self.get_active_agent_ids()),
            pre_height=pre_height,
            rng=self.derived_rng("economic-bootstrap", step, actor_id, action),
            preferred_leader_agent_id=preferred_leader_agent_id,
            preferred_prev_inscription_id=preferred_prev_inscription_id,
        )
        block_height = self.mine_one_block()
        self.wait_service_synced(block_height)
        self.verify_expectation(expectation, block_height)
        self.metrics["verify_ok"] += 1
        actor.last_action = action
        actor.cooldown = 0
        for active_agent_id in self.get_active_agent_ids():
            self.refresh_agent_state(self.agents[active_agent_id], block_height)
        cross_check = self.run_global_cross_check(block_height, -step)
        self.metrics["global_cross_check_ok"] += 1
        self.clear_external_action_result(action_id)
        self.emit_report(
            "economic_bootstrap_step",
            {
                "step": step,
                "action": action,
                "actor_id": actor_id,
                "pass_id": expectation.inscription_id,
                "prev_pass_id": expectation.prev_inscription_id,
                "leader_ref_kind": expectation.leader_ref_kind,
                "leader_ref_value": expectation.leader_ref_value,
                "block_height": block_height,
                "detail": detail,
                "global_cross_check_info": cross_check,
            },
        )
        if expectation.inscription_id is None:
            raise WorldSimError(f"economic bootstrap action produced no pass id: {action}")
        return expectation.inscription_id, block_height, cross_check

    def run_economic_bootstrap(self) -> None:
        self.log("Starting deterministic UIP0001-UIP0006 economic bootstrap")

        leader_v1, _, _ = self.run_economic_bootstrap_mint(
            step=1, actor_id=0, action="standard_mint"
        )
        fixed_v1, _, _ = self.run_economic_bootstrap_mint(
            step=2,
            actor_id=1,
            action="fixed_collab_mint",
            preferred_leader_agent_id=0,
        )
        address_v1, _, _ = self.run_economic_bootstrap_mint(
            step=3,
            actor_id=2,
            action="address_collab_mint",
            preferred_leader_agent_id=0,
        )
        candidate_peer, _, _ = self.run_economic_bootstrap_mint(
            step=4, actor_id=3, action="standard_mint"
        )

        growth_height = self.mine_one_block()
        self.wait_service_synced(growth_height)
        for active_agent_id in self.get_active_agent_ids():
            self.refresh_agent_state(self.agents[active_agent_id], growth_height)
        growth_cross_check = self.run_global_cross_check(growth_height, -5)
        self.metrics["global_cross_check_ok"] += 1
        self.emit_report(
            "economic_bootstrap_step",
            {
                "step": 5,
                "action": "energy_growth",
                "block_height": growth_height,
                "global_cross_check_info": growth_cross_check,
            },
        )

        leader_v2, remint_height, _ = self.run_economic_bootstrap_mint(
            step=6,
            actor_id=0,
            action="standard_remint",
            preferred_prev_inscription_id=leader_v1,
        )
        state_ref = self.get_state_ref_at_height(remint_height)
        if state_ref is None:
            raise WorldSimError(
                f"economic bootstrap missing state ref at height={remint_height}"
            )
        context = self.build_consensus_context_from_state_ref(state_ref)
        old_leader_profile = self.load_pass_economic_profile_at_height(
            leader_v1, remint_height, context
        )
        self.assert_zero_economic_profile(
            old_leader_profile,
            inscription_id=leader_v1,
            expected_state="consumed",
        )
        old_leader_breakdown = self.load_collab_breakdown_at_height(
            leader_v1,
            remint_height,
            context,
            sort="collab_pass_id_asc",
        )
        if old_leader_breakdown["items"]:
            raise WorldSimError(
                "consumed fixed Leader retained collab contributions after remint: "
                f"leader={leader_v1}, items={old_leader_breakdown['items']}"
            )
        new_leader_breakdown = self.load_collab_breakdown_at_height(
            leader_v2,
            remint_height,
            context,
            sort="collab_pass_id_asc",
        )
        followed_ids = {
            str(item["collab_pass_id"]) for item in new_leader_breakdown["items"]
        }
        if followed_ids != {address_v1}:
            raise WorldSimError(
                "Leader remint follow semantics mismatch: "
                f"expected_address_only={[address_v1]}, got={sorted(followed_ids)}, "
                f"fixed_pass={fixed_v1}"
            )

        fixed_v2, _, _ = self.run_economic_bootstrap_mint(
            step=7,
            actor_id=1,
            action="fixed_collab_remint",
            preferred_leader_agent_id=0,
            preferred_prev_inscription_id=fixed_v1,
        )
        address_v2, final_height, _ = self.run_economic_bootstrap_mint(
            step=8,
            actor_id=2,
            action="address_collab_remint",
            preferred_leader_agent_id=0,
            preferred_prev_inscription_id=address_v1,
        )
        final_state_ref = self.get_state_ref_at_height(final_height)
        if final_state_ref is None:
            raise WorldSimError(
                f"economic bootstrap missing final state ref at height={final_height}"
            )
        final_context = self.build_consensus_context_from_state_ref(final_state_ref)
        final_breakdown = self.load_collab_breakdown_at_height(
            leader_v2,
            final_height,
            final_context,
            sort="collab_pass_id_asc",
        )
        final_collab_ids = {
            str(item["collab_pass_id"]) for item in final_breakdown["items"]
        }
        if final_collab_ids != {fixed_v2, address_v2}:
            raise WorldSimError(
                "economic bootstrap final collab set mismatch: "
                f"expected={sorted([fixed_v2, address_v2])}, got={sorted(final_collab_ids)}"
            )

        self.emit_report(
            "economic_bootstrap_complete",
            {
                "block_height": final_height,
                "leader_v1": leader_v1,
                "leader_v2": leader_v2,
                "candidate_peer": candidate_peer,
                "fixed_collab_v1": fixed_v1,
                "fixed_collab_v2": fixed_v2,
                "address_collab_v1": address_v1,
                "address_collab_v2": address_v2,
                "leader_remint_followed_collabs": sorted(followed_ids),
                "final_collab_ids": sorted(final_collab_ids),
            },
        )
        self.log(
            "Deterministic economic bootstrap completed: "
            f"height={final_height}, leader={leader_v2}, collabs={sorted(final_collab_ids)}"
        )

    def run(self) -> None:
        self.log(
            "World simulation started: "
            f"seed={self.args.seed}, blocks={self.args.blocks}, total_agents={self.total_agents}, "
            f"initial_active_agents={self.active_agent_count}, policy_mode={self.args.policy_mode}, "
            f"scripted_cycle={self.args.scripted_cycle}, "
            f"agent_self_check_enabled={self.args.agent_self_check_enabled}, "
            f"agent_self_check_interval_blocks={self.args.agent_self_check_interval_blocks}, "
            f"agent_self_check_sample_size={self.args.agent_self_check_sample_size}, "
            f"global_cross_check_enabled={self.args.global_cross_check_enabled}, "
            f"global_cross_check_interval_blocks={self.args.global_cross_check_interval_blocks}, "
            f"global_cross_check_leaderboard_top_n={self.args.global_cross_check_leaderboard_top_n}, "
            f"global_cross_check_owner_sample_size={self.args.global_cross_check_owner_sample_size}, "
            f"economic_page_limit={self.args.economic_page_limit}, "
            f"economic_bootstrap_enabled={self.args.economic_bootstrap_enabled}, "
            f"validator_sample_enabled={self.args.validator_sample_enabled}, "
            f"validator_sample_mode={self.args.validator_sample_mode}, "
            f"validator_sample_tamper_enabled={self.args.validator_sample_tamper_enabled}, "
            f"validator_sample_interval_blocks={self.args.validator_sample_interval_blocks}, "
            f"validator_sample_size={self.args.validator_sample_size}, "
            f"validator_sample_min_head_advance={self.args.validator_sample_min_head_advance}, "
            f"reorg_interval_blocks={self.args.reorg_interval_blocks}, "
            f"reorg_depth={self.args.reorg_depth}, "
            f"reorg_max_events={self.args.reorg_max_events}"
        )
        if self.report_path is not None:
            self.log(f"Structured tick report enabled: path={self.report_path}")
        if self.recovery_state_path is not None:
            self.log(f"Recovery state enabled: path={self.recovery_state_path}")

        tick = 0
        resume_tick_state: dict[str, Any] | None = None
        if self.resume_state is not None:
            resume_status = str(self.resume_state.get("status", ""))
            self.apply_recovery_snapshot(self.resume_state)
            if resume_status == "between_ticks":
                tick = max(0, int(self.resume_state.get("next_tick", 1)) - 1)
                self.log(
                    "Resuming world-sim between ticks: "
                    f"next_tick={tick + 1}, batch_seed={self.resume_state.get('batch_seed')}"
                )
                self.emit_report(
                    "recovery_resume",
                    {
                        "status": "between_ticks",
                        "next_tick": tick + 1,
                        "batch_seed": self.resume_state.get("batch_seed"),
                    },
                )
            elif resume_status == "tick_in_progress":
                resume_tick_state = self.resume_state
                tick = max(0, int(self.resume_state.get("tick", 1)) - 1)
                self.log(
                    "Resuming world-sim inside tick: "
                    f"tick={self.resume_state.get('tick')}, "
                    f"next_slot_index={self.resume_state.get('next_slot_index')}, "
                    f"batch_seed={self.resume_state.get('batch_seed')}"
                )
                self.emit_report(
                    "recovery_resume",
                    {
                        "status": "tick_in_progress",
                        "tick": self.resume_state.get("tick"),
                        "next_slot_index": self.resume_state.get("next_slot_index"),
                        "batch_seed": self.resume_state.get("batch_seed"),
                    },
                )
            else:
                raise WorldSimError(
                    f"unsupported recovery state status: {resume_status}"
                )

        if self.args.economic_bootstrap_enabled:
            if self.resume_state is None:
                self.run_economic_bootstrap()
            else:
                self.log("Skipping economic bootstrap while resuming recovery state")

        while True:
            if self.args.blocks > 0 and tick >= self.args.blocks:
                break

            if resume_tick_state is not None:
                tick = int(resume_tick_state.get("tick", tick + 1))
                pre_height = int(resume_tick_state.get("pre_height", 0))
                active_agent_ids = self.get_active_agent_ids()
                available_ids = {
                    int(agent_id)
                    for agent_id in (resume_tick_state.get("available_ids") or [])
                }
                action_slots = int(resume_tick_state.get("action_slots", 0))
                action_results = list(resume_tick_state.get("action_results") or [])
                action_trace_samples = list(
                    resume_tick_state.get("action_trace_samples") or []
                )
                tick_action_type_counts = {
                    action: int(
                        (resume_tick_state.get("tick_action_type_counts") or {}).get(
                            action, 0
                        )
                    )
                    for action in sorted(self.SUPPORTED_ACTIONS)
                }
                expectations = [
                    self.deserialize_expectation(payload)
                    for payload in (resume_tick_state.get("expectations") or [])
                ]
                action_failed = int(resume_tick_state.get("action_failed", 0))
                action_fail_samples = list(
                    resume_tick_state.get("action_fail_samples") or []
                )
                current_slot_plan_payload = resume_tick_state.get("current_slot_plan")
                current_slot_plan = (
                    self.deserialize_planned_action(current_slot_plan_payload)
                    if isinstance(current_slot_plan_payload, dict)
                    else None
                )
                current_slot_receipt_payload = resume_tick_state.get("current_slot_receipt")
                current_slot_receipt = (
                    self.deserialize_action_receipt(current_slot_receipt_payload)
                    if isinstance(current_slot_receipt_payload, dict)
                    else None
                )
                start_slot_index = int(resume_tick_state.get("next_slot_index", 0))
                batch_seed = int(resume_tick_state.get("batch_seed", self.action_seed))
                if (
                    current_slot_plan is not None
                    and current_slot_plan.slot_index != start_slot_index
                ):
                    raise WorldSimError(
                        "recovery snapshot slot mismatch: "
                        f"next_slot_index={start_slot_index}, "
                        f"current_slot_plan.slot_index={current_slot_plan.slot_index}"
                    )
                resume_tick_state = None
            else:
                tick += 1
                self.maybe_grow_agents(tick)
                pre_height = int(self.run_btc_cli(None, ["getblockcount"]))

                active_agent_ids = self.get_active_agent_ids()
                available_ids = set(active_agent_ids)
                max_slots = min(self.args.max_actions_per_block, len(available_ids))
                action_slots = self.action_position_rng(
                    tick, -1, "slot-count", pre_height, len(available_ids)
                ).randint(0, max(0, max_slots))

                action_results = []
                action_trace_samples = []
                tick_action_type_counts = {
                    action: 0 for action in sorted(self.SUPPORTED_ACTIONS)
                }
                expectations = []
                action_failed = 0
                action_fail_samples = []
                current_slot_plan = None
                current_slot_receipt = None
                start_slot_index = 0
                batch_seed = self.action_seed
                self.write_recovery_state(
                    self.build_recovery_snapshot(
                        status="tick_in_progress",
                        batch_seed=batch_seed,
                        tick=tick,
                        next_slot_index=start_slot_index,
                        action_slots=action_slots,
                        pre_height=pre_height,
                        active_agent_count=self.active_agent_count,
                        available_ids=available_ids,
                        action_results=action_results,
                        action_trace_samples=action_trace_samples,
                        tick_action_type_counts=tick_action_type_counts,
                        current_slot_plan=current_slot_plan,
                        current_slot_receipt=current_slot_receipt,
                        expectations=expectations,
                        action_failed=action_failed,
                        action_fail_samples=action_fail_samples,
                    )
                )

            verify_failed = 0
            verify_fail_samples: list[str] = []
            self_check_failed = 0
            self_check_fail_samples: list[str] = []
            self_checked_count = 0
            global_cross_checked = 0
            global_cross_check_failed = 0
            global_cross_check_fail_samples: list[str] = []
            global_cross_check_info: dict[str, Any] | None = None
            validator_sample_captured = 0
            validator_sample_capture_ids: list[str] = []
            validator_sample_checked = 0
            validator_sample_failed = 0
            validator_sample_fail_samples: list[str] = []
            reorg_applied = 0
            reorg_info: dict[str, Any] | None = None
            refresh_failed_agent_ids: set[int] = set()

            for slot_index in range(start_slot_index, action_slots):
                if not available_ids:
                    break

                replaying_recorded_receipt = False
                if current_slot_plan is not None:
                    if current_slot_plan.slot_index != slot_index:
                        raise WorldSimError(
                            "unexpected inflight slot replay mismatch: "
                            f"expected_slot={current_slot_plan.slot_index}, got_slot={slot_index}"
                        )
                    actor_id = current_slot_plan.actor_id
                    action = current_slot_plan.action
                    action_id = current_slot_plan.action_id
                    actor = self.agents[actor_id]
                    self.log(
                        "Replaying inflight world-sim slot: "
                        f"tick={tick}, slot_index={slot_index}, action_id={action_id}, "
                        f"actor={actor.wallet_name}, action={action}"
                    )
                    if current_slot_receipt is not None:
                        replaying_recorded_receipt = True
                        self.log(
                            "Replaying recorded world-sim slot receipt: "
                            f"tick={tick}, slot_index={slot_index}, action_id={action_id}"
                        )
                    self.emit_report(
                        "recovery_replay_slot",
                        {
                            "tick": tick,
                            "slot_index": slot_index,
                            "action_id": action_id,
                            "actor_id": actor_id,
                            "actor_wallet": actor.wallet_name,
                            "action": action,
                        },
                    )
                    if not replaying_recorded_receipt:
                        external_result = self.load_external_action_result(action_id)
                        if external_result is not None:
                            tick_action_type_counts[action] = (
                                tick_action_type_counts.get(action, 0) + 1
                            )
                            current_slot_receipt = self.rebuild_receipt_from_external_result(
                                plan=current_slot_plan,
                                payload=external_result,
                                pre_height=pre_height,
                                available_ids=available_ids,
                                tick=tick,
                                slot_index=slot_index,
                            )
                            replaying_recorded_receipt = True
                            self.log(
                                "Recovered world-sim slot from external result: "
                                f"tick={tick}, slot_index={slot_index}, action_id={action_id}"
                            )
                            self.emit_report(
                                "recovery_external_result_replay",
                                {
                                    "tick": tick,
                                    "slot_index": slot_index,
                                    "action_id": action_id,
                                    "actor_id": actor_id,
                                    "action": action,
                                },
                            )
                        else:
                            external_result = self.wait_for_inflight_external_action_result(
                                plan=current_slot_plan,
                                pre_height=pre_height,
                                available_ids=available_ids,
                                tick=tick,
                                slot_index=slot_index,
                            )
                            if external_result is None:
                                # The previous process died after persisting the inflight slot
                                # but before recording any external result or observable chain
                                # delta. Treat that slot as failed so recovery can make progress
                                # instead of crash-looping forever on the same stale action.
                                action_failed += 1
                                self.on_action_failed(action)
                                failure = (
                                    "action_id="
                                    f"{action_id},actor={actor.wallet_name},action={action},"
                                    "error=unresolved_inflight_slot_after_probe_window"
                                )
                                action_fail_samples.append(failure)
                                action_trace_samples.append(
                                    {
                                        "action_id": action_id,
                                        "slot_index": slot_index,
                                        "actor_id": actor_id,
                                        "actor_wallet": actor.wallet_name,
                                        "action": action,
                                        "status": "failed",
                                        "error": "unresolved_inflight_slot_after_probe_window",
                                    }
                                )
                                self.log(
                                    "WARN abandoning unresolved inflight world-sim slot after probe window: "
                                    f"tick={tick}, action_id={action_id}, actor={actor.wallet_name}, "
                                    f"action={action}"
                                )
                                self.emit_report(
                                    "recovery_unresolved_slot_abandoned",
                                    {
                                        "tick": tick,
                                        "slot_index": slot_index,
                                        "action_id": action_id,
                                        "actor_id": actor_id,
                                        "actor_wallet": actor.wallet_name,
                                        "action": action,
                                    },
                                )
                                available_ids.discard(actor_id)
                                actor.last_action = "failed"
                                actor.cooldown = 1
                                current_slot_plan = None
                                current_slot_receipt = None
                                self.clear_external_action_result(action_id)
                                self.write_recovery_state(
                                    self.build_recovery_snapshot(
                                        status="tick_in_progress",
                                        batch_seed=batch_seed,
                                        tick=tick,
                                        next_slot_index=slot_index + 1,
                                        action_slots=action_slots,
                                        pre_height=pre_height,
                                        active_agent_count=self.active_agent_count,
                                        available_ids=available_ids,
                                        action_results=action_results,
                                        action_trace_samples=action_trace_samples,
                                        tick_action_type_counts=tick_action_type_counts,
                                        current_slot_plan=current_slot_plan,
                                        current_slot_receipt=current_slot_receipt,
                                        expectations=expectations,
                                        action_failed=action_failed,
                                        action_fail_samples=action_fail_samples,
                                    )
                                )
                                continue
                            tick_action_type_counts[action] = (
                                tick_action_type_counts.get(action, 0) + 1
                            )
                            current_slot_receipt = self.rebuild_receipt_from_external_result(
                                plan=current_slot_plan,
                                payload=external_result,
                                pre_height=pre_height,
                                available_ids=available_ids,
                                tick=tick,
                                slot_index=slot_index,
                            )
                            replaying_recorded_receipt = True
                            self.log(
                                "Recovered world-sim slot from external probe: "
                                f"tick={tick}, slot_index={slot_index}, action_id={action_id}, "
                                f"source={external_result.get('source')}"
                            )
                            self.emit_report(
                                "recovery_external_probe_replay",
                                {
                                    "tick": tick,
                                    "slot_index": slot_index,
                                    "action_id": action_id,
                                    "actor_id": actor_id,
                                    "action": action,
                                    "source": external_result.get("source"),
                                },
                            )
                else:
                    actor_id = self.choose_actor(
                        available_ids,
                        self.action_position_rng(
                            tick,
                            slot_index,
                            "actor",
                            pre_height,
                            tuple(sorted(available_ids)),
                        ),
                    )
                    actor = self.agents[actor_id]
                    action = self.choose_action_for_agent(
                        actor,
                        available_ids,
                        pre_height,
                        self.action_position_rng(
                            tick,
                            slot_index,
                            "action",
                            actor_id,
                            actor.scripted_index,
                            actor.last_action,
                            actor.cooldown,
                            tuple(sorted(available_ids)),
                        ),
                    )
                    action_id = self.make_action_id(tick, slot_index, actor_id, action)
                    current_slot_plan = PlannedAction(
                        slot_index=slot_index,
                        actor_id=actor_id,
                        action=action,
                        action_id=action_id,
                        probe_state=self.build_action_probe_state(actor, action),
                    )
                    self.write_recovery_state(
                        self.build_recovery_snapshot(
                            status="tick_in_progress",
                            batch_seed=batch_seed,
                            tick=tick,
                            next_slot_index=slot_index,
                            action_slots=action_slots,
                            pre_height=pre_height,
                            active_agent_count=self.active_agent_count,
                            available_ids=available_ids,
                            action_results=action_results,
                            action_trace_samples=action_trace_samples,
                            tick_action_type_counts=tick_action_type_counts,
                            current_slot_plan=current_slot_plan,
                            current_slot_receipt=current_slot_receipt,
                            expectations=expectations,
                            action_failed=action_failed,
                            action_fail_samples=action_fail_samples,
                        )
                    )
                if not replaying_recorded_receipt:
                    tick_action_type_counts[action] = (
                        tick_action_type_counts.get(action, 0) + 1
                    )

                try:
                    if replaying_recorded_receipt:
                        if current_slot_receipt is None:
                            raise WorldSimError(
                                f"missing current_slot_receipt for action_id={action_id}"
                            )
                        detail, expectation, used_ids = self.apply_action_receipt(
                            current_slot_plan,
                            current_slot_receipt,
                        )
                    else:
                        metrics_before = dict(self.metrics)
                        detail, expectation, used_ids = self.execute_agent_action(
                            actor=actor,
                            action_id=action_id,
                            action=action,
                            available_agent_ids=available_ids,
                            pre_height=pre_height,
                            rng=self.action_position_rng(
                                tick,
                                slot_index,
                                "execute",
                                actor_id,
                                action,
                                pre_height,
                            ),
                        )
                        if current_slot_plan is None:
                            raise WorldSimError(
                                f"missing current_slot_plan for executed action_id={action_id}"
                            )
                        current_slot_receipt = self.build_action_receipt(
                            planned_action=current_slot_plan,
                            detail=detail,
                            expectation=expectation,
                            used_ids=used_ids,
                            metrics_before=metrics_before,
                        )
                        self.write_recovery_state(
                            self.build_recovery_snapshot(
                                status="tick_in_progress",
                                batch_seed=batch_seed,
                                tick=tick,
                                next_slot_index=slot_index,
                                action_slots=action_slots,
                                pre_height=pre_height,
                                active_agent_count=self.active_agent_count,
                                available_ids=available_ids,
                                action_results=action_results,
                                action_trace_samples=action_trace_samples,
                                tick_action_type_counts=tick_action_type_counts,
                                current_slot_plan=current_slot_plan,
                                current_slot_receipt=current_slot_receipt,
                                expectations=expectations,
                                action_failed=action_failed,
                                action_fail_samples=action_fail_samples,
                            )
                        )
                    action_results.append(f"{action_id}:{detail}")
                    if expectation is not None and expectation.action != "noop":
                        expectation.action_id = action_id
                        expectations.append(expectation)
                    action_trace_samples.append(
                        {
                            "action_id": action_id,
                            "slot_index": slot_index,
                            "actor_id": actor_id,
                            "actor_wallet": actor.wallet_name,
                            "action": action,
                            "result": detail,
                            "used_agent_ids": sorted(used_ids),
                            "status": "ok",
                        }
                    )
                    available_ids -= used_ids
                    actor.last_action = action
                    actor.cooldown = max(0, actor.cooldown - 1)
                    current_slot_plan = None
                    current_slot_receipt = None
                    self.clear_external_action_result(action_id)
                    self.write_recovery_state(
                        self.build_recovery_snapshot(
                            status="tick_in_progress",
                            batch_seed=batch_seed,
                            tick=tick,
                            next_slot_index=slot_index + 1,
                            action_slots=action_slots,
                            pre_height=pre_height,
                            active_agent_count=self.active_agent_count,
                            available_ids=available_ids,
                            action_results=action_results,
                            action_trace_samples=action_trace_samples,
                            tick_action_type_counts=tick_action_type_counts,
                            current_slot_plan=current_slot_plan,
                            current_slot_receipt=current_slot_receipt,
                            expectations=expectations,
                            action_failed=action_failed,
                            action_fail_samples=action_fail_samples,
                        )
                    )
                except Exception as e:  # noqa: BLE001
                    action_failed += 1
                    self.on_action_failed(action)
                    action_fail_samples.append(
                        f"action_id={action_id},actor={actor.wallet_name},action={action},error={e}"
                    )
                    action_trace_samples.append(
                        {
                            "action_id": action_id,
                            "slot_index": slot_index,
                            "actor_id": actor_id,
                            "actor_wallet": actor.wallet_name,
                            "action": action,
                            "status": "failed",
                            "error": str(e),
                        }
                    )
                    self.log(
                        "WARN action failed: "
                        f"tick={tick}, action_id={action_id}, actor={actor.wallet_name}, action={action}, error={e}"
                    )
                    available_ids.discard(actor_id)
                    actor.last_action = "failed"
                    actor.cooldown = 1
                    current_slot_plan = None
                    current_slot_receipt = None
                    self.clear_external_action_result(action_id)
                    self.write_recovery_state(
                        self.build_recovery_snapshot(
                            status="tick_in_progress",
                            batch_seed=batch_seed,
                            tick=tick,
                            next_slot_index=slot_index + 1,
                            action_slots=action_slots,
                            pre_height=pre_height,
                            active_agent_count=self.active_agent_count,
                            available_ids=available_ids,
                            action_results=action_results,
                            action_trace_samples=action_trace_samples,
                            tick_action_type_counts=tick_action_type_counts,
                            current_slot_plan=current_slot_plan,
                            current_slot_receipt=current_slot_receipt,
                            expectations=expectations,
                            action_failed=action_failed,
                            action_fail_samples=action_fail_samples,
                        )
                    )
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
                        f"action_id={expectation.action_id},action={expectation.action},actor_id={expectation.actor_id},error={e}"
                    )
                    self.log(
                        "WARN verification failed: "
                        f"tick={tick}, action_id={expectation.action_id}, action={expectation.action}, error={e}"
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

            if self.should_run_global_cross_check(tick):
                global_cross_checked = 1
                try:
                    global_cross_check_info = self.run_global_cross_check(
                        block_height, tick
                    )
                    self.metrics["global_cross_check_ok"] += 1
                except Exception as e:  # noqa: BLE001
                    self.metrics["global_cross_check_fail"] += 1
                    global_cross_check_failed = 1
                    global_cross_check_fail_samples.append(str(e))
                    self.log(
                        "WARN global cross-check failed: "
                        f"tick={tick}, block_height={block_height}, error={e}"
                    )
                    if self.args.fail_fast:
                        raise

            if self.should_trigger_reorg(tick, block_height):
                try:
                    reorg_info = self.perform_reorg(tick, block_height)
                    reorg_applied = 1
                except Exception:
                    self.metrics["reorg_fail"] += 1
                    raise

            if self.should_capture_validator_sample(tick):
                try:
                    validator_sample_capture_ids = self.capture_validator_samples(
                        block_height, tick
                    )
                    validator_sample_captured = len(validator_sample_capture_ids)
                except Exception as e:  # noqa: BLE001
                    validator_sample_failed = 1
                    self.metrics["validator_sample_fail"] += 1
                    validator_sample_fail_samples.append(f"capture_error={e}")
                    self.log(
                        "WARN validator sample capture failed: "
                        f"tick={tick}, block_height={block_height}, error={e}"
                    )
                    if self.args.fail_fast:
                        raise

            (
                validator_sample_checked,
                validator_sample_failed_runtime,
                validator_sample_fail_runtime_samples,
            ) = self.validate_pending_validator_samples(block_height, tick)
            validator_sample_failed += validator_sample_failed_runtime
            validator_sample_fail_samples.extend(validator_sample_fail_runtime_samples)

            summary = self.collect_summary(block_height)
            pass_stats = summary["pass_stats"] or {}
            latest_aggregate_view = summary["latest_miner_aggregate"] or {}
            latest_aggregate = latest_aggregate_view.get("miner_aggregate") or {}
            top_energy = self.format_top_energy(summary["top_item"])
            synced_height = (summary["sync_status"] or {}).get("synced_block_height")
            active_count = int(pass_stats.get("active_count", 0))
            total_count = int(pass_stats.get("total_count", 0))
            invalid_count = int(pass_stats.get("invalid_count", 0))
            total_balance = int(latest_aggregate.get("total_miner_btc_sats", 0))
            active_addresses = int(
                latest_aggregate.get("active_miner_owner_count", 0)
            )

            self.log(
                "tick_summary: "
                f"tick={tick}, block_height={block_height}, synced_height={synced_height}, "
                f"active_agent_count={self.active_agent_count}, actions={action_slots}, action_failed={action_failed}, "
                f"agent_self_checked={self_checked_count}, agent_self_check_failed={self_check_failed}, "
                f"global_cross_checked={global_cross_checked}, global_cross_check_failed={global_cross_check_failed}, "
                f"validator_sample_mode={self.args.validator_sample_mode}, "
                f"validator_sample_captured={validator_sample_captured}, validator_sample_checked={validator_sample_checked}, "
                f"validator_sample_failed={validator_sample_failed}, "
                f"reorg_applied={reorg_applied}, "
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
                    "global_cross_checked": global_cross_checked,
                    "global_cross_check_failed": global_cross_check_failed,
                    "validator_sample_mode": self.args.validator_sample_mode,
                    "validator_sample_captured": validator_sample_captured,
                    "validator_sample_capture_ids": validator_sample_capture_ids,
                    "validator_sample_checked": validator_sample_checked,
                    "validator_sample_failed": validator_sample_failed,
                    "reorg_applied": reorg_applied,
                    "known_passes": len(self.pass_owner_by_id),
                    "tick_action_type_counts": tick_action_type_counts,
                    "pass_total": total_count,
                    "pass_active": active_count,
                    "pass_invalid": invalid_count,
                    "active_addresses": active_addresses,
                    "active_total_balance": total_balance,
                    "top_energy": top_energy,
                    "action_results": action_results,
                    "action_trace_samples": action_trace_samples[:8],
                    "action_fail_samples": action_fail_samples[:8],
                    "verify_fail_samples": verify_fail_samples[:8],
                    "agent_self_check_fail_samples": self_check_fail_samples[:8],
                    "global_cross_check_info": global_cross_check_info,
                    "validator_sample_fail_samples": validator_sample_fail_samples[:8],
                    "reorg_info": reorg_info,
                    "global_cross_check_fail_samples": global_cross_check_fail_samples[
                        :8
                    ],
                },
            )

            exact_aggregate_view = summary["miner_aggregate_exact"]
            if isinstance(exact_aggregate_view, dict):
                exact_aggregate = (
                    exact_aggregate_view.get("miner_aggregate") or {}
                )
                exact_active = int(
                    exact_aggregate.get("active_miner_owner_count", 0)
                )
                if exact_active != active_count:
                    self.log(
                        "WARN invariant mismatch: "
                        f"block_height={block_height}, active_pass_count={active_count}, "
                        f"active_miner_owner_count={exact_active}"
                    )

            if self.args.sleep_ms_between_blocks > 0:
                time.sleep(self.args.sleep_ms_between_blocks / 1000.0)

            self.write_recovery_state(
                self.build_between_ticks_snapshot(
                    batch_seed=batch_seed,
                    next_tick=tick + 1,
                    current_height=block_height,
                )
            )

        self.log("World simulation completed.")
        self.log(f"final_metrics={json.dumps(self.metrics, sort_keys=True)}")
        self.emit_report("session_end", {"final_metrics": self.metrics})
        self.clear_recovery_state()


def parse_args() -> Args:
    parser = argparse.ArgumentParser(
        prog="regtest_world_simulator",
        description="Run continuous random protocol simulation on regtest",
    )
    parser.add_argument("--btc-cli", required=True)
    parser.add_argument("--bitcoin-dir", required=True)
    parser.add_argument("--btc-rpc-host", default="127.0.0.1")
    parser.add_argument("--btc-rpc-port", required=True, type=int)
    parser.add_argument(
        "--btc-auth-mode",
        default="cookie",
        choices=["cookie", "userpass"],
    )
    parser.add_argument("--btc-cookie-file")
    parser.add_argument("--btc-rpc-user")
    parser.add_argument("--btc-rpc-password")
    parser.add_argument("--ord-bin", required=True)
    parser.add_argument("--ord-data-dir", required=True)
    parser.add_argument("--ord-server-url", required=True)
    parser.add_argument("--miner-wallet", required=True)
    parser.add_argument("--mining-address", required=True)
    parser.add_argument("--agent-wallets", required=True)
    parser.add_argument("--agent-addresses", required=True)
    parser.add_argument("--identity-seed")
    parser.add_argument("--usdb-chain-miner-address")
    parser.add_argument("--usdb-chain-miner-agent-id", type=int, default=0)
    parser.add_argument("--balance-history-rpc-url", required=True)
    parser.add_argument("--usdb-indexer-rpc-url", required=True)
    parser.add_argument("--sync-timeout-sec", type=int, default=300)
    parser.add_argument("--blocks", type=int, default=200)
    parser.add_argument("--seed", type=int, default=42)
    parser.add_argument("--fee-rate", type=int, default=1)
    parser.add_argument("--max-actions-per-block", type=int, default=2)
    parser.add_argument("--standard-mint-probability", type=float, default=0.14)
    parser.add_argument("--fixed-collab-mint-probability", type=float, default=0.04)
    parser.add_argument("--address-collab-mint-probability", type=float, default=0.04)
    parser.add_argument("--invalid-mint-probability", type=float, default=0.02)
    parser.add_argument("--transfer-probability", type=float, default=0.18)
    parser.add_argument("--remint-probability", type=float, default=0.12)
    parser.add_argument("--send-probability", type=float, default=0.28)
    parser.add_argument("--spend-probability", type=float, default=0.14)
    parser.add_argument("--sleep-ms-between-blocks", type=int, default=0)
    parser.add_argument("--fail-fast", action="store_true")
    parser.add_argument("--temp-dir", required=True)
    parser.add_argument("--initial-active-agents", type=int, default=3)
    parser.add_argument("--agent-growth-interval-blocks", type=int, default=30)
    parser.add_argument("--agent-growth-step", type=int, default=1)
    parser.add_argument("--policy-mode", default="adaptive")
    parser.add_argument(
        "--scripted-cycle",
        default=(
            "standard_mint,fixed_collab_mint,address_collab_mint,"
            "send_balance,transfer,standard_remint,fixed_collab_remint,"
            "address_collab_remint,spend_balance,noop"
        ),
    )
    parser.add_argument("--report-file")
    parser.add_argument("--report-flush-every", type=int, default=1)
    parser.add_argument("--recovery-state-file")
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
    parser.add_argument(
        "--disable-global-cross-check",
        action="store_true",
        help="Disable low-frequency global cross-check diagnostics",
    )
    parser.add_argument(
        "--global-cross-check-interval-blocks",
        type=int,
        default=20,
        help="Run global cross-check every N mined blocks",
    )
    parser.add_argument(
        "--global-cross-check-leaderboard-top-n",
        type=int,
        default=20,
        help="Cross-check this many top leaderboard rows each run",
    )
    parser.add_argument(
        "--global-cross-check-owner-sample-size",
        type=int,
        default=16,
        help="How many active owners to sample per global cross-check (0 means all active owners)",
    )
    parser.add_argument(
        "--economic-page-limit",
        type=int,
        default=16,
        help="Page size used while replaying cursor-based UIP0006 economic views",
    )
    parser.add_argument(
        "--enable-economic-bootstrap",
        action="store_true",
        help="Build a deterministic standard/collab/remint scenario before random ticks",
    )
    parser.add_argument(
        "--enable-validator-sample",
        action="store_true",
        help="Enable low-frequency validator-style historical payload sampling and delayed validation",
    )
    parser.add_argument(
        "--enable-validator-sample-tamper-check",
        action="store_true",
        help="For candidate_set samples, also run a negative winner-tamper check after successful replay",
    )
    parser.add_argument(
        "--validator-sample-interval-blocks",
        type=int,
        default=0,
        help="Capture validator samples every N mined blocks (0 disables sampling)",
    )
    parser.add_argument(
        "--validator-sample-mode",
        default="single",
        choices=["single", "candidate_set"],
        help="Capture individual pass samples or one multi-pass candidate-set sample per interval",
    )
    parser.add_argument(
        "--validator-sample-size",
        type=int,
        default=1,
        help="How many active passes to sample per validator capture interval (0 means all active passes)",
    )
    parser.add_argument(
        "--validator-sample-min-head-advance",
        type=int,
        default=2,
        help="How many new head blocks must pass before validating a captured historical sample",
    )
    parser.add_argument(
        "--reorg-interval-blocks",
        type=int,
        default=0,
        help="Inject a deterministic reorg every N mined ticks (0 disables reorg injection)",
    )
    parser.add_argument(
        "--reorg-depth",
        type=int,
        default=3,
        help="How many latest canonical blocks to replace when reorg injection triggers",
    )
    parser.add_argument(
        "--reorg-max-events",
        type=int,
        default=1,
        help="Maximum number of injected reorg events during one simulation run (0 means unlimited)",
    )
    parsed = parser.parse_args()

    agent_wallets = [v for v in parsed.agent_wallets.split(",") if v]
    agent_addresses = [v for v in parsed.agent_addresses.split(",") if v]
    scripted_cycle = [v.strip() for v in parsed.scripted_cycle.split(",") if v.strip()]

    return Args(
        btc_cli=parsed.btc_cli,
        bitcoin_dir=parsed.bitcoin_dir,
        btc_rpc_host=parsed.btc_rpc_host,
        btc_rpc_port=parsed.btc_rpc_port,
        btc_auth_mode=parsed.btc_auth_mode,
        btc_cookie_file=parsed.btc_cookie_file,
        btc_rpc_user=parsed.btc_rpc_user,
        btc_rpc_password=parsed.btc_rpc_password,
        ord_bin=parsed.ord_bin,
        ord_data_dir=parsed.ord_data_dir,
        ord_server_url=parsed.ord_server_url,
        miner_wallet=parsed.miner_wallet,
        mining_address=parsed.mining_address,
        agent_wallets=agent_wallets,
        agent_addresses=agent_addresses,
        identity_seed=parsed.identity_seed,
        usdb_chain_miner_address=parsed.usdb_chain_miner_address,
        usdb_chain_miner_agent_id=parsed.usdb_chain_miner_agent_id,
        balance_history_rpc_url=parsed.balance_history_rpc_url,
        usdb_indexer_rpc_url=parsed.usdb_indexer_rpc_url,
        sync_timeout_sec=parsed.sync_timeout_sec,
        blocks=parsed.blocks,
        seed=parsed.seed,
        fee_rate=parsed.fee_rate,
        max_actions_per_block=parsed.max_actions_per_block,
        standard_mint_probability=parsed.standard_mint_probability,
        fixed_collab_mint_probability=parsed.fixed_collab_mint_probability,
        address_collab_mint_probability=parsed.address_collab_mint_probability,
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
        recovery_state_file=parsed.recovery_state_file,
        agent_self_check_enabled=(not parsed.disable_agent_self_check),
        agent_self_check_interval_blocks=parsed.agent_self_check_interval_blocks,
        agent_self_check_sample_size=parsed.agent_self_check_sample_size,
        global_cross_check_enabled=(not parsed.disable_global_cross_check),
        global_cross_check_interval_blocks=parsed.global_cross_check_interval_blocks,
        global_cross_check_leaderboard_top_n=parsed.global_cross_check_leaderboard_top_n,
        global_cross_check_owner_sample_size=parsed.global_cross_check_owner_sample_size,
        economic_page_limit=parsed.economic_page_limit,
        economic_bootstrap_enabled=parsed.enable_economic_bootstrap,
        validator_sample_enabled=parsed.enable_validator_sample,
        validator_sample_mode=parsed.validator_sample_mode,
        validator_sample_tamper_enabled=parsed.enable_validator_sample_tamper_check,
        validator_sample_interval_blocks=parsed.validator_sample_interval_blocks,
        validator_sample_size=parsed.validator_sample_size,
        validator_sample_min_head_advance=parsed.validator_sample_min_head_advance,
        reorg_interval_blocks=parsed.reorg_interval_blocks,
        reorg_depth=parsed.reorg_depth,
        reorg_max_events=parsed.reorg_max_events,
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
