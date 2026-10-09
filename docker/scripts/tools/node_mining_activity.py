"""Bounded, display-only mining observations; never inputs to mining authorization."""

from copy import deepcopy
from datetime import datetime, timezone
import json
import re
import subprocess
import time

HASH = re.compile(r"0x[0-9a-fA-F]{64}")
ADDRESS = re.compile(r"0x[0-9a-fA-F]{40}")
PASS = re.compile(r"[0-9a-f]{64}i(?:0|[1-9][0-9]{0,9})")
REFRESH_SECS = 15
PROBE_BUDGET_SECS = 6


def _quantity(value):
    """Accept only bounded RPC quantities, never floats or booleans."""
    if not isinstance(value, str) or not re.fullmatch(r"0x[0-9a-fA-F]{1,64}", value):
        raise ValueError("Invalid block quantity")
    return int(value, 16)


def _atoms(value):
    """Preserve uint256 monetary precision in the JSON observation contract."""
    if not isinstance(value, str) or not re.fullmatch(r"0|[1-9][0-9]{0,77}", value) or int(value) >= 2**256:
        raise ValueError("Invalid reward amount")
    return int(value)


def _reward(value, seal, address):
    """Bind verified income to the exact canonical block and its reward recipient."""
    if (not isinstance(value, dict) or value.get("schema_version") != "usdb-block-economics:v1"
            or value.get("status") != "verified" or value.get("block_hash") != seal["hash"]
            or value.get("block_number") != str(seal["height"])
            or str(value.get("miner", "")).lower() != address):
        raise ValueError("Unverified or mismatched block economics")
    amounts = value.get("amounts") or {}
    if not isinstance(amounts, dict):
        raise ValueError("Invalid reward amounts")
    emission, fees = (_atoms(amounts.get(key)) for key in ("miner_emission_atoms", "miner_fees_atoms"))
    if emission + fees >= 2**256:
        raise ValueError("Invalid total block income")
    selector = value.get("selector") or {}
    if not isinstance(selector, dict):
        raise ValueError("Invalid historical block selector")
    pass_id, height = selector.get("pass_id"), selector.get("btc_height")
    if (not isinstance(pass_id, str) or not PASS.fullmatch(pass_id)
            or type(height) is not int or not 0 <= height <= 0xFFFFFFFF):
        raise ValueError("Invalid historical block Pass")
    return dict(state="verified", emission_atoms=str(emission), fee_atoms=str(fees),
                total_atoms=str(emission + fees), pass_id=pass_id, btc_height=height)


def _reward_error(error):
    """Expose stable reasons without leaking RPC endpoints or raw response bodies."""
    message = str(error)
    for code in ("HISTORICAL_STATE_UNAVAILABLE", "ECONOMICS_POLICY_UNSUPPORTED", "ECONOMICS_BUSY",
                 "ECONOMICS_TIMEOUT", "ECONOMICS_REPLAY_LIMIT", "BLOCK_NOT_CANONICAL"):
        if code in message:
            return code
    if "-32601" in message or "not available" in message or "not found" in message.lower():
        return "economics RPC unavailable in this chain release"
    return "economics RPC unavailable or response not verified"


class MiningActivityObserver:
    """Keep one bounded in-memory observation, scoped to network, writer and miner.

    A changed head (including same-height reorgs) triggers canonical verification.
    Identical heads are sampled at most every 15s. Economics calls have a separate
    15s throttle, 60s failure backoff, and a one-block verified result cache.
    """

    def __init__(self):
        self.scope = None
        self.head = None
        self.next_probe = 0
        self.report = None
        self.seal_hash = None
        self.reward = None
        self.reward_failure = None
        self.next_reward = 0

    def observe(self, layout, mining, component, service):
        """Sample only an applied miner on a ready chain; failures stay display-only."""
        config, runtime = mining.get("configured", {}), mining.get("runtime", {})
        address = str(config.get("USDB_MINER_ADDRESS", "")).lower()
        head = component.get("head", {})
        if (config.get("USDB_NODE_ROLE") != "miner" or not ADDRESS.fullmatch(address)
                or mining.get("applied") is not True or mining.get("drift")
                or component.get("state") != "READY" or component.get("observation_unavailable")
                or not HASH.fullmatch(str(head.get("hash", "")))
                or type(head.get("number")) is not int or head["number"] < 0 or not runtime.get("id")):
            return None
        scope = (str(layout.node_env), layout.network_identity.get("chain_id"),
                 layout.network_identity.get("network_id"), layout.network_identity.get("genesis_block_hash"),
                 runtime["id"], service.get("started_at"), address)
        if self.scope != scope:
            self.__init__()
            self.scope = scope
        now = time.monotonic()
        if self.report is not None and self.head == head["hash"] and now < self.next_probe:
            return deepcopy(self.report)
        self.report = self._collect(layout, runtime, address, head)
        self.head, self.next_probe = head["hash"], time.monotonic() + REFRESH_SECS
        return deepcopy(self.report)

    def _collect(self, layout, runtime, address, head):
        """Use log evidence for local attribution and recheck canonicality after replay."""
        import usdb_node as node
        deadline = time.monotonic() + PROBE_BUDGET_SECS
        report = dict(state="unavailable", observed_at=datetime.now(timezone.utc).isoformat(),
                      observed_head_hash=head["hash"], observed_head_height=head["number"])

        def timeout():
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise TimeoutError("Mining display probe budget exhausted")
            return min(2, remaining)

        def rpc(method, params):
            return node._json_rpc_batch(url, ((method, params),), timeout_secs=timeout())[method]

        try:
            env = node.read_env(layout.node_env)
            url = node._host_rpc_url(env, "USDB_HTTP_BIND_ADDRESS", "USDB_HTTP_BIND_PORT", 8545)
            logs = subprocess.run(["docker", "logs", "--tail", "200", runtime["id"]],
                                  capture_output=True, text=True, check=True, timeout=timeout())
            block_hash = self.seal_hash
            for line in reversed((logs.stdout + logs.stderr).splitlines()):
                try:
                    entry = json.loads(line)
                except ValueError:
                    continue
                if (isinstance(entry, dict) and entry.get("msg") == "Successfully sealed new block"
                        and HASH.fullmatch(str(entry.get("hash", "")))):
                    block_hash = entry["hash"].lower()
                    break
            if block_hash is None:
                return dict(report, state="not_observed", detail="No local seal observed in recent container logs")
            self.seal_hash = block_hash
            block = rpc("eth_getBlockByHash", [block_hash, False])
            if (not isinstance(block, dict) or block.get("hash") != block_hash
                    or str(block.get("miner", "")).lower() != address):
                raise ValueError("Local seal block or recipient unavailable")
            seal = dict(hash=block_hash, height=_quantity(block.get("number")),
                        timestamp=_quantity(block.get("timestamp")))
            report["local_seal"] = seal
            if seal["height"] > head["number"]:
                return dict(report, state="pending", detail="Local seal is newer than the chain-head observation")
            canonical = rpc("eth_getBlockByNumber", [block["number"], False])
            if not isinstance(canonical, dict) or not HASH.fullmatch(str(canonical.get("hash", ""))):
                raise ValueError("Canonical block unavailable")
            if canonical["hash"] != block_hash:
                self.reward = None
                return dict(report, state="orphaned", detail="Latest observed local seal is no longer canonical")
            reward = self.reward_failure or dict(state="pending", detail="Waiting for the next reward refresh")
            if self.reward and self.reward[0] == block_hash:
                reward = self.reward[1]
            elif time.monotonic() >= self.next_reward:
                self.next_reward = time.monotonic() + REFRESH_SECS
                try:
                    reward = _reward(rpc("eth_getUSDBBlockEconomics", [block_hash]), seal, address)
                    self.reward = (block_hash, reward)
                    self.reward_failure = None
                except (OSError, ValueError, TypeError, KeyError) as error:
                    self.next_reward = time.monotonic() + 60
                    reward = dict(state="unavailable", detail=_reward_error(error))
                    self.reward_failure = reward
            canonical = rpc("eth_getBlockByNumber", [block["number"], False])
            if not isinstance(canonical, dict) or not HASH.fullmatch(str(canonical.get("hash", ""))):
                raise ValueError("Canonical block unavailable after economics query")
            if canonical["hash"] != block_hash:
                self.reward = None
                return dict(report, state="orphaned", detail="Local seal left the canonical chain during observation")
            return dict(report, state="canonical", reward=reward,
                        confirmations=head["number"] - seal["height"] + 1)
        except (OSError, ValueError, TypeError, KeyError, subprocess.SubprocessError):
            # Never turn an unavailable optional observation into a mining gate.
            return dict(report, detail="Local block observation unavailable; retrying on the next sample")


OBSERVER = MiningActivityObserver()
