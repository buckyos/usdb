#!/usr/bin/env python3
"""Download a pinned UTXO snapshot and reconcile Bitcoin Core snapshot activation."""

from __future__ import annotations

import argparse
import base64
from contextlib import contextmanager
from dataclasses import asdict, dataclass
import fcntl
import hashlib
import json
import math
import os
from pathlib import Path
import queue
import re
import shutil
import signal
import stat
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request

from assumeutxo_bootstrap import checkpoint_metadata, unsigned


SCHEMA = "usdb-bitcoin-assumeutxo:v1"
CHUNK_BYTES = 4 * 1024 * 1024


@dataclass(frozen=True)
class Snapshot:
    """Artifact identity; the production CLI only constructs this from the pinned catalog."""
    base_height: int
    base_hash: str
    file_sha256: str
    size_bytes: int
    chain: str = "main"


def pinned_snapshot(env: dict) -> Snapshot:
    if env.get("BTC_NETWORK", "bitcoin") != "bitcoin":
        raise ValueError("Core bootstrap requires BTC_NETWORK=bitcoin")
    base = unsigned(env, "BH_ASSUMEUTXO_BASE_HEIGHT", 935000)
    metadata = checkpoint_metadata(base)
    return Snapshot(base, metadata["base_hash"], metadata["file_sha256"], 9387990306)


def regular_file(path: Path) -> None:
    """Never replace or follow a non-regular managed file."""
    if path.is_symlink() or (path.exists() and not path.is_file()):
        raise ValueError(f"Expected a regular file: {path}")


def sync_directory(path: Path) -> None:
    descriptor = os.open(path, os.O_RDONLY | os.O_DIRECTORY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


def save_json(path: Path, value: dict) -> None:
    regular_file(path)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, prefix=".bootstrap-", delete=False) as output:
            temporary = Path(output.name)
            json.dump(value, output, indent=2, sort_keys=True)
            output.write("\n")
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
        sync_directory(path.parent)
    finally:
        if temporary is not None and temporary.exists():
            temporary.unlink()


@contextmanager
def exclusive_directory(path: Path):
    """Serialize tool-managed download/load requests; Core's datadir lock stays Core-owned."""
    if path.is_symlink():
        raise ValueError("Bootstrap state directory must not be a symlink")
    path.mkdir(mode=0o700, parents=True, exist_ok=True)
    descriptor = os.open(path / ".lock", os.O_RDWR | os.O_CREAT | os.O_NOFOLLOW, 0o600)
    try:
        if not stat.S_ISREG(os.fstat(descriptor).st_mode):
            raise ValueError("Bootstrap lock must be a regular file")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise ValueError("Another bootstrap operation owns this directory") from error
        yield
    finally:
        os.close(descriptor)


class Journal:
    """Persist phase/identity for recovery, never use a journal as chain validation evidence."""
    def __init__(self, path: Path, identity: dict):
        self.path = path
        regular_file(path)
        self.value = json.loads(path.read_text()) if path.exists() else {
            "schema_version": SCHEMA, "identity": identity, "started_at": time.time(), "phase": "new",
        }
        if self.value.get("schema_version") != SCHEMA or self.value.get("identity") != identity:
            raise ValueError("Bootstrap journal identity mismatch; use its original target or a dedicated directory")

    def phase(self, name: str, **details) -> None:
        now = time.time()
        previous = self.value["phase"]
        if previous != name:
            self.value["phase_started_at"] = now
        self.value.update(phase=name, updated_at=now, details=details)
        save_json(self.path, self.value)
        event = dict(phase=name, previous_phase=previous, elapsed_seconds=round(now - self.value["started_at"], 1), **details)
        print("Bitcoin bootstrap progress: " + json.dumps(event, sort_keys=True), file=sys.stderr, flush=True)


def verify_snapshot(path: Path, snapshot: Snapshot, journal: Journal) -> None:
    regular_file(path)
    if not path.is_file() or path.stat().st_size != snapshot.size_bytes:
        raise ValueError("UTXO snapshot file size mismatch")
    digest, processed, last = hashlib.sha256(), 0, time.monotonic()
    journal.phase("verifying_file", bytes=0, total_bytes=snapshot.size_bytes)
    with path.open("rb") as source:
        before = os.fstat(source.fileno())
        while chunk := source.read(CHUNK_BYTES):
            digest.update(chunk)
            processed += len(chunk)
            if time.monotonic() - last >= 10:
                journal.phase("verifying_file", bytes=processed, total_bytes=snapshot.size_bytes)
                last = time.monotonic()
        after = os.fstat(source.fileno())
    current = path.stat()
    if (before.st_size, before.st_mtime_ns, before.st_ctime_ns) != (after.st_size, after.st_mtime_ns, after.st_ctime_ns) or (current.st_dev, current.st_ino) != (after.st_dev, after.st_ino):
        raise ValueError("UTXO snapshot changed during file verification")
    if processed != snapshot.size_bytes or digest.hexdigest() != snapshot.file_sha256:
        raise ValueError("UTXO snapshot SHA-256 mismatch; preserve evidence and replace the invalid file explicitly")
    journal.phase("file_verified", bytes=processed, total_bytes=snapshot.size_bytes)


def source_url(value: str) -> str:
    parsed = urllib.parse.urlsplit(value)
    if parsed.scheme != "https" or not parsed.hostname or parsed.username or parsed.password or parsed.fragment or parsed.query:
        raise ValueError("Snapshot source must be HTTPS without credentials, query or fragment")
    return value


class HttpsRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        source_url(newurl)
        return super().redirect_request(request, fp, code, message, headers, newurl)


def download_snapshot(snapshot: Snapshot, destination: Path, url: str, *, reserve_bytes: int = 1024**3) -> None:
    """Resume only an identity-bound partial file, and publish only after a full SHA-256 scan."""
    if not destination.is_absolute():
        raise ValueError("Snapshot destination must be an absolute path")
    destination.parent.mkdir(parents=True, exist_ok=True)
    work = destination.with_name(destination.name + ".download")
    with exclusive_directory(work):
        journal = Journal(work / "progress.json", {"snapshot": asdict(snapshot), "destination": str(destination)})
        journal.phase("checking_file")
        regular_file(destination)
        if destination.exists():
            verify_snapshot(destination, snapshot, journal)
            return
        source_url(url)
        part = work / "snapshot.part"
        regular_file(part)
        offset = part.stat().st_size if part.exists() else 0
        if offset > snapshot.size_bytes:
            raise ValueError("Partial UTXO snapshot is larger than its pinned size")
        remaining = snapshot.size_bytes - offset
        if shutil.disk_usage(destination.parent).free < remaining + reserve_bytes:
            raise ValueError("Insufficient snapshot filesystem space for the remaining bytes and reserve")
        if remaining:
            request = urllib.request.Request(url, headers={"Range": f"bytes={offset}-{snapshot.size_bytes - 1}", "Accept-Encoding": "identity"})
            opener = urllib.request.build_opener(HttpsRedirect())
            journal.phase("downloading", bytes=offset, total_bytes=snapshot.size_bytes)
            try:
                with opener.open(request, timeout=30) as response:
                    source_url(response.url)
                    expected_range = f"bytes {offset}-{snapshot.size_bytes - 1}/{snapshot.size_bytes}"
                    ranges = response.headers.get_all("Content-Range", [])
                    if response.status == 206:
                        if ranges != [expected_range]:
                            raise ValueError("Snapshot server returned a mismatched Content-Range")
                    elif response.status != 200 or offset != 0 or ranges:
                        raise ValueError("Snapshot server did not honor the resume range")
                    lengths = response.headers.get_all("Content-Length", [])
                    if lengths != [str(remaining)] or response.headers.get("Content-Encoding", "identity") != "identity":
                        raise ValueError("Snapshot server returned a mismatched length or encoded body")
                    with part.open("ab") as output:
                        last = time.monotonic()
                        try:
                            while offset < snapshot.size_bytes:
                                chunk = response.read(min(CHUNK_BYTES, snapshot.size_bytes - offset))
                                if not chunk:
                                    raise ValueError("Snapshot download interrupted; rerun to resume the retained partial file")
                                output.write(chunk)
                                offset += len(chunk)
                                if time.monotonic() - last >= 10:
                                    output.flush()
                                    os.fsync(output.fileno())
                                    journal.phase("downloading", bytes=offset, total_bytes=snapshot.size_bytes)
                                    last = time.monotonic()
                        finally:
                            output.flush()
                            os.fsync(output.fileno())
            except (OSError, urllib.error.URLError) as error:
                # Do not echo request URLs or server bodies in operator diagnostics.
                raise ValueError("Snapshot transport failed; rerun to resume the retained partial file") from error
        verify_snapshot(part, snapshot, journal)
        if destination.exists() or destination.is_symlink():
            raise ValueError("Snapshot destination appeared during download; refusing to replace it")
        os.replace(part, destination)
        sync_directory(destination.parent)
        journal.phase("file_published", bytes=snapshot.size_bytes, total_bytes=snapshot.size_bytes)


class RpcFailure(Exception):
    def __init__(self, method: str, code: int | None = None):
        self.code = code
        super().__init__(f"Bitcoin RPC {method} failed" + (f" (code {code})" if code is not None else " (transport or response unavailable)"))


class NoRpcRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, request, fp, code, message, headers, newurl):
        return None


class Rpc:
    """Use short read timeouts and an unbounded load request; never follow authenticated redirects."""
    def __init__(self, url: str, *, cookie: Path | None = None, user: str = "", password: str = ""):
        parsed = urllib.parse.urlsplit(url)
        if parsed.scheme not in {"http", "https"} or not parsed.hostname or parsed.username or parsed.password or parsed.fragment or parsed.query:
            raise ValueError("Bitcoin RPC URL must be HTTP(S) without credentials, query or fragment")
        if cookie is None and (not user or not password):
            raise ValueError("Bitcoin RPC cookie or user/password authentication is required")
        self.url, self.cookie, self.user, self.password = url, cookie, user, password

    def call(self, method: str, params: list | None = None, *, timeout: float | None = 5):
        try:
            credential = self.cookie.read_text().strip() if self.cookie else f"{self.user}:{self.password}"
            if ":" not in credential or (not self.cookie and (not self.user or not self.password)):
                raise ValueError("Bitcoin RPC authentication is required")
            headers = {"Authorization": "Basic " + base64.b64encode(credential.encode()).decode(), "Content-Type": "application/json"}
            body = json.dumps(dict(jsonrpc="2.0", id="usdb-assumeutxo", method=method, params=params or [])).encode()
            # Core has no redirect responses; bypass ambient proxy configuration for this private RPC.
            opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), NoRpcRedirect())
            request = urllib.request.Request(self.url, body, headers)
            try:
                response = opener.open(request, timeout=timeout)
            except urllib.error.HTTPError as error:
                if error.code != 500:
                    raise RpcFailure(method, error.code) from error
                response = error
            with response:
                value = json.load(response)
            if not isinstance(value, dict) or value.get("id") != "usdb-assumeutxo":
                raise RpcFailure(method)
            if value.get("error") is not None:
                error = value["error"]
                raise RpcFailure(method, error.get("code") if isinstance(error, dict) else None)
            return value["result"]
        except RpcFailure:
            raise
        except (OSError, ValueError, KeyError) as error:
            raise RpcFailure(method) from error


def core_status(rpc: Rpc, snapshot: Snapshot) -> dict:
    """Inspect the active chain, independently reporting snapshot activation and full validation."""
    info = rpc.call("getblockchaininfo")
    network = rpc.call("getnetworkinfo")
    states = rpc.call("getchainstates")
    if info["chain"] != snapshot.chain or info["pruned"] is not False:
        raise ValueError("Core bootstrap requires the pinned network and prune=0")
    if type(network.get("version")) is not int or network["version"] < 310100:
        raise ValueError("Core bootstrap requires Bitcoin Core 31.1 or newer")
    chains = states["chainstates"]
    if not isinstance(chains, list) or not 1 <= len(chains) <= 2:
        raise ValueError("Unexpected Core chainstate layout")
    active = chains[-1]
    for chain in chains:
        if chain.get("snapshot_blockhash") not in (None, snapshot.base_hash):
            raise ValueError("Core has a different AssumeUTXO baseline")
    if type(active.get("blocks")) is not int or type(active.get("validated")) is not bool:
        raise ValueError("Core chainstate height or validation flag is invalid")
    if not active["validated"] and active.get("snapshot_blockhash") != snapshot.base_hash:
        raise ValueError("Unvalidated Core chainstate has no matching snapshot baseline")
    report = dict(schema_version=SCHEMA, rpc_available=True, bootstrap_ready=False,
                  headers=states["headers"], active_height=active["blocks"],
                  snapshot_active=active.get("snapshot_blockhash") == snapshot.base_hash,
                  history_validated=active["validated"] and active["blocks"] >= snapshot.base_height,
                  active_validated=active["validated"], chainstates=chains,
                  background_height=chains[0].get("blocks") if len(chains) == 2 else None,
                  phase="waiting_for_headers")
    if info["bestblockhash"] != active["bestblockhash"]:
        report["phase"] = "chain_changed_during_probe"
        return report
    if active["blocks"] >= snapshot.base_height:
        if rpc.call("getblockhash", [snapshot.base_height]) != snapshot.base_hash:
            raise ValueError("Core canonical baseline hash mismatch")
        report.update(bootstrap_ready=True, phase="snapshot_active" if report["snapshot_active"] else "fully_validated_chain")
    return report


def loading(rpc: Rpc) -> bool:
    commands = rpc.call("getrpcinfo")["active_commands"]
    if not isinstance(commands, list):
        raise ValueError("Core active_commands must be an array")
    return any(command.get("method") == "loadtxoutset" for command in commands)


def activate(snapshot: Snapshot, source: Path, rpc: Rpc, state_dir: Path, *, url: str = "",
             poll_seconds: float = 5, wait_seconds: float = 0, retry_interrupted: bool = False,
             reserve_bytes: int = 1024**3) -> dict:
    """Reconcile before every load; uncertain requests require observation or explicit recovery."""
    if not source.is_absolute() or not state_dir.is_absolute() or state_dir == Path("/") or not math.isfinite(poll_seconds) or not math.isfinite(wait_seconds) or poll_seconds <= 0 or wait_seconds < 0:
        raise ValueError("Invalid source path or bootstrap wait intervals")
    identity = dict(snapshot=asdict(snapshot), source=str(source), rpc_target_sha256=hashlib.sha256(rpc.url.encode()).hexdigest())
    with exclusive_directory(state_dir):
        journal = Journal(state_dir / "activation.json", identity)
        uncertain = journal.value["phase"] in {"load_requested", "loading", "load_uncertain"}
        deadline = time.monotonic() + wait_seconds if wait_seconds else None
        prepared, requested = False, False
        result_queue = queue.Queue()
        load_result = None

        def load():
            try:
                result_queue.put((True, rpc.call("loadtxoutset", [str(source)], timeout=None)))
            except Exception as error:
                result_queue.put((False, error))

        while True:
            if deadline is not None and time.monotonic() >= deadline:
                raise ValueError("Bootstrap observation deadline reached; Core load may still be running; rerun to reconcile")
            try:
                report = core_status(rpc, snapshot)
                if report["bootstrap_ready"]:
                    journal.phase(report["phase"], report=report)
                    return report
                if not result_queue.empty():
                    succeeded, load_result = result_queue.get_nowait()
                    if not succeeded:
                        uncertain = not isinstance(load_result, RpcFailure) or load_result.code is None
                        journal.phase("load_uncertain" if uncertain else "load_failed")
                        raise ValueError(str(load_result))
                    if not isinstance(load_result, dict) or load_result.get("base_height") != snapshot.base_height or load_result.get("tip_hash") != snapshot.base_hash:
                        journal.phase("load_uncertain")
                        raise ValueError("Core load result identity mismatch")
                if loading(rpc):
                    uncertain = True
                    journal.phase("loading", report=report)
                elif requested:
                    # Never send another request while our unbounded call is pending or its result is being reconciled.
                    journal.phase("load_requested", report=report)
                elif uncertain and not retry_interrupted:
                    journal.phase("load_uncertain", report=report)
                    raise ValueError("Previous load outcome is uncertain and no active load is visible; inspect Core, then use --retry-interrupted-load to retry explicitly")
                elif report["phase"] == "chain_changed_during_probe":
                    journal.phase(report["phase"], report=report)
                elif not prepared:
                    download_snapshot(snapshot, source, url, reserve_bytes=reserve_bytes)
                    prepared = True
                    continue
                else:
                    try:
                        header = rpc.call("getblockheader", [snapshot.base_hash])
                    except RpcFailure as error:
                        if error.code != -5:
                            raise
                        header = None
                    if header is None:
                        journal.phase("waiting_for_headers", report=report)
                    elif header.get("hash") != snapshot.base_hash or header.get("height") != snapshot.base_height:
                        raise ValueError("Core baseline header identity mismatch")
                    else:
                        # This fsync happens before sending the only mutating RPC in this operation.
                        journal.phase("load_requested", report=report)
                        requested = True
                        threading.Thread(target=load, daemon=True).start()
            except RpcFailure as error:
                if error.code not in (None, -28, 503):
                    raise ValueError(str(error)) from error
                # Preserve the uncertain phase across RPC warmup/flush timeouts.
                journal.phase("load_requested" if requested or uncertain else "waiting_for_rpc", rpc_available=False, error=str(error))
            time.sleep(poll_seconds)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["download", "bootstrap", "status"])
    parser.add_argument("--snapshot-file", type=Path, default=os.environ.get("BTC_ASSUMEUTXO_SNAPSHOT_FILE"))
    parser.add_argument("--source-url", default=os.environ.get("BTC_ASSUMEUTXO_SOURCE_URL", ""))
    parser.add_argument("--state-dir", type=Path, default=os.environ.get("BTC_ASSUMEUTXO_STATE_DIR", "/data/assumeutxo-state"))
    parser.add_argument("--rpc-url", default=os.environ.get("BTC_RPC_URL", "http://127.0.0.1:8332"))
    parser.add_argument("--cookie-file", type=Path, default=os.environ.get("BTC_COOKIE_FILE"))
    parser.add_argument("--poll-seconds", type=float, default=5)
    parser.add_argument("--wait-seconds", type=float, default=0, help="0 observes indefinitely; a timeout never cancels Core's import")
    parser.add_argument("--reserve-bytes", type=int, default=1024**3, help="Additional free bytes to retain on the artifact filesystem; not a full-node capacity estimate")
    parser.add_argument("--retry-interrupted-load", action="store_true", help="After inspecting Core, allow a new load if an earlier request has no known outcome and no active load is visible")
    args = parser.parse_args()
    try:
        snapshot = pinned_snapshot(os.environ)
        if args.reserve_bytes < 0:
            raise ValueError("Download reserve must not be negative")
        if args.command != "status" and args.snapshot_file is None:
            raise ValueError("BTC_ASSUMEUTXO_SNAPSHOT_FILE or --snapshot-file is required")
        if args.command == "download":
            download_snapshot(snapshot, args.snapshot_file, args.source_url, reserve_bytes=args.reserve_bytes)
            print(json.dumps(dict(schema_version=SCHEMA, phase="file_verified", snapshot=asdict(snapshot))))
        else:
            password = os.environ.get("BTC_RPC_PASSWORD", "")
            if os.environ.get("BTC_RPC_PASSWORD_FILE"):
                password = Path(os.environ["BTC_RPC_PASSWORD_FILE"]).read_text().strip()
            rpc = Rpc(args.rpc_url, cookie=args.cookie_file, user=os.environ.get("BTC_RPC_USER", ""), password=password)
            if args.command == "status":
                report = core_status(rpc, snapshot)
            else:
                report = activate(snapshot, args.snapshot_file, rpc, args.state_dir, url=args.source_url,
                                  poll_seconds=args.poll_seconds, wait_seconds=args.wait_seconds,
                                  retry_interrupted=args.retry_interrupted_load, reserve_bytes=args.reserve_bytes)
            print(json.dumps(report, sort_keys=True))
            return 0 if report["bootstrap_ready"] else 1
    except (OSError, ValueError, KeyError, TypeError, RpcFailure) as error:
        if args.command == "status":
            print(json.dumps(dict(schema_version=SCHEMA, bootstrap_ready=False, error=str(error))))
        else:
            print(f"Bitcoin bootstrap failed: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print("Bitcoin bootstrap observer stopped; Core may still be importing. Rerun to reconcile.", file=sys.stderr)
        return 130
    return 0


if __name__ == "__main__":
    def interrupt(_signum, _frame):
        raise KeyboardInterrupt
    signal.signal(signal.SIGTERM, interrupt)
    raise SystemExit(main())
