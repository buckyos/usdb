"""A narrow Docker command recorder for testing the real native shell helpers."""

from pathlib import Path


def install_docker_recorder(directory: Path) -> Path:
    directory.mkdir()
    binary = directory / "docker"
    binary.write_text('''#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
with open(os.environ["NATIVE_DOCKER_CALLS"], "a") as output:
    output.write(json.dumps(args) + "\\n")
if args[0] == "compose":
    if "exec" in args:
        ready = os.environ.get("NATIVE_CORE_READY", "1") == "1"
        print(json.dumps(dict(schema_version="usdb-bitcoin-assumeutxo:v1", bootstrap_ready=ready, active_height=935000)))
        raise SystemExit(0 if ready else 1)
    if "ps" in args:
        print("fixture-container")
elif args[0] == "inspect":
    print("exited:" + os.environ.get("NATIVE_OBSERVER_EXIT", "0"))
elif args[0] != "update":
    raise SystemExit("Unexpected Docker operation: " + args[0])
''')
    binary.chmod(0o755)
    return directory
