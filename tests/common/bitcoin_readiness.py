"""Safe native Bitcoin readiness reports for RPC and mining diagnostic tests."""

import bitcoin_assumeutxo as BOOT


READY = dict(schema_version=BOOT.SCHEMA, rpc_available=True, bootstrap_ready=True,
             tip_ready=True, history_validated=False, background_height=12)


def failed_rpc(kind="timeout", code=None):
    """Use the same safe metadata that the in-container status command emits."""
    error = BOOT.RpcFailure("getchainstates", code, kind=kind)
    return dict(schema_version=BOOT.SCHEMA, rpc_available=False, bootstrap_ready=False,
                tip_ready=False, error_kind="rpc_unavailable", error=str(error), rpc_failure=error.diagnostic())
