"""Synthetic proc/cgroup evidence and resource samples; never read live services."""
from pathlib import Path

from common.node_monitor import BASE


def evidence(at=BASE, *, full=2, maximum=3, identity="container:start", window="bad", phase="overlap"):
    return dict(at_ms=at, session_id="session-a", release_id="r1", boot_id="boot-a",
                configuration={"phase": phase}, observation={"observed_at_ms": at - 1000, "observation_available": False},
                items=[dict(service="btc-node", identity=identity, pressure_window=window, status="available", missing=[],
                            metrics=dict(memory_current_bytes=1023, memory_limit_bytes=1024,
                                         memory_psi_full_avg10=full, events_max_delta=maximum, events_high_delta=0,
                                         swap_used_bytes=0))])


def resource_report(value, *, storage="available"):
    return dict(diagnostics=value, history=dict(state=storage, at_ms=value["at_ms"], last_sample_ms=value["at_ms"]))


def kernel_tree(root):
    proc, cgroup = Path(root) / "proc", Path(root) / "cgroup"
    files = {"proc/sys/kernel/random/boot_id": "boot-a\n", "proc/stat": "cpu 10 0 10 50 10 0 0 0\n",
             "proc/vmstat": "pswpin 1\npswpout 2\n", "proc/42/cgroup": "0::/docker/a\n",
             "cgroup/docker/a/memory.current": "1024\n", "cgroup/docker/a/memory.max": "2048\n",
             "cgroup/docker/a/memory.swap.current": "16\n", "cgroup/docker/a/memory.swap.max": "max\n",
             "cgroup/docker/a/memory.stat": "anon 128\nfile 896\ninactive_file 400\nslab 0\n",
             "cgroup/docker/a/memory.events": "high 0\nmax 5\noom 0\noom_kill 0\n"}
    psi = "some avg10=3.00 avg60=0.00 avg300=0.00 total=500\nfull avg10=2.00 avg60=0.00 avg300=0.00 total=200\n"
    files.update({"proc/pressure/" + kind: psi for kind in ("memory", "io", "cpu")})
    files.update({"cgroup/docker/a/" + kind + ".pressure": psi for kind in ("memory", "io")})
    for name, content in files.items():
        path = Path(root) / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)
    resources = dict(host=dict(status="available", memory_total_bytes=2048, memory_available_bytes=1024),
                     containers=dict(status="available", items=[dict(service="btc-node", container_id="a" * 12,
                         state="running", status="available", memory_used_bytes=624)]))
    inspect = "a" * 64 + "\t42\t0\t2048\t4096\t2026-09-25T00:00:00Z\n"
    return proc, cgroup, resources, inspect
