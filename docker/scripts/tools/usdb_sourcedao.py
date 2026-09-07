"""Run release-pinned SourceDAO ceremonies as explicit, durable Docker tasks."""
from __future__ import annotations

import argparse
from contextlib import contextmanager
import fcntl
import hashlib
import json
import os
from pathlib import Path
import re
import stat
import subprocess
import time
import uuid

import usdb_node as node
from sourcedao_release import safe_file, semantic_digest

SCHEMA = "usdb-node-sourcedao:v1"
IMAGE_NAME = "sourcedao-bootstrap-tools"
LABEL = "io.usdb.sourcedao"
ACTIVE = {"created", "running", "restarting", "paused", "removing"}


def root(layout) -> Path:
    """Keep recovery and public exports outside immutable release directories."""
    return layout.node_env.parent / "sourcedao"


def task_path(layout) -> Path:
    return root(layout) / "task.json"


def read_task(layout) -> dict | None:
    path = task_path(layout)
    if not path.exists():
        return None
    task = node._load_json(path)
    if (task.get("schema_version") != SCHEMA or not isinstance(task.get("task_id"), str) or
            re.fullmatch(r"[0-9a-f]{32}", task["task_id"]) is None or
            task.get("action") not in {"bootstrap", "export", "validate"} or
            not isinstance(task.get("binding"), dict) or
            set(task["binding"]) != {"network", "chain_id", "genesis_hash", "config_sha256", "golden_sha256", "image"} or
            not isinstance(task.get("release_id"), str)):
        raise ValueError("Invalid SourceDAO task record; preserve it for recovery")
    return task


def docker(arguments: list[str], *, timeout: int = 60) -> str:
    """Never pass signing material through argv or the Docker environment."""
    result = subprocess.run(["docker", *arguments], capture_output=True, text=True, timeout=timeout)
    if result.returncode:
        raise ValueError(f"SourceDAO Docker {arguments[0]} failed: {result.stderr.strip() or result.stdout.strip()}")
    # Docker forwards container stderr separately even when `logs` succeeds.
    # Preserve both streams before removing a stopped task container.
    return (result.stdout + result.stderr if arguments[0] == "logs" else result.stdout).strip()


def container_name(layout) -> str:
    scope = hashlib.sha256(str(layout.node_env.resolve()).encode()).hexdigest()[:20]
    return f"usdb-sourcedao-{scope}"


def inspect_task(layout, task: dict | None) -> dict | None:
    """Distinguish a missing task container from an unavailable Docker daemon."""
    name = container_name(layout)
    ids = docker(["container", "ls", "-aq", "--no-trunc", "--filter", f"name=^/{name}$"]).split()
    if not ids:
        return None
    if len(ids) != 1:
        raise ValueError("SourceDAO container identity is ambiguous")
    info = json.loads(docker(["container", "inspect", ids[0]]))[0]
    labels = info.get("Config", {}).get("Labels") or {}
    if task is None or labels.get(f"{LABEL}.task") != task.get("task_id") or labels.get(f"{LABEL}.binding") != semantic_digest(task["binding"]):
        raise ValueError("SourceDAO container has no matching private task record; inspect it before recovery")
    if info["Config"].get("Image") != task["binding"]["image"]:
        raise ValueError("SourceDAO container image differs from its task record")
    return info


def require_idle(layout) -> None:
    """Block release/configuration changes while a detached ceremony owns the node."""
    task = read_task(layout)
    if task is None:
        return
    info = inspect_task(layout, task)
    if info and info["State"]["Status"] in ACTIVE:
        raise ValueError("A SourceDAO task is active; use 'usdb-node sourcedao status --watch' before changing node configuration")


def context(layout) -> dict:
    """Derive all runtime paths and identities from verified installed artifacts."""
    node._validate_node_config(layout, require_runtime=False, require_bitcoin_runtime=False)
    node._validate_node_release_images(layout)
    network = node.validate_network_bundle(layout.bundle_dir)
    artifacts = network["artifacts"]
    if "sourcedao_bootstrap_freeze" not in artifacts:
        raise ValueError("SourceDAO operations require a frozen bootstrap bundle")
    config = node._load_json(safe_file(layout.bundle_dir, artifacts["sourcedao_bootstrap"]["path"]))
    freeze = node._load_json(safe_file(layout.bundle_dir, artifacts["sourcedao_bootstrap_freeze"]["path"]))
    manifest = node._load_json(layout.manifest_path)
    image = node._require_image(manifest, "sourcedao_tools", IMAGE_NAME)
    entry = manifest["images"]["sourcedao_tools"]
    source = manifest.get("repositories", {}).get("source_dao", {})
    if (source.get("repository") != "buckyos/SourceDAO" or
            entry.get("source_repository") != source.get("repository") or
            entry.get("source_revision") != source.get("revision") or
            re.fullmatch(r"[0-9a-f]{40}", source.get("revision", "")) is None or
            entry.get("platform") != "linux/amd64" or
            entry.get("attestation") != {"repository": "buckyos/SourceDAO", "signer_workflow": "buckyos/SourceDAO/.github/workflows/usdb-tools-image.yml"}):
        raise ValueError("SourceDAO tools source identity differs from the release manifest")
    genesis = layout.network_identity["genesis_block_hash"]
    binding = {"network": layout.bundle_id, "chain_id": config["chainId"], "genesis_hash": genesis,
               "config_sha256": freeze["config_semantic_sha256"], "golden_sha256": freeze["golden_sha256"], "image": image}
    relative = Path(str(config["chainId"])) / genesis / binding["config_sha256"]
    private_root, public_root = root(layout) / "private", root(layout) / "public"
    state = private_root / relative / "state.json"
    env = node.read_env(layout.node_env)
    return {"binding": binding, "private_root": private_root, "public_root": public_root,
            "state": state, "public_state": public_root / relative / "sourcedao-bootstrap-public-state.json",
            "validation": public_root / relative / "sourcedao-bootstrap-validation.json",
            "rpc": node._host_rpc_url(env, "USDB_HTTP_BIND_ADDRESS", "USDB_HTTP_BIND_PORT", 8545)}


def mount(source: Path, target: str, *, readonly: bool = True) -> list[str]:
    value = str(source.resolve())
    if any(character in value for character in (",", "\n", "\r", "\0")):
        raise ValueError("SourceDAO mount path contains an unsupported character")
    return ["--mount", f"type=bind,src={value},dst={target}" + (",readonly" if readonly else "")]


def base_arguments(layout, ctx: dict) -> list[str]:
    """Use host loopback RPC without exposing a port or mounting Docker control."""
    return ["--network", "host", "--user", f"{os.getuid()}:{os.getgid()}", "--read-only",
            "--cap-drop", "ALL", "--security-opt", "no-new-privileges", "--pids-limit", "128",
            "--memory", "2g", "--cpus", "1", "--tmpfs", "/tmp:rw,nosuid,nodev,noexec,size=64m",
            *mount(layout.bundle_dir, "/release")]


def check(layout, ctx: dict) -> dict:
    ensure_image(ctx)
    output = docker(["run", "--rm", *base_arguments(layout, ctx), ctx["binding"]["image"],
                     "check", "--bundle-dir", "/release", "--rpc-url", ctx["rpc"]], timeout=90)
    value = json.loads(output)
    if value.get("schema_version") != "sourcedao-bootstrap-check:v1":
        raise ValueError("Unexpected SourceDAO check output")
    for key in ("chain_id", "genesis_hash", "config_sha256", "golden_sha256"):
        if value.get(key) != ctx["binding"][key]:
            raise ValueError(f"SourceDAO live check identity mismatch: {key}")
    return value


def ensure_image(ctx: dict) -> None:
    """Give the initial download its own timeout, separate from read-only RPC checks."""
    try:
        docker(["image", "inspect", ctx["binding"]["image"]])
    except ValueError:
        docker(["pull", ctx["binding"]["image"]], timeout=600)


@contextmanager
def task_lock(layout):
    """Serialize starts/retries; Docker owns the long-running task after this lock exits."""
    directory = root(layout)
    directory.mkdir(parents=True, exist_ok=True, mode=0o700)
    descriptor = os.open(directory / ".operation.lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    with os.fdopen(descriptor, "w") as handle:
        try:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("Another SourceDAO command is starting; retry after it finishes") from error
        yield


def key_file(filename: Path, ctx: dict) -> Path:
    """Only mount an owner-private regular file; never read its signing bytes in Python."""
    path = filename.expanduser().absolute()
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or stat.S_IMODE(info.st_mode) not in {0o600, 0o400}:
        raise ValueError("Bootstrap key must be an owner-owned regular file with mode 0600 or 0400")
    path = path.resolve()
    if path.is_relative_to(ctx["public_root"].resolve()):
        raise ValueError("Bootstrap key must not be stored under public exports")
    if path.is_relative_to(ctx["private_root"].resolve()):
        raise ValueError("Bootstrap key must be outside the writable recovery mount")
    return path


def prepare_previous(layout, ctx: dict, previous: dict | None) -> None:
    """Clear only a lock proven to belong to this stopped managed container."""
    info = inspect_task(layout, previous)
    if info and info["State"]["Status"] in ACTIVE - {"created"}:
        raise ValueError("A SourceDAO task is already active; use sourcedao status --watch")
    lock = Path(str(ctx["state"]) + ".lock")
    if lock.exists():
        owner = node._load_json(lock)
        if (not info or not previous or any(previous["binding"].get(key) != ctx["binding"][key]
                for key in ("chain_id", "genesis_hash", "config_sha256", "golden_sha256")) or
                owner.get("managed_task_id") != previous["task_id"] or info["State"]["Status"] not in {"exited", "dead"}):
            raise ValueError("Bootstrap state lock has no proven stopped owner; preserve it and inspect the original task")
        lock.unlink()
    if info:
        # Keep the previous task log privately before replacing its stopped container.
        logs = docker(["logs", info["Id"]])
        node._atomic_write_private(root(layout) / "logs" / f"{previous['task_id']}.log", logs + "\n")
        docker(["container", "rm", info["Id"]])


def start(layout, action: str, *, key: Path | None = None) -> dict:
    """Start one explicit operation; daemon restarts never automatically unlock a signer."""
    with node.node_operation_lock(layout, "sourcedao-" + action), task_lock(layout):
        ctx = context(layout)
        previous = read_task(layout)
        info = inspect_task(layout, previous)
        if info and info["State"]["Status"] in ACTIVE - {"created"}:
            raise ValueError("A SourceDAO task is already active; use sourcedao status --watch")
        secret = key_file(key, ctx) if key is not None else None
        if action == "bootstrap" and secret is None:
            raise ValueError("bootstrap requires --key-file")
        # Pull the immutable image before any key mount or task record is created.
        ensure_image(ctx)
        live = check(layout, ctx) if action != "export" else None
        if action == "bootstrap" and live["blockers"]:
            raise ValueError("SourceDAO bootstrap is blocked: " + "; ".join(live["blockers"]))
        if action == "bootstrap" and not live["finalized"] and live.get("fee_split_block") is not None:
            # Each remaining transaction can need its own block. Do not begin a
            # ceremony whose finalization cannot fit before the consensus gate.
            journal_path = ctx["state"].with_name("state.json.transactions.json")
            confirmed = 0
            if journal_path.is_file():
                journal = node._load_json(journal_path)
                if any(journal.get("identity", {}).get(key) != ctx["binding"][key]
                       for key in ("chain_id", "genesis_hash", "config_sha256", "golden_sha256")):
                    raise ValueError("Recovery journal identity differs from the frozen ceremony")
                confirmed = sum(bool(entry.get("block_hash")) for entry in journal["transactions"])
            remaining = max(1, 22 - confirmed)
            if live["checkpoint"]["number"] + remaining >= int(live["fee_split_block"]):
                raise ValueError(f"Insufficient blocks before the Dividend fee split gate for {remaining} remaining bootstrap transactions")
        if action in {"export", "validate"}:
            required = ctx["state"] if action == "export" else ctx["public_state"]
            if not required.is_file():
                raise ValueError(f"SourceDAO {action} input is missing: {required}")
        if action == "validate" and not live["finalized"]:
            raise ValueError("SourceDAO Dividend bootstrap is not finalized on this chain")
        prepare_previous(layout, ctx, previous)
        for directory in (ctx["private_root"], ctx["public_root"]):
            directory.mkdir(parents=True, exist_ok=True, mode=0o700)
        task = {"schema_version": SCHEMA, "task_id": uuid.uuid4().hex, "action": action,
                "release_id": layout.release_id, "binding": ctx["binding"], "created_at": node.datetime.now(node.timezone.utc).isoformat()}
        node._atomic_write_private(task_path(layout), json.dumps(task, indent=2, sort_keys=True) + "\n")
        command = {"bootstrap": "bootstrap", "export": "export-state", "validate": "validate"}[action]
        arguments = ["create", "--name", container_name(layout), "--restart", "no", "--init",
                     "--label", f"{LABEL}.task={task['task_id']}", "--label", f"{LABEL}.binding={semantic_digest(task['binding'])}",
                     "--log-opt", "max-size=10m", "--log-opt", "max-file=2", *base_arguments(layout, ctx),
                     "--env", f"SOURCE_DAO_MANAGED_TASK_ID={task['task_id']}"]
        if action in {"bootstrap", "export"}:
            arguments += mount(ctx["private_root"], "/private", readonly=False)
        arguments += mount(ctx["public_root"], "/public", readonly=action == "bootstrap")
        if secret is not None:
            arguments += mount(secret, "/run/bootstrap-admin.key")
            arguments += ["--env", "SOURCE_DAO_BOOTSTRAP_PRIVATE_KEY_FILE=/run/bootstrap-admin.key"]
        arguments += [ctx["binding"]["image"], command, "--bundle-dir", "/release"]
        if action != "export":
            arguments += ["--rpc-url", ctx["rpc"]]
        docker(arguments)
        docker(["start", container_name(layout)])
        return task


def status(layout, *, live: bool = True) -> dict:
    """Report process outcome separately from fresh chain readiness and public validation."""
    ctx = context(layout)
    task = read_task(layout)
    info = inspect_task(layout, task)
    result = {"schema_version": SCHEMA, "release_id": layout.release_id, "outcome": "NOT_STARTED",
              "state_file": str(ctx["state"]), "public_state": str(ctx["public_state"]), "validation": str(ctx["validation"])}
    if task:
        result.update(action=task["action"], task_id=task["task_id"], task_release_id=task["release_id"])
        if task["binding"] != ctx["binding"]:
            result.update(outcome="DIFFERENT_RELEASE", error="The retained task belongs to different tools or frozen inputs")
            return result
        if info is None:
            result.update(outcome="INTERRUPTED", error="Task container is missing; recovery files were preserved")
        else:
            state = info["State"]
            result.update(container_state=state["Status"], exit_code=state["ExitCode"], oom_killed=state.get("OOMKilled", False))
            result["outcome"] = ("STARTING" if state["Status"] == "created" else "RUNNING" if state["Status"] in ACTIVE else
                                 "SUCCEEDED" if state["Status"] == "exited" and state["ExitCode"] == 0 and not state.get("OOMKilled") else "FAILED")
            if result["outcome"] == "FAILED":
                result["error"] = "Tool container failed; inspect its private log with docker logs " + container_name(layout)
    if ctx["state"].is_file():
        state = node._load_json(ctx["state"])
        identity = state.get("ceremony_identity", {})
        if any(identity.get(key) != ctx["binding"][key] for key in ("chain_id", "genesis_hash", "config_sha256", "golden_sha256")):
            raise ValueError("Private bootstrap state identity differs from the selected release")
        result.update(bootstrap_status=state.get("status"), current_step=state.get("current_step"))
        if task and task["action"] == "bootstrap" and result["outcome"] == "SUCCEEDED" and state.get("status") != "completed":
            result.update(outcome="FAILED", error="Container exited without completed bootstrap state")
    elif task and task["action"] == "bootstrap" and result["outcome"] == "SUCCEEDED":
        result.update(outcome="FAILED", error="Container exited without bootstrap state")
    if task and task["action"] in {"export", "validate"} and result["outcome"] == "SUCCEEDED":
        output = ctx["public_state"] if task["action"] == "export" else ctx["validation"]
        if not output.is_file():
            result.update(outcome="FAILED", error="Tool exited without its expected public output")
        else:
            record = node._load_json(output)
            identity = record.get("ceremony_identity", {}) if task["action"] == "export" else record.get("evidence", {})
            valid = (record.get("record_schema") == "sourcedao-bootstrap-public-state:v1" if task["action"] == "export" else
                     record.get("status") == "ok" and record.get("mode") == "strict")
            if not valid or any(identity.get(key) != ctx["binding"][key] for key in ("genesis_hash", "config_sha256", "golden_sha256")):
                result.update(outcome="FAILED", error="Public output does not match this ceremony")
    if live:
        try:
            result["chain"] = check(layout, ctx)
            if result["outcome"] == "SUCCEEDED" and task["action"] in {"bootstrap", "validate"} and not result["chain"]["finalized"]:
                result.update(outcome="FAILED", error="Dividend bootstrap is not finalized on the observed chain")
        except (ValueError, OSError, subprocess.SubprocessError) as error:
            result.update(observation_error=str(error))
            if result["outcome"] not in {"RUNNING", "STARTING"}:
                result.update(outcome="UNAVAILABLE", error=str(error))
    return result


def display(value: dict, *, as_json: bool) -> None:
    if as_json:
        print(json.dumps(value, indent=2, sort_keys=True), flush=True)
        return
    print(f"SourceDAO | {value.get('action', 'check')} | {value.get('outcome', 'CHECKED')}", flush=True)
    chain = value.get("chain", value)
    if "checkpoint" in chain:
        print(f"  block={chain['checkpoint']['number']} finalized={chain['finalized']} admin={chain['bootstrap_admin']}")
        for blocker in chain["blockers"]:
            print(f"  blocked: {blocker}")
    for key in ("current_step", "error", "observation_error", "state_file", "public_state", "validation"):
        if value.get(key):
            print(f"  {key}: {value[key]}", flush=True)


def add_parser(subparsers):
    parser = subparsers.add_parser("sourcedao", help="Check, bootstrap, export and validate release-bound SourceDAO contracts",
                                  description="Use the installed frozen bundle and digest-pinned tools image. Only bootstrap signs transactions.")
    actions = parser.add_subparsers(dest="sourcedao_action", required=True)
    descriptions = {
        "check": ("Inspect chain identity, predeploys and bootstrap prerequisites",
                  "Read-only preflight; no key or deployment state is needed. This does not replace strict validation."),
        "bootstrap": ("Deploy or resume the frozen SourceDAO ceremony",
                      "Requires the frozen Bootstrap Admin key. Runs detached; preserve private state and the signed journal for recovery."),
        "status": ("Observe the latest task, live finalization marker and output paths",
                   "SUCCEEDED refers to the latest task. The live finalized marker is not PoW finality or a full validation report."),
        "export": ("Export public deployment evidence from completed private recovery records",
                   "Requires completed private state and its signed transaction journal. Does not read chain RPC or need a key. "
                   "Writes a whitelisted public state for validate/acceptance; preserves private inputs and refuses different existing output. "
                   "It does not verify current canonical receipts. Waits for the detached task; status --watch reconnects after SSH loss."),
        "validate": ("Verify deployed contracts against the frozen inputs at one checkpoint",
                     "Requires exported public state and RPC access to the checkpoint's historical state; no key or private journal. "
                     "Strictly checks code, wiring and initial configuration. First run selects latest; repeats recheck the original report's "
                     "checkpoint and preserve its bytes. Run the first check before DAO business activity. Does not wait for confirmations "
                     "or create release acceptance. Waits for the detached task; status --watch reconnects after SSH loss."),
    }
    for action, (summary, description) in descriptions.items():
        command = actions.add_parser(action, help=summary, description=description)
        command.add_argument("--json", action="store_true", help="Print structured observations")
        if action == "bootstrap":
            command.add_argument("--key-file", type=Path, required=True, help="Owner-private file containing the bootstrap admin key")
            command.add_argument("--wait", action="store_true", help="Observe the detached task until it exits")
        if action == "status":
            command.add_argument("--watch", action="store_true", help="Observe until the current task exits; Ctrl-C leaves it running")
            command.add_argument("--interval", type=int, default=10, help="Seconds between observations (default: 10)")


def execute(layout, args: argparse.Namespace) -> int:
    action = args.sourcedao_action
    if action == "check":
        value = check(layout, context(layout))
        display(value, as_json=args.json)
        return 1 if value["blockers"] else 0
    if action in {"bootstrap", "export", "validate"}:
        task = start(layout, action, key=getattr(args, "key_file", None))
        if action == "bootstrap" and not args.wait:
            display({"schema_version": SCHEMA, "outcome": "STARTED", "action": action, "task_id": task["task_id"]}, as_json=args.json)
            if not args.json:
                print("Use usdb-node sourcedao status --watch. SSH disconnects do not stop this task.")
            return 0
    watching = action in {"export", "validate"} or getattr(args, "wait", False) or getattr(args, "watch", False)
    interval = getattr(args, "interval", 10)
    if interval <= 0:
        raise ValueError("SourceDAO watch interval must be positive")
    try:
        while True:
            value = status(layout, live=action not in {"export", "validate"})
            display(value, as_json=args.json)
            if not watching or value["outcome"] not in {"RUNNING", "STARTING"}:
                return 0 if value["outcome"] in {"SUCCEEDED", "NOT_STARTED"} else 1
            time.sleep(interval)
    except KeyboardInterrupt:
        if not args.json:
            print("Observation stopped; the Docker task continues. Use sourcedao status to reconnect.")
        return 130
