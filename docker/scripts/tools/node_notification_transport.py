"""Bounded notification adapters. Only stable error codes leave the sender process."""
from __future__ import annotations

from datetime import datetime, timezone
from email.message import EmailMessage
from email.utils import format_datetime, parsedate_to_datetime
import hashlib
import hmac
import http.client
import json
import smtplib
import ssl
import time
from urllib.error import HTTPError, URLError
from urllib.request import HTTPRedirectHandler, HTTPSHandler, ProxyHandler, Request, build_opener


class NoRedirect(HTTPRedirectHandler):
    def redirect_request(self, *_args, **_kwargs):
        return None


def retry_after(value):
    try:
        seconds = int(value) if str(value).isdigit() else int(parsedate_to_datetime(value).timestamp() - time.time())
        return min(86400, max(0, seconds))
    except (ValueError, TypeError, OverflowError):
        return 0


def webhook(channel, payload):
    body = json.dumps(payload, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    headers = {"Content-Type": "application/json", "User-Agent": "usdb-node-monitor/1",
               "X-USDB-Notification-ID": payload["notification_id"]}
    if channel["bearer_token"]:
        headers["Authorization"] = "Bearer " + channel["bearer_token"]
    if channel["signing_secret"]:
        timestamp = str(int(time.time()))
        signature = hmac.new(channel["signing_secret"].encode(), timestamp.encode() + b"." + body, hashlib.sha256).hexdigest()
        headers.update({"X-USDB-Timestamp": timestamp, "X-USDB-Signature": "sha256=" + signature})
    # Do not inherit ambient proxy credentials or redirect secrets to another host.
    opener = build_opener(ProxyHandler({}), NoRedirect(), HTTPSHandler(context=ssl.create_default_context()))
    try:
        with opener.open(Request(channel["url"], data=body, headers=headers), timeout=10) as response:
            code = response.status
    except HTTPError as error:
        code = error.code
        delay = retry_after(error.headers.get("Retry-After"))
        error.close()
        return dict(state="retry" if code in (408, 425, 429) or code >= 500 else "failed", code=f"HTTP_{code}", retry_after_secs=delay)
    return dict(state="accepted" if 200 <= code < 300 else "failed", code=f"HTTP_{code}")


def smtp(channel, payload, recipient):
    message = EmailMessage()
    message["From"] = channel["sender"]
    message["To"] = recipient
    message["Subject"] = f"[USDB][{payload['severity'].upper()}][{payload['kind']}] {payload['service']}: {payload['code']}"
    message["Message-ID"] = f"<{payload['notification_id']}@usdb-node.local>"
    message["Date"] = format_datetime(datetime.fromtimestamp(payload["at_ms"] / 1000, timezone.utc))
    message.set_content(json.dumps(payload, ensure_ascii=False, indent=2))
    context = ssl.create_default_context()
    client = (smtplib.SMTP_SSL(channel["host"], channel["port"], timeout=10, context=context)
              if channel["tls"] == "tls" else smtplib.SMTP(channel["host"], channel["port"], timeout=10))
    try:
        if channel["tls"] == "starttls":
            client.ehlo()
            client.starttls(context=context)
            client.ehlo()
        if channel["username"]:
            client.login(channel["username"], channel["password"])
        # One envelope recipient per task keeps partial acceptance unambiguous.
        refused = client.send_message(message, from_addr=channel["sender"], to_addrs=[recipient])
        if refused:
            code = next(iter(refused.values()))[0]
            return dict(state="retry" if 400 <= code < 500 else "failed", code=f"SMTP_{code}")
        return dict(state="accepted", code="SMTP_ACCEPTED")
    finally:
        # A QUIT failure after DATA acceptance must not trigger another delivery.
        client.close()


def send(channel, payload, recipient=None):
    try:
        return webhook(channel, payload) if channel["type"] == "webhook" else smtp(channel, payload, recipient)
    except smtplib.SMTPRecipientsRefused as error:
        codes = [value[0] for value in error.recipients.values()]
        code = codes[0] if codes else 550
        return dict(state="retry" if 400 <= code < 500 else "failed", code=f"SMTP_{code}")
    except smtplib.SMTPResponseException as error:
        return dict(state="retry" if 400 <= error.smtp_code < 500 else "failed", code=f"SMTP_{error.smtp_code}")
    except (ssl.SSLCertVerificationError, smtplib.SMTPNotSupportedError):
        return dict(state="failed", code="TLS_OR_AUTH_UNSUPPORTED")
    except (OSError, URLError, smtplib.SMTPException, http.client.HTTPException):
        return dict(state="retry", code="CONNECTION_FAILED")
    except (ValueError, UnicodeError):
        return dict(state="failed", code="INVALID_MESSAGE")
