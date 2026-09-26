"""Safe native Bitcoin readiness reports for RPC and mining diagnostic tests."""

import bitcoin_assumeutxo as BOOT
import time


READY = dict(schema_version=BOOT.SCHEMA, rpc_available=True, bootstrap_ready=True,
             tip_ready=True, history_validated=False, background_height=12)


def failed_rpc(kind="timeout", code=None):
    """Use the same safe metadata that the in-container status command emits."""
    error = BOOT.RpcFailure("getchainstates", code, kind=kind)
    return dict(schema_version=BOOT.SCHEMA, rpc_available=False, bootstrap_ready=False,
                tip_ready=False, error_kind="rpc_unavailable", error=str(error), rpc_failure=error.diagnostic())


class ScheduledRpc:
    """Model slow RPCs against a monotonic clock without wall-clock sleeps or networking."""
    def __init__(self, steps):
        self.steps = list(steps)
        self.now = 0.0
        self.calls = []

    def monotonic(self):
        return self.now

    def sleep(self, seconds):
        self.now += seconds

    def time(self):
        return time.time()

    def call(self, method, params=None, *, timeout):
        self.calls.append((method, timeout))
        duration, result = self.steps.pop(0)
        self.now += min(duration, timeout)
        if duration >= timeout:
            raise BOOT.RpcFailure(method, kind="timeout")
        if isinstance(result, Exception):
            raise result
        return result
