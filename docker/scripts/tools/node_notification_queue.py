"""Durable delivery jobs and simple per-alert reminder clocks, separate from events."""
from __future__ import annotations

import hashlib
import json
import os
import sqlite3
import uuid

from node_monitor_store import encode, private_directory, private_file, StateError
from node_notification_config import DEFAULTS, read_json, validate

MAX_PENDING = 4096
MAX_AGE_MS = 72 * 3600000
RANK = {"info": 0, "warning": 1, "critical": 2}


def destination(channel):
    # Credential rotation can retry existing jobs; destination changes cannot redirect them.
    keys = ("type", "url") if channel["type"] == "webhook" else ("type", "host", "port", "tls", "sender", "recipients")
    return hashlib.sha256(encode({key: channel[key] for key in keys}).encode()).hexdigest()


class Queue:
    def __init__(self, path, node_id, *, start_seq=None):
        private_directory(path.parent, create=True)
        created = False
        if not path.exists() and not path.is_symlink():
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
            os.close(fd)
            created = True
        for item in (path, path.with_name(path.name + "-wal"), path.with_name(path.name + "-shm"), path.with_name(path.name + "-journal")):
            if item.exists() or item.is_symlink():
                private_file(item)
        self.db = sqlite3.connect(path.as_uri() + "?mode=rw", uri=True, timeout=1)
        self.db.row_factory = sqlite3.Row
        try:
            version = self.db.execute("PRAGMA user_version").fetchone()[0]
            if created:
                self.db.execute("PRAGMA auto_vacuum=INCREMENTAL")
                self.db.executescript("""
                    BEGIN IMMEDIATE;
                    CREATE TABLE metadata(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                    CREATE TABLE schedules(alert_id TEXT PRIMARY KEY, severity TEXT, at_ms INTEGER, value TEXT);
                    CREATE TABLE deliveries(id TEXT PRIMARY KEY, channel TEXT, target TEXT, recipient TEXT,
                        alert_id TEXT, kind TEXT, severity TEXT, created_ms INTEGER, next_ms INTEGER,
                        finished_ms INTEGER, attempts INTEGER DEFAULT 0, state TEXT, code TEXT, payload TEXT);
                    CREATE INDEX delivery_due ON deliveries(state,next_ms);
                    CREATE INDEX delivery_alert ON deliveries(alert_id,channel,state);
                    PRAGMA user_version=1;
                """)
                with self.db:
                    self.put("node_id", node_id)
                    if start_seq is not None:
                        self.put("cursor", start_seq)
            elif version != 1:
                raise StateError("NOTIFICATION_DATABASE_VERSION_UNSUPPORTED", "Unsupported notification database version")
            if self.get("node_id") != node_id:
                raise StateError("NOTIFICATION_DATABASE_NODE_MISMATCH", "Notification database belongs to a different node")
            self.db.execute("PRAGMA journal_mode=WAL")
            self.db.execute("PRAGMA synchronous=FULL")
        except BaseException:
            self.db.close()
            raise
        self.config = self.get("config", DEFAULTS)
        self.config_error = None

    def close(self):
        self.db.close()

    def get(self, key, default=None):
        row = self.db.execute("SELECT value FROM metadata WHERE key=?", (key,)).fetchone()
        return json.loads(row[0]) if row else default

    def put(self, key, value):
        self.db.execute("INSERT OR REPLACE INTO metadata VALUES (?,?)", (key, encode(value)))

    def reload(self, path, seq):
        """Invalid edits leave the last valid configuration effective, even after restart."""
        try:
            settings = validate(read_json(path))
        except (OSError, ValueError, TypeError):
            self.config_error = "CONFIG_INVALID"
            return
        self.config_error = None
        if settings == self.config and self.get("config") is not None:
            return
        previous = {v["id"]: v for v in self.config["channels"] if v["enabled"]}
        cutoffs = self.get("channel_since", {})
        channels = {v["id"]: v for v in settings["channels"] if v["enabled"]}
        for identity, channel in channels.items():
            if identity not in previous or destination(previous[identity]) != destination(channel):
                cutoffs[identity] = seq
        self.config = settings
        self.put("config", settings)
        self.put("channel_since", {key: cutoffs.get(key, seq) for key in channels})
        for job in self.db.execute("SELECT id,channel,target FROM deliveries WHERE state='pending'").fetchall():
            channel = channels.get(job["channel"])
            if channel is None or destination(channel) != job["target"]:
                self.db.execute("UPDATE deliveries SET state='cancelled',code='CHANNEL_CHANGED' WHERE id=?", (job["id"],))

    def enqueue(self, value, kind, at, *, event_id=None, seq=None, only_channel=None, test_id=None):
        cutoffs = self.get("channel_since", {})
        for channel in self.config["channels"]:
            if not channel["enabled"] or (only_channel and channel["id"] != only_channel):
                continue
            if kind != "test" and RANK[value["severity"]] < RANK[channel["min_severity"]]:
                continue
            if seq is not None and seq <= cutoffs.get(channel["id"], seq):
                continue
            target = destination(channel)
            if kind == "resolved":
                routed = self.db.execute("SELECT 1 FROM deliveries WHERE alert_id=? AND channel=? AND target=? LIMIT 1",
                                         (value["alert_id"], channel["id"], target)).fetchone()
                if not routed:
                    continue
            for recipient in channel.get("recipients", [""]):
                pending = self.db.execute("SELECT id FROM deliveries WHERE state='pending' AND alert_id=? AND channel=? AND recipient=?",
                                          (value["alert_id"], channel["id"], recipient)).fetchone()
                if pending:
                    continue
                count = self.db.execute("SELECT count(*) FROM deliveries WHERE state='pending'").fetchone()[0]
                if count >= MAX_PENDING:
                    self.put("overflow_count", self.get("overflow_count", 0) + 1)
                    continue
                identity = uuid.uuid4().hex
                payload = dict(schema_version="usdb-node-notification:v1", notification_id=identity,
                               node_id=self.get("node_id"), network=self.get("network"), kind=kind,
                               at_ms=at, alert_id=value["alert_id"], service=value["service"], code=value["code"],
                               severity=value["severity"], event_id=event_id, first_seen_ms=value.get("first_seen_ms"),
                               last_seen_ms=value.get("last_seen_ms"), condition=value.get("condition"), evidence=value.get("evidence", {}))
                if test_id:
                    payload["test_id"] = test_id
                self.db.execute("""INSERT INTO deliveries(id,channel,target,recipient,alert_id,kind,severity,
                    created_ms,next_ms,state,payload) VALUES (?,?,?,?,?,?,?,?,?,'pending',?)""",
                    (identity, channel["id"], target, recipient, value["alert_id"], kind, value["severity"], at, at, encode(payload)))

    def track(self, value, at):
        self.db.execute("INSERT OR REPLACE INTO schedules VALUES (?,?,?,?)",
                        (value["alert_id"], value["severity"], at, encode(value)))

    def cancel(self, identity, reason, at):
        self.db.execute("UPDATE deliveries SET state='cancelled',code=?,finished_ms=? WHERE alert_id=? AND state='pending'",
                        (reason, at, identity))

    def sync(self, source, config_path, at):
        """Advance event cursor and generated tasks atomically; no network IO in transactions."""
        source.db.execute("BEGIN")
        try:
            highest = source.db.execute("SELECT COALESCE(max(seq),0) FROM events").fetchone()[0]
            cursor = self.get("cursor")
            events = source.db.execute("SELECT * FROM events WHERE seq>? ORDER BY seq LIMIT 500", (highest if cursor is None else cursor,)).fetchall()
            active = {v["alert_id"]: v for v in source.alerts() if v["state"] == "firing"}
            pruned = source.get("pruned_through", 0)
            network = source.get("scope")
        finally:
            source.db.rollback()
        with self.db:
            self.reload(config_path, cursor if cursor is not None and self.get("config") is None else highest)
            self.put("network", network)
            if cursor is not None and cursor < pruned:
                self.put("history_gap", True)
            if cursor is None:
                self.put("cursor", highest)
            # Current source state wins even while a large event backlog is draining.
            for row in self.db.execute("SELECT DISTINCT alert_id FROM deliveries WHERE state='pending' AND kind NOT IN ('resolved','test')").fetchall():
                if row[0] not in active:
                    self.cancel(row[0], "RECOVERED", at)
            for row in events:
                evidence = json.loads(row["evidence"])
                tracked = self.db.execute("SELECT value FROM schedules WHERE alert_id=?", (row["alert_id"],)).fetchone()
                value = active.get(row["alert_id"]) or (json.loads(tracked[0]) if tracked else dict(
                    alert_id=row["alert_id"], service=row["service"], code=evidence.get("code", row["code"]),
                    severity=row["severity"], first_seen_ms=evidence.get("first_seen_ms"), evidence=evidence))
                if row["code"] in ("ALERT_FIRING", "ALERT_ESCALATED"):
                    # Already recovered while offline: do not page for a stale problem.
                    if row["alert_id"] in active:
                        value = {**value, "severity": row["severity"]}
                        self.cancel(row["alert_id"], "SUPERSEDED", at)
                        if at - row["at_ms"] < MAX_AGE_MS:
                            self.enqueue(value, "firing" if row["code"] == "ALERT_FIRING" else "escalated",
                                         row["at_ms"], event_id=row["event_id"], seq=row["seq"])
                        self.track(value, row["at_ms"])
                elif row["code"] == "ALERT_RESOLVED":
                    self.cancel(row["alert_id"], "RECOVERED", at)
                    if self.config["notify_recovery"] and tracked and at - row["at_ms"] < MAX_AGE_MS:
                        self.enqueue({**value, "condition": "good"}, "resolved", row["at_ms"], event_id=row["event_id"], seq=row["seq"])
                    self.db.execute("DELETE FROM schedules WHERE alert_id=?", (row["alert_id"],))
                self.put("cursor", row["seq"])
            # Finish historical transitions before scheduling current-state reminders.
            if len(events) < 500:
                for value in active.values():
                    row = self.db.execute("SELECT severity,at_ms FROM schedules WHERE alert_id=?", (value["alert_id"],)).fetchone()
                    if row is None:
                        self.track(value, at)
                    elif at - row["at_ms"] >= self.config[value["severity"] + "_interval_secs"] * 1000:
                        self.enqueue(value, "ongoing", at)
                        self.track(value, at)
                for row in self.db.execute("SELECT alert_id FROM schedules").fetchall():
                    if row[0] not in active:
                        self.cancel(row[0], "NO_LONGER_ACTIVE", at)
                        self.db.execute("DELETE FROM schedules WHERE alert_id=?", (row[0],))
            self.db.execute("UPDATE deliveries SET state='expired',code='RETRY_WINDOW_EXPIRED',finished_ms=? WHERE state='pending' AND created_ms<?", (at, at - MAX_AGE_MS))
            self.db.execute("DELETE FROM deliveries WHERE state!='pending' AND (created_ms<? OR id IN (SELECT id FROM deliveries ORDER BY created_ms DESC LIMIT -1 OFFSET 20000))", (at - 30 * 86400000,))

    def test_request(self, path, at):
        if not path.exists() and not path.is_symlink():
            return
        try:
            request = read_json(path)
            if (not isinstance(request, dict) or set(request) != {"id", "channel", "at_ms"}
                    or not isinstance(request["id"], str) or len(request["id"]) != 32
                    or type(request["at_ms"]) is not int or not -5000 <= at - request["at_ms"] < 300000):
                raise ValueError("Invalid or expired test request")
            with self.db:
                if request["id"] != self.get("last_test_id"):
                    self.enqueue(dict(alert_id="test:" + request["id"], service="monitor", code="NOTIFICATION_TEST", severity="info"),
                                 "test", at, only_channel=request["channel"], test_id=request["id"])
                    self.put("last_test_id", request["id"])
        except (ValueError, OSError, TypeError):
            self.config_error = "TEST_REQUEST_INVALID"
        finally:
            path.unlink(missing_ok=True)

    def due(self, at, busy):
        rows = self.db.execute("SELECT * FROM deliveries WHERE state='pending' AND next_ms<=? ORDER BY next_ms,created_ms", (at,)).fetchall()
        channels = {v["id"]: v for v in self.config["channels"] if v["enabled"]}
        for row in rows:
            if row["channel"] not in busy and row["channel"] in channels and at >= self.get("backoff:" + row["channel"], 0):
                yield dict(row), channels[row["channel"]]

    def result(self, job, result, at):
        """Acceptance is transport acceptance, never proof a person read the message."""
        attempt = job["attempts"] + 1
        state = result.get("state", "retry")
        code = result.get("code", "SENDER_FAILED")
        delay = max(min(1800, 30 * 2 ** min(attempt - 1, 6)), min(86400, result.get("retry_after_secs", 0)))
        with self.db:
            self.db.execute("""UPDATE deliveries SET attempts=?,state=?,code=?,next_ms=?,finished_ms=?
                WHERE id=? AND state='pending'""", (attempt, "pending" if state == "retry" else state, code,
                    at + delay * 1000, None if state == "retry" else at, job["id"]))
            if state == "retry":
                self.put("backoff:" + job["channel"], at + delay * 1000)
            elif state == "accepted":
                self.put("backoff:" + job["channel"], 0)

    def summary(self, at):
        counts = {row[0]: row[1] for row in self.db.execute("SELECT state,count(*) FROM deliveries GROUP BY state")}
        channels = []
        for channel in self.config["channels"]:
            recent = self.db.execute("SELECT id,state,code,attempts,created_ms,next_ms,finished_ms FROM deliveries WHERE channel=? ORDER BY created_ms DESC,rowid DESC LIMIT 1", (channel["id"],)).fetchone()
            success = self.db.execute("SELECT max(finished_ms) FROM deliveries WHERE channel=? AND state='accepted'", (channel["id"],)).fetchone()[0]
            # A successful recipient must not conceal another recipient's failure.
            failures = self.db.execute("""SELECT code FROM deliveries WHERE channel=? AND
                ((state='pending' AND attempts>0) OR (created_ms=? AND state IN ('failed','expired')))
                ORDER BY created_ms DESC,rowid DESC""", (channel["id"], recent["created_ms"] if recent else 0)).fetchall()
            pending = self.db.execute("SELECT count(*) FROM deliveries WHERE channel=? AND state='pending'", (channel["id"],)).fetchone()[0]
            channels.append(dict(id=channel["id"], type=channel["type"], enabled=channel["enabled"], last_success_ms=success,
                                 pending_count=pending, failed_count=len(failures), last_error=failures[0][0] if failures else None,
                                 latest=dict(recent) if recent else None))
        failed = any(v["enabled"] and v["failed_count"] for v in channels)
        return dict(state="degraded" if self.config_error or failed else "running", updated_at_ms=at,
                    config_error=self.config_error, history_gap=self.get("history_gap", False),
                    overflow_count=self.get("overflow_count", 0), counts=counts, channels=channels)
