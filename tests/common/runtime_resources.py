"""Version-two resource controller fixture using only temporary data and fake Docker."""

from contextlib import ExitStack
from pathlib import Path
from unittest import mock

import node_resource_monitor
import resource_policy as policy
import usdb_node as node
from common.native_node import NativeRuntime


class RuntimeResources(NativeRuntime):
    """Observe cache adoption and graceful targeted stops across interruptions."""
    def __init__(self, root: Path, *, memory=32 * policy.GIB, phase="catchup", ord_enabled=False,
                 runtime_budget=None, ord_policy="adaptive-v1"):
        super().__init__(root)
        self.memory = memory
        env = dict(node.read_env(self.layout.node_env), USDB_RESOURCE_POLICY_VERSION="2",
                   USDB_STORAGE_PROFILE="slow-disk", USDB_RESOURCE_MEMORY_PERCENT="90",
                   USDB_RESOURCE_PHASE=phase, USDB_MINTING_ENABLED=str(int(ord_enabled)),
                   USDB_ORD_RESOURCE_POLICY=ord_policy, USDB_DATA_ROOT=str(root / "data"))
        if runtime_budget is not None:
            env["USDB_RESOURCE_RUNTIME_BUDGET"] = runtime_budget
        env.update(policy.build_resource_plan(memory, phase, env).environment())
        self.layout.node_env.write_text(node.upsert_env("", env))
        self.core.update(rpc_available=True, tip_ready=True, history_validated=True,
                         indexes_ready=True, active_height=970000, headers=970000)
        self.ready = True
        for name in ("btc-node", "balance-history", "usdb-indexer", "usdb-chain", "usdb-control-plane"):
            self.containers[name] = self.container(name)
        if ord_enabled:
            self.containers["ord-server"] = self.container("ord-server")

    def container(self, service):
        result = super().container(service)
        result["started_at"] = "2026-10-15T00:00:00+00:00"
        if service == "ord-server":
            result["swap"] = result["memory"]
        return result

    def helper(self, layout, helper, args, **kwargs):
        if args[0] == "quiesce-services":
            self.events.append((args[0], tuple(args[1:])))
            for name in args[1:]:
                self.containers[name]["state"] = "exited"
            if self.crash == args[0]:
                self.crash = None
                raise RuntimeError("Interrupted after quiesce-services")
        elif args[0] == "up-console":
            self.containers["usdb-control-plane"] = self.container("usdb-control-plane")
        else:
            return super().helper(layout, helper, args, **kwargs)

    def readiness(self, layout, helper, args, service):
        value, detail = super().readiness(layout, helper, args, service)
        return (dict(value, current=969990, total=969990) if value else None), detail

    def patches(self):
        """Patch all operating-system mutations, leaving policy/journal logic real."""
        stack = ExitStack()
        for item in (mock.patch.object(node, "effective_memory_bytes", side_effect=lambda: self.memory),
                     mock.patch.object(node, "_resource_containers", side_effect=self.observed),
                     mock.patch.object(node, "run_helper", side_effect=self.helper),
                     mock.patch.object(node, "_read_service_readiness", side_effect=self.readiness),
                     mock.patch.object(node, "_chain_component", return_value=dict(state="READY")),
                     mock.patch.object(node_resource_monitor, "transition"),
                     mock.patch.object(node, "_start_optional_ord"),
                     mock.patch("node_upgrade.ensure_no_pending")):
            stack.enter_context(item)
        return stack
