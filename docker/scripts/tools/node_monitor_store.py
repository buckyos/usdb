"""Private, transactional node events and alert lifecycles, independent of delivery."""
from __future__ import annotations

import json
import os
from pathlib import Path
import sqlite3
import stat
import uuid

VERSION = 1


class StateError(ValueError):
    """Stable diagnostics that are safe to log without private database payloads."""

    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


def encode(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), allow_nan=False)


def private_directory(path: Path, *, create=False):
    """Reject redirected or shared monitor state before SQLite opens sidecar files."""
    if create:
        path.mkdir(mode=0o700, parents=True, exist_ok=True)
    info = path.lstat()
    if not stat.S_ISDIR(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise StateError("MONITOR_DIRECTORY_UNSAFE", "Monitor state directory must be operator-owned and private (0700)")


def private_file(path: Path):
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077 or info.st_nlink != 1:
        raise StateError("MONITOR_FILE_UNSAFE", "Monitor state file must be an operator-owned private regular file")


class Store:
    """One versioned event database per node/network; readers never initialize it."""

    def __init__(self, path: Path, scope: dict, *, writable=False):
        private_directory(path.parent, create=writable)
        created = False
        if writable and not path.exists() and not path.is_symlink():
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            os.close(fd)
            created = True
        private_file(path)
        for item in (path, Path(str(path) + "-wal"), Path(str(path) + "-shm"), Path(str(path) + "-journal")):
            if item.exists() or item.is_symlink():
                private_file(item)
        self.db = sqlite3.connect(path.as_uri() + ("?mode=rw" if writable else "?mode=ro"), uri=True, timeout=3)
        self.db.row_factory = sqlite3.Row
        try:
            version = self.db.execute("PRAGMA user_version").fetchone()[0]
            tables = self.db.execute("SELECT name FROM sqlite_master WHERE type='table'").fetchall()
            if created and version == 0 and not tables:
                self.db.execute("PRAGMA auto_vacuum=INCREMENTAL")
                self.db.executescript("""
                    BEGIN IMMEDIATE;
                    CREATE TABLE metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                    CREATE TABLE events (
                        seq INTEGER PRIMARY KEY AUTOINCREMENT, event_id TEXT UNIQUE NOT NULL,
                        at_ms INTEGER NOT NULL, service TEXT NOT NULL, code TEXT NOT NULL,
                        severity TEXT NOT NULL, alert_id TEXT, evidence TEXT NOT NULL);
                    CREATE INDEX events_time ON events(at_ms);
                    CREATE INDEX events_alert ON events(alert_id);
                    CREATE TABLE alerts (key TEXT PRIMARY KEY, alert_id TEXT NOT NULL UNIQUE, value TEXT NOT NULL);
                    CREATE TABLE baselines (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                    PRAGMA user_version=1;
                """)
                with self.db:
                    self.put("scope", scope)
                    self.put("node_id", uuid.uuid4().hex)
            elif version != VERSION:
                raise StateError("MONITOR_DATABASE_VERSION_UNSUPPORTED", "Unsupported monitor database version; preserve it and use a compatible release")
            if self.get("scope") != scope:
                raise StateError("MONITOR_DATABASE_NETWORK_MISMATCH", "Monitor database belongs to a different node network; refusing to reuse it")
            if writable:
                self.db.execute("PRAGMA journal_mode=WAL")
                self.db.execute("PRAGMA synchronous=FULL")
            else:
                self.db.execute("PRAGMA query_only=ON")
        except BaseException:
            self.db.close()
            raise

    def close(self):
        self.db.close()

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()

    def get(self, key, default=None):
        row = self.db.execute("SELECT value FROM metadata WHERE key=?", (key,)).fetchone()
        return json.loads(row[0]) if row else default

    def put(self, key, value):
        self.db.execute("INSERT OR REPLACE INTO metadata VALUES (?,?)", (key, encode(value)))

    def baseline(self, key, default=None):
        row = self.db.execute("SELECT value FROM baselines WHERE key=?", (key,)).fetchone()
        return json.loads(row[0]) if row else default

    def set_baseline(self, key, value):
        self.db.execute("INSERT OR REPLACE INTO baselines VALUES (?,?)", (key, encode(value)))

    def event(self, at, service, code, severity="info", *, alert_id=None, evidence=None):
        """Append only bounded, already allowlisted evidence from rules or lifecycle code."""
        payload = encode(evidence or {})
        if len(payload.encode()) > 8192:
            raise ValueError("Monitor event evidence exceeds its size limit")
        self.db.execute("INSERT INTO events(event_id,at_ms,service,code,severity,alert_id,evidence) VALUES (?,?,?,?,?,?,?)",
                        (uuid.uuid4().hex, at, service, code, severity, alert_id, payload))

    def alerts(self, *, include_resolved=False):
        values = [json.loads(row[0]) for row in self.db.execute("SELECT value FROM alerts ORDER BY rowid DESC")]
        return [v for v in values if include_resolved or v["state"] != "resolved"]

    def save_alert(self, key, value):
        self.db.execute("INSERT INTO alerts VALUES (?,?,?) ON CONFLICT(key) DO UPDATE SET alert_id=excluded.alert_id,value=excluded.value",
                        (key, value["alert_id"], encode(value)))

    def condition(self, key, service, code, bad, at, *, evidence=None, warning_ms=120000,
                  critical_ms=600000, recovery_ms=60000, latched=False, reset=False):
        """Unknown breaks consecutive windows; only explicit good evidence may recover."""
        row = self.db.execute("SELECT value FROM alerts WHERE key=?", (key,)).fetchone()
        value = json.loads(row[0]) if row else None
        if value is None or value["state"] == "resolved":
            if bad is not True:
                return
            value = dict(alert_id=uuid.uuid4().hex, service=service, code=code, state="pending",
                         severity="warning", first_seen_ms=at, last_seen_ms=at, occurrences=0,
                         bad_since_ms=None, good_since_ms=None, acknowledged_at_ms=None,
                         latched=latched, evidence={})
        if reset:
            value.update(bad_since_ms=None, good_since_ms=None)
        value["condition"] = "unknown" if bad is None else "bad" if bad else "good"
        value["observed_at_ms"] = at
        if bad is None:
            value.update(bad_since_ms=None, good_since_ms=None)
        elif bad:
            value["bad_since_ms"] = at if value["bad_since_ms"] is None else value["bad_since_ms"]
            value.update(good_since_ms=None, last_seen_ms=at, occurrences=value["occurrences"] + 1)
            previous = value["evidence"]
            value["evidence"] = evidence or {}
            duration = max(0, at - value["bad_since_ms"])
            if latched or duration >= warning_ms:
                severity = "critical" if latched or duration >= critical_ms else "warning"
                if value["state"] == "pending":
                    value.update(state="firing", severity=severity, fired_at_ms=at)
                    self.event(at, service, "ALERT_FIRING", severity, alert_id=value["alert_id"], evidence={"code": code, "first_seen_ms": value["first_seen_ms"], **value["evidence"]})
                elif severity == "critical" and value["severity"] != "critical":
                    value["severity"] = severity
                    self.event(at, service, "ALERT_ESCALATED", severity, alert_id=value["alert_id"], evidence={"code": code, **value["evidence"]})
                elif latched and previous != value["evidence"]:
                    self.event(at, service, "INCIDENT_EVIDENCE_CHANGED", value["severity"], alert_id=value["alert_id"], evidence=value["evidence"])
        else:
            value["bad_since_ms"] = None
            value["good_since_ms"] = at if value["good_since_ms"] is None else value["good_since_ms"]
            if not value["latched"] and (value["state"] == "pending" or at - value["good_since_ms"] >= recovery_ms):
                if value["state"] == "firing":
                    self.event(at, service, "ALERT_RESOLVED", alert_id=value["alert_id"], evidence={"code": code})
                value.update(state="resolved", resolved_at_ms=at)
        self.save_alert(key, value)

    def begin(self, at, release):
        """Start a fresh continuity window while retaining previously firing incidents."""
        with self.db:
            previous = self.get("session")
            if previous and not previous.get("stopped_at_ms"):
                self.event(at, "monitor", "PREVIOUS_SESSION_INTERRUPTED", "warning")
            session = dict(id=uuid.uuid4().hex, started_at_ms=at, release_id=release)
            self.put("session", session)
            self.put("last_sample_ms", None)
            self.db.execute("DELETE FROM baselines")
            for value in self.alerts():
                value.update(bad_since_ms=None, good_since_ms=None, condition="unknown")
                self.db.execute("UPDATE alerts SET value=? WHERE alert_id=?", (encode(value), value["alert_id"]))
            self.event(at, "monitor", "MONITOR_STARTED", evidence={"release_id": release, "session_id": session["id"]})

    def end(self, at, reason):
        with self.db:
            session = self.get("session", {})
            session.update(stopped_at_ms=at)
            self.put("session", session)
            self.event(at, "monitor", "MONITOR_STOPPED", evidence={"reason": reason})

    def acknowledge(self, identity, at):
        with self.db:
            row = self.db.execute("SELECT key,value FROM alerts WHERE alert_id=?", (identity,)).fetchone()
            if not row or (value := json.loads(row[1]))["state"] != "firing":
                raise ValueError("Only a firing alert can be acknowledged")
            if value["acknowledged_at_ms"] is None:
                value["acknowledged_at_ms"] = at
                self.save_alert(row[0], value)
                self.event(at, value["service"], "ALERT_ACKNOWLEDGED", alert_id=identity, evidence={"code": value["code"]})

    def resolve_incident(self, identity, at):
        """Called only after an explicit recovery confirmation and a fresh source check."""
        with self.db:
            row = self.db.execute("SELECT key,value FROM alerts WHERE alert_id=?", (identity,)).fetchone()
            if not row or not (value := json.loads(row[1]))["latched"] or value["state"] != "firing":
                raise ValueError("Only a firing latched incident can be manually resolved")
            value.update(state="resolved", resolved_at_ms=at, condition="good")
            self.save_alert(row[0], value)
            self.event(at, value["service"], "INCIDENT_MANUALLY_RESOLVED", alert_id=identity, evidence={"code": value["code"]})

    def events(self, *, limit=100, service=None, severity=None, since=None, identity=None):
        if not 1 <= limit <= 1000:
            raise ValueError("Event limit must be between 1 and 1000")
        terms, values = [], []
        for name, value, op in (("service", service, "="), ("severity", severity, "="),
                                ("at_ms", since, ">="), ("event_id", identity, "=")):
            if value is not None:
                terms.append(f"{name}{op}?")
                values.append(value)
        where = " WHERE " + " AND ".join(terms) if terms else ""
        rows = self.db.execute("SELECT * FROM events" + where + " ORDER BY seq DESC LIMIT ?", (*values, limit))
        context = dict(schema_version="usdb-node-event:v1", node_id=self.get("node_id"), network=self.get("scope"))
        return [{**context, **dict(row), "evidence": json.loads(row["evidence"])} for row in rows]

    def prune(self, at, *, days=90, count=20000):
        """Bound ordinary history; never discard events belonging to active alerts."""
        cutoff = at - days * 86400000
        with self.db:
            self.db.execute("""DELETE FROM events WHERE (at_ms < ? OR seq <=
                COALESCE((SELECT seq FROM events ORDER BY seq DESC LIMIT 1 OFFSET ?),0))
                AND (alert_id IS NULL OR alert_id NOT IN
                    (SELECT alert_id FROM alerts WHERE json_extract(value,'$.state') != 'resolved'))""", (cutoff, count))
            self.db.execute("""DELETE FROM alerts WHERE json_extract(value,'$.state')='resolved'
                AND json_extract(value,'$.resolved_at_ms') < ?""", (cutoff,))
        self.db.execute("PRAGMA wal_checkpoint(PASSIVE)")
        self.db.execute("PRAGMA incremental_vacuum(256)")
