"""In-memory host process observations; reject every non-query command."""

import copy
from datetime import datetime, timedelta, timezone
import json
from pathlib import Path
import subprocess
import sys
import time


class ProcessHost:
    def __init__(self, bundle):
        self.bundle = bundle
        self.calls = []
        self.containers = {}
        self.systemd = {}
        self.fail_projects = set()
        self.fail_units = set()
        self.missing_details = set()
        self.fail_stats = False
        self.list_override = {}
        self.inspect_override = {}

    def unit(self, service, *, active="active", sub="running", pid=1234, code=0, status=0, result="success", **extra):
        name = f"usdb-node-{'bootstrap' if service == 'controller' else 'monitor'}-{self.bundle}.service"
        self.systemd[name] = dict(Id=name, LoadState="loaded", ActiveState=active, SubState=sub, MainPID=str(pid),
            UnitFileState="enabled", Result=result, ExecMainCode=str(code), ExecMainStatus=str(status), NRestarts="0",
            ExecMainStartTimestampMonotonic=str(time.monotonic_ns() // 1000 - 60_000_000),
            MemoryCurrent="16777216", MemoryMax="18446744073709551615") | extra
        return name

    def container(self, service, *, state="running", pid=2222, code=0, oom=False, **extra):
        identifier = f"{len(self.containers) + 1:064x}"
        project = self.bundle + "-bitcoin" if service.startswith("btc-") else self.bundle
        started = (datetime.now(timezone.utc) - timedelta(hours=1)).isoformat()
        self.containers[identifier] = dict(id=identifier, project=project, service=service, state=state, pid=pid,
            started_at=started, finished_at=started if state == "exited" else "0001-01-01T00:00:00Z",
            restart_count=3, exit_code=code, oom_killed=oom, memory_limit_bytes=4 * 1024**3, health=None,
            **extra)
        return identifier

    def run(self, args, **kwargs):
        self.calls.append(list(args))
        assert kwargs["timeout"] > 0 and kwargs["capture_output"] and kwargs["check"] is False
        assert kwargs["env"]["LC_ALL"] == "C"
        stdout, code = "", 0
        if args[:2] == ["systemctl", "show"]:
            unit = args[-1]
            assert unit in {f"usdb-node-bootstrap-{self.bundle}.service", f"usdb-node-monitor-{self.bundle}.service"}
            if unit in self.fail_units:
                raise subprocess.TimeoutExpired(args, kwargs["timeout"])
            props = self.systemd.get(unit, dict(Id=unit, LoadState="not-found", ActiveState="inactive", SubState="dead", MainPID="0"))
            stdout = "\n".join(f"{k}={v}" for k, v in props.items())
        elif args[:2] == ["docker", "ps"]:
            assert "--all" in args and "--no-trunc" in args
            project = args[args.index("--filter") + 1].removeprefix("label=com.docker.compose.project=")
            assert project in {self.bundle, self.bundle + "-bitcoin"}
            if project in self.fail_projects:
                raise PermissionError("DO_NOT_PRINT_DOCKER_SECRET")
            listing = self.list_override.get(project, [dict(id=k, service=v["service"], project=v["project"])
                for k, v in self.containers.items() if v["project"] == project])
            stdout = "\n".join(json.dumps(item) for item in listing)
        elif args[:2] == ["docker", "inspect"]:
            assert args[2] == "--format" and ".Config.Env" not in args[3] and ".Path" not in args[3]
            records = []
            for identifier in args[4:]:
                assert identifier in self.containers
                if identifier in self.missing_details:
                    code = 1
                else:
                    records.append(self.inspect_override.get(identifier, self.containers[identifier]))
            stdout = "\n".join(json.dumps(item) for item in records)
        elif args[:2] == ["docker", "stats"]:
            assert "--no-stream" in args and "--no-trunc" in args
            if self.fail_stats:
                raise subprocess.TimeoutExpired(args, kwargs["timeout"])
            records = []
            for identifier in args[args.index("--format") + 2:]:
                assert identifier in self.containers and self.containers[identifier]["state"] == "running"
                records.append(dict(ID=identifier, MemUsage="2.5GiB / 4GiB"))
            stdout = "\n".join(json.dumps(item) for item in records)
        else:
            raise AssertionError("Unexpected command: " + repr(args))
        return subprocess.CompletedProcess(args, code, stdout, "DO_NOT_PRINT_STDERR_SECRET")

    def changed(self, identifier, **values):
        self.containers[identifier] = {**copy.deepcopy(self.containers[identifier]), **values}

    def install_cli(self, directory):
        """Exercise packaged subprocess entrypoints with fake system binaries, never host services."""
        directory.mkdir()
        data = directory / "host.json"
        data.write_text(json.dumps(dict(bundle=self.bundle, containers=self.containers, systemd=self.systemd)))
        script = (f"#!{sys.executable}\nimport json, sys\nfrom pathlib import Path\n"
                  f"sys.path.insert(0, {str(Path(__file__).resolve().parents[1])!r})\n"
                  "from common.node_top import ProcessHost\n"
                  "data = json.loads(Path(__file__).with_name('host.json').read_text())\n"
                  "host = ProcessHost(data['bundle'])\nhost.containers = data['containers']\nhost.systemd = data['systemd']\n"
                  "result = host.run([Path(sys.argv[0]).name, *sys.argv[1:]], timeout=5, capture_output=True, check=False, env={'LC_ALL':'C'})\n"
                  "sys.stdout.write(result.stdout)\nsys.stderr.write(result.stderr)\nraise SystemExit(result.returncode)\n")
        for name in ("docker", "systemctl"):
            executable = directory / name
            executable.write_text(script)
            executable.chmod(0o700)
