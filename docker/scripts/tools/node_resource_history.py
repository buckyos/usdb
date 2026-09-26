"""Bounded private resource history, independent of the durable alert journal."""
from __future__ import annotations

from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import sqlite3
import uuid
import zlib

from node_monitor_store import StateError, encode, private_directory, private_file

SCHEMA = "usdb-resource-history:v1"
MIB = 1024 * 1024
DAY = 86400000


def pack(value):
    payload = encode(value).encode()
    if len(payload) > 128 * 1024:
        raise ValueError("Resource record exceeds its size limit")
    return zlib.compress(payload)


def unpack(value):
    decoder = zlib.decompressobj()
    try:
        payload = decoder.decompress(value, 128 * 1024 + 1)
    except zlib.error:
        raise ValueError("Corrupt resource history record; preserve the database") from None
    if len(payload) > 128 * 1024 or not decoder.eof:
        raise ValueError("Invalid resource history record")
    return json.loads(payload)


class History:
    """One connection per writer; read-only queries never create or migrate files."""

    def __init__(self, path, scope, *, writable=False, max_mib=256):
        self.path, self.max_mib = Path(path), max_mib
        private_directory(self.path.parent, create=writable)
        if writable:
            try:
                os.close(os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY | os.O_NOFOLLOW, 0o600))
            except FileExistsError:
                pass
        private_file(self.path)
        for suffix in ("-wal", "-shm", "-journal"):
            sidecar = Path(str(self.path) + suffix)
            if sidecar.exists() or sidecar.is_symlink():
                private_file(sidecar)
        self.db = sqlite3.connect(self.path.as_uri() + ("?mode=rw" if writable else "?mode=ro"), uri=True, timeout=0.5)
        self.db.row_factory = sqlite3.Row
        try:
            if writable:
                # The transaction serializes first initialization across controller and sampler.
                self.db.execute("PRAGMA auto_vacuum=INCREMENTAL")
                self.db.execute("BEGIN IMMEDIATE")
            version = self.db.execute("PRAGMA user_version").fetchone()[0]
            tables = self.db.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
            if writable and version == 0 and not tables:
                self.db.execute("CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL)")
                for name in ("raw", "minute", "transitions"):
                    self.db.execute(f"CREATE TABLE {name}(id INTEGER PRIMARY KEY AUTOINCREMENT, at_ms INTEGER NOT NULL, session TEXT NOT NULL, identity TEXT UNIQUE, payload BLOB NOT NULL)")
                    self.db.execute(f"CREATE INDEX {name}_time ON {name}(at_ms)")
                self.put("scope", scope)
                self.put("schema_version", SCHEMA)
                self.db.execute("PRAGMA user_version=1")
            elif version != 1 or self.get("schema_version") != SCHEMA:
                raise StateError("RESOURCE_HISTORY_VERSION_UNSUPPORTED", "Unsupported resource history version; preserve the database")
            if self.get("scope") != scope:
                raise StateError("RESOURCE_HISTORY_NETWORK_MISMATCH", "Resource history belongs to a different network")
            if writable:
                self.db.commit()
                self.db.execute("PRAGMA journal_mode=WAL")
                self.db.execute("PRAGMA synchronous=FULL")
                self.db.execute("PRAGMA wal_autocheckpoint=64")
                self.db.execute("PRAGMA journal_size_limit=1048576")
                pages = (max_mib - 8) * MIB // self.db.execute("PRAGMA page_size").fetchone()[0]
                self.db.execute(f"PRAGMA max_page_count={pages}")
            else:
                self.db.execute("PRAGMA query_only=ON")
        except BaseException:
            self.db.close()
            raise

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def close(self):
        self.db.close()

    def get(self, key, default=None):
        row = self.db.execute("SELECT value FROM metadata WHERE key=?", (key,)).fetchone()
        return json.loads(row[0]) if row else default

    def put(self, key, value):
        self.db.execute("INSERT OR REPLACE INTO metadata VALUES (?,?)", (key, encode(value)))

    def guard_size(self):
        """A long-lived reader may pin WAL pages; suspend history, never fill the disk."""
        wal = Path(str(self.path) + "-wal")
        if (wal.exists() and wal.stat().st_size > 4 * MIB) or self.path.stat().st_size > (self.max_mib - 4) * MIB:
            raise StateError("RESOURCE_HISTORY_CAPACITY", "Resource history reached its disk budget; check readers and retention")

    def append(self, value, *, node_id=None, failed_samples=0):
        self.guard_size()
        at, session = value["at_ms"], value["session_id"]
        # Do not average across boots, releases, phases, limits or container replacement.
        identities = sorted((item["service"], item["identity"]) for item in value["items"])
        identity = hashlib.sha256(encode([session, value["release_id"], value.get("boot_id"),
                                         value.get("configuration"), identities]).encode()).hexdigest()
        bucket = at // 60000 * 60000
        identity = f"{bucket}:{identity}"
        with self.db:
            self.db.execute("INSERT INTO raw(at_ms,session,payload) VALUES (?,?,?)", (at, session, pack(value)))
            row = self.db.execute("SELECT payload FROM minute WHERE identity=?", (identity,)).fetchone()
            aggregate = unpack(row[0]) if row else dict(at_ms=bucket, session_id=session, release_id=value["release_id"],
                boot_id=value.get("boot_id"), configuration=value.get("configuration"), samples=0, items=[])
            aggregate["samples"] += 1
            aggregate["last_sample_ms"] = at
            aggregate.setdefault("first_sample_ms", at)
            aggregate["observation"] = value.get("observation", {})
            if value.get("observation_is_new"):
                observation = value.get("observation", {})
                counts = aggregate.setdefault("collection_outcomes", {})
                outcome = observation.get("collection", {}).get("outcome", "unknown")
                counts[outcome] = counts.get(outcome, 0) + 1
                probes = aggregate.setdefault("probes", {})
                for probe in observation.get("probes", []):
                    key = probe["service"] + ":" + probe["operation"]
                    stats = probes.setdefault(key, dict(count=0, min_ms=probe["duration_ms"], max_ms=0, sum_ms=0, outcomes={}))
                    stats.update(count=stats["count"] + 1, min_ms=min(stats["min_ms"], probe["duration_ms"]),
                                 max_ms=max(stats["max_ms"], probe["duration_ms"]), sum_ms=stats["sum_ms"] + probe["duration_ms"])
                    stats["outcomes"][probe["outcome"]] = stats["outcomes"].get(probe["outcome"], 0) + 1
            previous = {item["service"]: item for item in aggregate["items"]}
            for item in value["items"]:
                target = previous.setdefault(item["service"], dict(service=item["service"], identity=item["identity"],
                                                                  status_counts={}, missing_counts={}, metrics={}))
                for counts, keys in (("status_counts", [item["status"]]), ("missing_counts", item.get("missing", []))):
                    for key in keys:
                        target[counts][key] = target[counts].get(key, 0) + 1
                for key, number in item["metrics"].items():
                    if type(number) not in {float, int} or not math.isfinite(number):
                        continue
                    stats = target["metrics"].setdefault(key, dict(count=0, min=number, max=number, sum=0))
                    stats.update(count=stats["count"] + 1, min=min(stats["min"], number), max=max(stats["max"], number), sum=stats["sum"] + number)
            aggregate["items"] = list(previous.values())
            self.db.execute("INSERT INTO minute(at_ms,session,identity,payload) VALUES (?,?,?,?) ON CONFLICT(identity) DO UPDATE SET payload=excluded.payload",
                            (bucket, session, identity, pack(aggregate)))
            self.put("last_sample_ms", at)
            self.put("session_id", session)
            if node_id is not None:
                self.put("node_id", node_id)
            if failed_samples:
                self.put("failed_samples", self.get("failed_samples", 0) + failed_samples)

    def transition(self, at, code, payload):
        self.guard_size()
        value = dict(at_ms=at, event_id=uuid.uuid4().hex, code=code, **payload)
        with self.db:
            self.db.execute("INSERT INTO transitions(at_ms,session,identity,payload) VALUES (?,?,?,?)",
                            (at, payload.get("session_id", ""), value["event_id"], pack(value)))

    def prune(self, at, settings):
        """Retention is best effort within a hard database budget; evictions are visible."""
        deleted = 0
        with self.db:
            for name, days in (("raw", settings["resource_raw_days"]), ("minute", settings["resource_minute_days"]),
                               ("transitions", settings["resource_minute_days"])):
                self.db.execute(f"DELETE FROM {name} WHERE at_ms < ?", (at - days * DAY,))
            size = self.db.execute("PRAGMA page_size").fetchone()[0]
            for name in ("raw", "minute", "transitions"):
                while True:
                    pages = self.db.execute("PRAGMA page_count").fetchone()[0] - self.db.execute("PRAGMA freelist_count").fetchone()[0]
                    if pages * size < (self.max_mib - 8) * MIB * 0.8:
                        break
                    count = self.db.execute(f"DELETE FROM {name} WHERE id IN (SELECT id FROM {name} WHERE id != (SELECT max(id) FROM {name}) ORDER BY at_ms LIMIT 128)").rowcount
                    deleted += count
                    if count == 0:
                        break
            if deleted:
                self.put("capacity_evictions", self.get("capacity_evictions", 0) + deleted)
                self.put("last_capacity_eviction_ms", at)
        # Small incremental work and frequent checkpoints bound write amplification/WAL size.
        self.db.execute("PRAGMA incremental_vacuum(256)")
        self.db.execute("PRAGMA wal_checkpoint(TRUNCATE)")

    def query(self, *, resolution="raw", since=None, until=None, session=None, service=None, limit=100, before_id=None):
        if resolution not in {"raw", "minute", "transitions"} or not 1 <= limit <= 1000:
            raise ValueError("Resource history requires raw/minute/transitions and a limit between 1 and 1000")
        terms, parameters = [], []
        for column, op, value in (("at_ms", ">=", since), ("at_ms", "<=", until), ("session", "=", session), ("id", "<", before_id)):
            if value is not None:
                terms.append(column + op + "?")
                parameters.append(value)
        records = []
        where = " WHERE " + " AND ".join(terms) if terms else ""
        rows = self.db.execute(f"SELECT * FROM {resolution}{where} ORDER BY id DESC LIMIT ?", (*parameters, limit)).fetchall()
        for row in rows:
            value = unpack(row["payload"])
            value.update(id=row["id"], session_id=row["session"])
            if service and resolution != "transitions":
                value["items"] = [item for item in value["items"] if item["service"] == service]
            if resolution == "minute":
                for item in value["items"]:
                    for metric in item["metrics"].values():
                        metric["mean"] = metric["sum"] / metric["count"]
                for probe in value.get("probes", {}).values():
                    probe["mean_ms"] = probe["sum_ms"] / probe["count"]
            records.append(value)
        return dict(schema_version=SCHEMA, scope=self.get("scope"), node_id=self.get("node_id"), resolution=resolution,
                    capacity_evictions=self.get("capacity_evictions", 0), failed_samples=self.get("failed_samples", 0),
                    next_before_id=rows[-1]["id"] if len(rows) == limit else None, records=records)


def timestamp(value):
    if value is None:
        return None
    parsed = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if parsed.tzinfo is None:
        raise ValueError("Resource history timestamps require an explicit timezone")
    return int(parsed.timestamp() * 1000)


def add_parser(actions):
    parser = actions.add_parser("resources", help="Query/export local resource history without RPC")
    parser.add_argument("--resolution", choices=("raw", "minute", "transitions"), default="raw")
    parser.add_argument("--since", help="Timezone-qualified ISO timestamp")
    parser.add_argument("--until", help="Timezone-qualified ISO timestamp")
    parser.add_argument("--session", help="Monitor session ID")
    parser.add_argument("--service", help="host or Compose service, e.g. btc-node or balance-history")
    parser.add_argument("--limit", type=int, default=100)
    parser.add_argument("--before-id", type=int, help="Continue a previous page using next_before_id")
    parser.add_argument("--json", action="store_true", help="Export the complete bounded records as JSON")


def dispatch(args, layout):
    import node_monitor
    path = node_monitor.root(layout) / "resources.sqlite3"
    if not path.exists():
        raise ValueError("Resource history has not started; run the updated node monitor first")
    since, until = timestamp(args.since), timestamp(args.until)
    if since is not None and until is not None and since > until:
        raise ValueError("--since must not be later than --until")
    try:
        with History(path, node_monitor.scope(layout)) as history:
            result = history.query(resolution=args.resolution, since=since, until=until, session=args.session,
                                   service=args.service, limit=args.limit, before_id=args.before_id)
    except sqlite3.Error as error:
        raise ValueError(f"Resource history is unavailable ({getattr(error, 'sqlite_errorname', 'SQLITE_ERROR')}); preserve its files and inspect monitor logs") from None
    if args.json:
        print(json.dumps(result, indent=2, allow_nan=False))
        return
    print(f"Resource history | {args.resolution} | {len(result['records'])} records | capacity evictions={result['capacity_evictions']}")
    for value in result["records"]:
        stamp = datetime.fromtimestamp(value["at_ms"] / 1000, timezone.utc).isoformat()
        print(f"{stamp} | session={value['session_id']} | release={value.get('release_id')} | phase={value.get('configuration', {}).get('phase', '-')}")
        if args.resolution == "transitions":
            print(f"  {value['code']} | {value.get('from_phase')} -> {value.get('to_phase')} | operation={value.get('operation_id')}")
        for item in value.get("items", []):
            if args.resolution == "minute":
                print(f"  {item['service']}: {item['status_counts']} | samples={value['samples']} (min/max/mean in --json)")
            else:
                metrics = item["metrics"]
                def size(key):
                    number = metrics.get(key)
                    return "unknown" if number is None else f"{number / MIB:.0f} MiB"
                print(f"  {item['service']}: {item['status']} | memory={size('memory_current_bytes' if item['service'] != 'host' else 'memory_used_bytes')} | limit={size('memory_limit_bytes')} | swap={size('swap_used_bytes')} | memory PSI full={metrics.get('memory_psi_full_avg10', 'unknown')}%")
    if result["next_before_id"]:
        print(f"Next page: --before-id {result['next_before_id']} (keep the same filters)")
