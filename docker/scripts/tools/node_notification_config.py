"""Small, shared-file notification policy; credentials never enter public projections."""
from __future__ import annotations

import copy
import json
import os
from pathlib import Path
import re
import tempfile
from urllib.parse import urlsplit

from node_monitor_store import private_directory, private_file

SCHEMA = "usdb-notifications:v1"
DEFAULTS = dict(schema_version=SCHEMA, warning_interval_secs=1800, critical_interval_secs=300,
                notify_recovery=True, channels=[])
MAX_BYTES = 65536


def atomic_write(path, value):
    """Publish complete private JSON, including directory durability after rename."""
    private_directory(path.parent)
    if path.exists() or path.is_symlink():
        private_file(path)
    fd, name = tempfile.mkstemp(prefix=".notify-", dir=path.parent)
    try:
        with os.fdopen(fd, "w") as stream:
            json.dump(value, stream, ensure_ascii=True, allow_nan=False)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(name, path)
        directory = os.open(path.parent, os.O_RDONLY | os.O_DIRECTORY)
        try:
            os.fsync(directory)
        finally:
            os.close(directory)
    finally:
        Path(name).unlink(missing_ok=True)


def read_json(path):
    private_directory(path.parent)
    private_file(path)
    with path.open("rb") as stream:
        raw = stream.read(MAX_BYTES + 1)
    if len(raw) > MAX_BYTES:
        raise ValueError("Notification file exceeds 64 KiB")
    return json.loads(raw)


def prepare(directory):
    private_directory(directory.parent, create=True)
    private_directory(directory, create=True)
    path = directory / "config.json"
    if not path.exists() and not path.is_symlink():
        # Exclusive creation avoids replacing a console save during first startup.
        try:
            fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            return
        with os.fdopen(fd, "w") as stream:
            json.dump(DEFAULTS, stream)
            stream.flush()
            os.fsync(stream.fileno())


def validate(raw):
    """Strict fields and bounded values; errors name fields, never their contents."""
    if not isinstance(raw, dict) or raw.keys() - DEFAULTS.keys():
        raise ValueError("Unknown notification configuration fields")
    result = {**copy.deepcopy(DEFAULTS), **raw}
    if result["schema_version"] != SCHEMA:
        raise ValueError("Unsupported notification configuration version")
    for key in ("warning_interval_secs", "critical_interval_secs"):
        if type(result[key]) is not int or not 60 <= result[key] <= 604800:
            raise ValueError(f"{key} must be an integer between 60 and 604800")
    if type(result["notify_recovery"]) is not bool:
        raise ValueError("notify_recovery must be boolean")
    if not isinstance(result["channels"], list) or len(result["channels"]) > 16:
        raise ValueError("At most 16 notification channels are supported")
    identities = set()
    normalized = []
    for raw_channel in result["channels"]:
        if not isinstance(raw_channel, dict):
            raise ValueError("Channel must be an object")
        kind = raw_channel.get("type")
        common = dict(enabled=True, min_severity="warning")
        if kind == "webhook":
            channel = dict(**common, url="", bearer_token="", signing_secret="", allow_http=False)
        elif kind == "smtp":
            channel = dict(**common, host="", port=587, tls="starttls", username="", password="", sender="", recipients=[])
        else:
            raise ValueError("Channel type must be webhook or smtp")
        if raw_channel.keys() - (channel.keys() | {"id", "type"}):
            raise ValueError("Unknown channel fields")
        channel.update(raw_channel)
        identity = channel.get("id")
        if not isinstance(identity, str) or not re.fullmatch(r"[A-Za-z0-9_-]{1,64}", identity) or identity in identities:
            raise ValueError("Channel IDs must be unique and contain 1-64 letters, digits, underscores or hyphens")
        identities.add(identity)
        if type(channel["enabled"]) is not bool or channel["min_severity"] not in ("warning", "critical"):
            raise ValueError("Invalid channel enabled or min_severity")
        for key, value in channel.items():
            if key not in ("recipients", "port", "enabled", "allow_http"):
                if not isinstance(value, str) or len(value.encode()) > 4096 or any(ord(c) < 32 or ord(c) == 127 for c in value):
                    raise ValueError(f"Invalid channel {key}")
        if kind == "webhook":
            if type(channel["allow_http"]) is not bool:
                raise ValueError("allow_http must be boolean")
            url = urlsplit(channel["url"])
            if (url.scheme not in (("https", "http") if channel["allow_http"] else ("https",))
                    or not url.hostname or url.username is not None or url.password is not None or url.fragment):
                raise ValueError("Webhook requires an HTTPS URL without userinfo or fragment; HTTP requires allow_http")
            try:
                url.port
            except ValueError:
                raise ValueError("Invalid webhook port") from None
        else:
            if (not re.fullmatch(r"[A-Za-z0-9_.:-]{1,253}", channel["host"])
                    or type(channel["port"]) is not int or not 1 <= channel["port"] <= 65535
                    or channel["tls"] not in ("tls", "starttls")):
                raise ValueError("Invalid SMTP host, port or TLS mode")
            recipients = channel["recipients"]
            if (not isinstance(recipients, list) or not 1 <= len(recipients) <= 16
                    or len(set(v for v in recipients if isinstance(v, str))) != len(recipients)):
                raise ValueError("SMTP requires 1-16 distinct recipients")
            for address in [channel["sender"], *recipients]:
                if not isinstance(address, str) or not re.fullmatch(r"[A-Za-z0-9.!#$%&'*+/=?^_`{|}~-]+@[A-Za-z0-9.-]+", address) or len(address) > 254:
                    raise ValueError("SMTP addresses must be plain ASCII mailbox addresses")
        normalized.append(channel)
    result["channels"] = normalized
    return result
