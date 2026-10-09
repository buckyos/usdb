#!/usr/bin/env python3
"""Exercise the real binary as Docker PID 1, with no node data or external network."""

import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]


def docker(*args, timeout=15):
    """Run bounded Docker operations on only this test's uniquely named container."""
    return subprocess.run(["docker", *args], check=True, capture_output=True,
                          text=True, timeout=timeout).stdout.strip()


def check_shutdown(binary, image, signal):
    """Require a zero exit and flushed shutdown logs; SIGKILL is test cleanup only."""
    name = "usdb-shutdown-test-" + uuid.uuid4().hex
    with tempfile.TemporaryDirectory(prefix="usdb-control-plane-shutdown-") as temporary:
        root = Path(temporary)
        (root / "web").mkdir()
        (root / "config.toml").write_text('''root_dir = "/fixture"
[server]
host = "127.0.0.1"
port = 28040
[bitcoin]
url = "http://127.0.0.1:1"
[rpc]
balance_history_url = "http://127.0.0.1:1"
usdb_indexer_url = "http://127.0.0.1:1"
usdb_chain_url = "http://127.0.0.1:1"
ord_url = "http://127.0.0.1:1"
[web]
console_root = "/fixture/web"
balance_history_explorer_root = "/fixture/web"
usdb_indexer_explorer_root = "/fixture/web"
''', encoding="utf-8")
        created = False
        try:
            docker("create", "--name", name, "--network", "none", "--read-only",
                   "--cap-drop", "ALL", "--security-opt", "no-new-privileges:true",
                   "--user", f"{os.getuid()}:{os.getgid()}",
                   "--mount", f"type=bind,src={binary},dst=/usdb-control-plane,readonly",
                   "--mount", f"type=bind,src={root},dst=/fixture",
                   # Minimal Ubuntu lacks the CA bundle required by reqwest's TLS client.
                   "--mount", "type=bind,src=/etc/ssl/certs,dst=/etc/ssl/certs,readonly",
                   "--entrypoint", "/usdb-control-plane", image,
                   "--root-dir", "/fixture", "--skip-process-lock")
            created = True
            docker("start", name)
            # No init wrapper: the binary itself must handle the Docker stop signal.
            assert docker("exec", name, "cat", "/proc/1/comm") == "usdb-control-pl"
            deadline = time.monotonic() + 20
            while True:
                state = json.loads(docker("inspect", "--format", "{{json .State}}", name))
                assert state["Running"], f"startup failed: {docker('logs', name)}"
                try:
                    response = docker("exec", name, "bash", "-c",
                                      "exec 3<>/dev/tcp/127.0.0.1/28040; "
                                      "printf 'GET /healthz HTTP/1.1\\r\\nHost: localhost\\r\\nConnection: close\\r\\n\\r\\n' >&3; "
                                      "head -n 1 <&3", timeout=3)
                    if "200 OK" in response:
                        break
                except subprocess.CalledProcessError:
                    pass
                assert time.monotonic() < deadline, "healthz never became ready"
                time.sleep(0.1)
            started = time.monotonic()
            docker("kill", f"--signal={signal}", name)
            assert docker("wait", name) == "0", "shutdown must exit successfully"
            logs = "\n".join(path.read_text(encoding="utf-8") for path in (root / "logs").rglob("*.log"))
            for message in (f"shutdown started: signal={signal}", "shutdown finished:"):
                assert message in logs, f"missing flushed log: {message}"
            print(f"PASS control-plane PID 1 {signal}: graceful exit in {time.monotonic() - started:.2f}s")
        except Exception:
            if created:
                print(docker("inspect", "--format", "{{json .State}}", name))
                # Fixture logs contain no real RPC credentials or node data.
                result = subprocess.run(["docker", "logs", name], capture_output=True, text=True, timeout=10)
                print(result.stdout + result.stderr)
                for path in (root / "logs").rglob("*.log"):
                    print(path.read_text(encoding="utf-8"))
            raise
        finally:
            if created:
                docker("rm", "--force", name)


def main():
    """Run both Docker stop signals against a freshly built local binary."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "src/btc/target/debug/usdb-control-plane")
    parser.add_argument("--image", default="ubuntu:24.04", help="Already available glibc runtime image with bash")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    for signal in ("SIGTERM", "SIGINT"):
        check_shutdown(binary, args.image, signal)


if __name__ == "__main__":
    main()
