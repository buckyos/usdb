"""Real controller/monitor state machines with test-only external service adapters.

No Docker, systemd or live node is contacted. All state belongs to the parent fixture.
"""
import json
import os
from pathlib import Path
import sys
import time
from types import SimpleNamespace

REPO = Path(__file__).resolve().parents[2]
sys.path[:0] = [str(REPO / "docker/scripts/tools"), str(REPO / "tests")]
import usdb_node as node
import node_monitor as monitor
import resource_policy as policy
from common.process_recovery import write_json


def layout(root):
    return SimpleNamespace(node_env=root / "node.env", bundle_id="p1-test", release_id="p1-test-release",
                           kit_root=root, network_identity={"btc_index_origin_height": 100,
                                                           "chain_id": 12, "genesis_block_hash": "a" * 64})


def boundary(root, phase, selected):
    if phase != selected:
        return
    write_json(root / "reached.json", dict(pid=os.getpid(), phase=phase))
    while True:
        time.sleep(1)


def container(env, service):
    return {"state": "running", "exit_code": 0,
            "memory": int(env.get(policy.SERVICE_MEMORY_KEYS[service], 128 * 1024**2)),
            "swap": int(env["BTC_MEMORY_SWAP_LIMIT"] if service == "btc-node" else env["BH_MEMORY_SWAP_LIMIT"]),
            "image": env["USDB_BITCOIN_IMAGE"] if service == "btc-node" else env["USDB_SERVICES_IMAGE"],
            "environment": env}


def controller(root, target, pause):
    selected = layout(root)
    model_path = root / "containers.json"
    read_model = lambda: json.loads(model_path.read_text())
    node.effective_memory_bytes = lambda: 64 * policy.GIB
    node._resource_containers = lambda _: read_model()

    def helper(_layout, helper, arguments, **_kwargs):
        action = arguments[0]
        model = read_model()
        env = node.read_env(selected.node_env)
        # Model only the external service boundary; journals, locks and reconciliation are real.
        if action == "quiesce-data":
            for name, item in model.items():
                if name != "btc-node":
                    item["state"] = "exited"
        elif action == "down":
            model.pop("btc-node", None)
        elif action == "start":
            model["btc-node"] = container(env, "btc-node")
        else:
            raise AssertionError(f"Unexpected helper {helper} {arguments}")
        write_json(model_path, model)
        with (root / "actions.jsonl").open("a") as stream:
            stream.write(json.dumps(dict(action=action, phase=env["USDB_RESOURCE_PHASE"])) + "\n")
            stream.flush()
            os.fsync(stream.fileno())
        boundary(root, "after-" + action, pause)

    def status(_layout):
        return {"release_id": selected.release_id, "overall_state": "READY" if (root / "ready").exists() else "STARTING",
                "checks": {}}

    def start(_layout, **_kwargs):
        node._transition_resources(selected, target, output_to_stderr=False)
        boundary(root, "waiting-dependency", pause)
        state = node._read_resource_state(selected)
        for service in state.get("recover_services", []):
            model = read_model()
            model[service] = container(node.read_env(selected.node_env), service)
            write_json(model_path, model)
            node._resource_service_started(selected, service)
        (root / "ready").touch()

    node.run_helper = helper
    node.collect_node_status = status
    node.start_node = start
    node.print_up_result = lambda value, **_: print(json.dumps(value), flush=True)
    return node.run_bootstrap_controller(selected, sync_timeout_secs=30, pull=False)


def run_monitor(root):
    selected = layout(root)
    node.__file__ = __file__
    original = monitor.config
    monitor.config = lambda value: {**original(value), "interval_secs": 0.1}
    monitor.ResourceObserver = lambda *_: SimpleNamespace(snapshot=lambda: {"status": "unavailable"}, observe=lambda _: None)
    monitor.run(selected, node)


def download(root, pause):
    import bitcoin_assumeutxo as boot
    options = json.loads((root / "download.json").read_text())
    snapshot = boot.Snapshot(**options["snapshot"])
    # Pause after a transport failure with a saved partial file; kill the actual downloader process.
    original = boot.time.sleep
    def retry_sleep(seconds):
        boundary(root, "download-retry", pause)
        original(seconds)
    boot.time.sleep = retry_sleep
    boot.download_snapshot(snapshot, root / "snapshot.dat", options["url"], reserve_bytes=0)


def activate(root):
    import bitcoin_assumeutxo as boot
    options = json.loads((root / "activate.json").read_text())
    snapshot = boot.Snapshot(**options["snapshot"])
    rpc = boot.Rpc(options["rpc"], user="test", password="pass")
    result = boot.activate(snapshot, root / "snapshot.dat", rpc, root / "activation", poll_seconds=0.02,
                           wait_seconds=10, reserve_bytes=0)
    write_json(root / "activation-result.json", result)


if __name__ == "__main__":
    if "sample" in sys.argv:
        from common.node_monitor import report, incident
        print(json.dumps(report(monitor.now_ms(), incidents=incident())))
    else:
        mode, root = sys.argv[1], Path(sys.argv[2])
        if mode == "controller":
            raise SystemExit(controller(root, *sys.argv[3:]))
        if mode == "monitor":
            run_monitor(root)
        elif mode == "download":
            download(root, *sys.argv[3:])
        elif mode == "activate":
            activate(root)
        else:
            raise ValueError(f"Unsupported worker mode: {mode}")
