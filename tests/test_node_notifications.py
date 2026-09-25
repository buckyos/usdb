"""Notification recovery, routing, retry isolation and transport contracts without live services."""
from pathlib import Path
import hashlib
import hmac
import json
import os
import sqlite3
import sys
import tempfile
import time
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_notification_config as config
import node_notification_transport as transport
import node_notifications as worker
from node_notification_queue import Queue, MAX_AGE_MS
import node_monitor
from common.node_monitor import BASE, incident
from common.node_notifications import NotificationFixture, hook, mail, webhook_server, smtp_server


class NotificationTests(unittest.TestCase):
    def test_firing_reminder_restart_and_automatic_recovery(self):
        with NotificationFixture() as f:
            f.tick(1, incidents=incident()); f.sync(1)
            first = f.jobs()[0]
            self.assertEqual(first["kind"], "firing")
            f.queue.result(first, dict(state="accepted", code="HTTP_200"), BASE + 1000)
            f.queue.close()
            f.queue = Queue(f.directory / "notifications.sqlite3", f.store.get("node_id"))
            f.sync(300)
            self.assertEqual(len(f.jobs()), 1)
            f.sync(301)
            self.assertEqual(f.jobs("pending")[0]["kind"], "ongoing")
            f.tick(302); f.tick(304); f.sync(304)
            self.assertEqual(f.jobs("pending")[0]["kind"], "resolved")
            self.assertTrue(any(v["code"] == "RECOVERED" for v in f.jobs()))
            f.sync(1000)
            self.assertEqual(len(f.jobs("pending")), 1)

    def test_new_channels_only_receive_future_notifications(self):
        with NotificationFixture() as f:
            f.tick(1, incidents=incident()); f.sync(1)
            f.save([hook(), hook("second")]); f.sync(2)
            self.assertEqual([v["channel"] for v in f.jobs()], ["hook"])
            f.sync(301)
            self.assertEqual([v["channel"] for v in f.jobs("pending")], ["hook", "second"])
            f.sync(1000)
            self.assertEqual(len(f.jobs("pending")), 2)

    def test_first_worker_captures_events_after_start_boundary(self):
        with NotificationFixture() as f:
            f.queue.close()
            path = f.directory / "notifications.sqlite3"
            for suffix in ("", "-wal", "-shm"):
                path.with_name(path.name + suffix).unlink(missing_ok=True)
            start = f.store.db.execute("SELECT max(seq) FROM events").fetchone()[0]
            f.tick(1, incidents=incident())
            f.queue = Queue(path, f.store.get("node_id"), start_seq=start)
            f.sync(2)
            self.assertEqual(f.jobs()[0]["kind"], "firing")

    def test_enabling_existing_alerts_does_not_catch_up(self):
        with NotificationFixture() as f:
            f.save([]); f.sync(1)
            f.tick(2, incidents=incident()); f.sync(2)
            f.save([hook("later")]); f.sync(3)
            self.assertEqual(f.jobs(), [])
            f.sync(302)
            self.assertEqual(f.jobs()[0]["channel"], "later")

    def test_retry_survives_restart_and_other_channels_still_run(self):
        with NotificationFixture() as f:
            f.save([hook(), hook("other")]); f.sync(0)
            f.tick(1, incidents=incident()); f.sync(1)
            first = f.jobs()[0]
            f.queue.result(first, dict(state="retry", code="HTTP_429", retry_after_secs=120), BASE + 1000)
            due = list(f.queue.due(BASE + 2000, set()))
            self.assertEqual([v[0]["channel"] for v in due], ["other"])
            f.queue.close(); f.queue = Queue(f.directory / "notifications.sqlite3", f.store.get("node_id"))
            self.assertEqual(len(list(f.queue.due(BASE + 121000, set()))), 2)
            self.assertEqual(f.jobs()[0]["attempts"], 1)

    def test_configuration_keeps_last_good_and_pins_destinations(self):
        with NotificationFixture() as f:
            f.tick(1, incidents=incident()); f.sync(1)
            f.config_path.write_text('{invalid')
            f.sync(2)
            self.assertEqual(f.queue.config_error, "CONFIG_INVALID")
            self.assertEqual(len(f.jobs("pending")), 1)
            f.queue.close(); f.queue = Queue(f.directory / "notifications.sqlite3", f.store.get("node_id"))
            f.sync(3)
            self.assertEqual(f.queue.config["channels"][0]["id"], "hook")
            f.save([dict(hook(), bearer_token="rotated")]); f.sync(4)
            self.assertEqual(len(f.jobs("pending")), 1)
            f.save([dict(hook(), url="https://elsewhere.invalid/")]); f.sync(5)
            self.assertEqual(f.jobs("pending"), [])
            self.assertEqual(f.jobs()[0]["code"], "CHANNEL_CHANGED")

    def test_severity_escalation_and_recovery_to_critical_only_channel(self):
        with NotificationFixture() as f:
            f.save([hook(min_severity="critical")]); f.sync(0)
            f.tick(0); f.tick(1, ready=False); f.tick(3, ready=False); f.sync(3)
            self.assertEqual(f.jobs(), [])
            f.tick(5, ready=False); f.sync(5)
            self.assertTrue(all(v["severity"] == "critical" for v in f.jobs()))
            f.tick(6); f.tick(8); f.sync(8)
            self.assertTrue(all(v["kind"] == "resolved" for v in f.jobs("pending")))

    def test_partial_smtp_delivery_retries_only_failed_recipient(self):
        with NotificationFixture() as f:
            f.save([mail()]); f.sync(0)
            f.tick(1, incidents=incident()); f.sync(1)
            first, second = f.jobs()
            f.queue.result(first, dict(state="accepted", code="SMTP_ACCEPTED"), BASE + 1000)
            f.queue.result(second, dict(state="retry", code="SMTP_450"), BASE + 1000)
            jobs = list(f.queue.due(BASE + 31000, set()))
            self.assertEqual([v[0]["recipient"] for v in jobs], ["two@example.invalid"])

    def test_later_smtp_success_does_not_hide_partial_failure(self):
        with NotificationFixture() as f:
            f.save([mail()]); f.sync(0)
            f.tick(1, incidents=incident()); f.sync(1)
            first, second = f.jobs()
            f.queue.result(first, dict(state="failed", code="SMTP_550"), BASE + 1000)
            f.queue.result(second, dict(state="accepted", code="SMTP_ACCEPTED"), BASE + 1001)
            summary = f.queue.summary(BASE + 1002)
            self.assertEqual(summary["state"], "degraded")
            self.assertEqual(summary["channels"][0]["last_error"], "SMTP_550")

    def test_outbox_and_cursor_rollback_together(self):
        with NotificationFixture() as f:
            f.tick(1, incidents=incident())
            cursor = f.queue.get("cursor")
            with mock.patch.object(f.queue, "track", side_effect=RuntimeError("crash")), self.assertRaises(RuntimeError):
                f.sync(1)
            self.assertEqual(f.queue.get("cursor"), cursor)
            self.assertEqual(f.jobs(), [])
            f.sync(2)
            self.assertEqual(len(f.jobs()), 1)

    def test_test_request_is_durable_and_not_repeated(self):
        with NotificationFixture() as f:
            path = f.config_path.parent / "test-request.json"
            value = dict(id="a" * 32, channel="hook", at_ms=BASE)
            config.atomic_write(path, value); f.queue.test_request(path, BASE)
            self.assertFalse(path.exists())
            config.atomic_write(path, value); f.queue.test_request(path, BASE)
            self.assertEqual(len(f.jobs()), 1)
            self.assertEqual(json.loads(f.jobs()[0]["payload"])["test_id"], "a" * 32)

    def test_corrupt_or_foreign_queue_does_not_modify_event_store(self):
        with NotificationFixture() as f:
            with self.assertRaisesRegex(ValueError, "different node"):
                Queue(f.directory / "notifications.sqlite3", "other")
            f.queue.db.execute("PRAGMA user_version=99")
            with self.assertRaisesRegex(ValueError, "Unsupported"):
                Queue(f.directory / "notifications.sqlite3", f.store.get("node_id"))
            f.tick(1, incidents=incident())
            self.assertIn("ALERT_FIRING", f.codes())

    def test_queue_bounds_expiration_and_history_gap_are_visible(self):
        with NotificationFixture() as f:
            with mock.patch("node_notification_queue.MAX_PENDING", 0):
                f.tick(1, incidents=incident()); f.sync(1)
            self.assertGreater(f.queue.summary(BASE)["overflow_count"], 0)
            f.sync(301)
            with f.store.db:
                f.store.put("pruned_through", f.queue.get("cursor") + 10)
            f.sync(302)
            self.assertTrue(f.queue.summary(BASE)["history_gap"])
            # Stop generating new reminders to inspect existing task expiration.
            f.queue.db.execute("DELETE FROM schedules"); f.queue.db.commit()
            f.sync(302 + MAX_AGE_MS / 1000)
            self.assertTrue(any(v["state"] == "expired" for v in f.jobs()))

    def test_prepare_creates_private_monitor_parent_before_first_up(self):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary) / "monitor" / "notifications"
            config.prepare(directory)
            self.assertEqual(directory.parent.stat().st_mode & 0o777, 0o700)
            self.assertEqual(directory.stat().st_mode & 0o777, 0o700)
            self.assertEqual((directory / "config.json").stat().st_mode & 0o777, 0o600)

    def test_strict_config_rejects_secrets_in_headers_and_wrong_types(self):
        cases = [dict(hook(), url="http://example.invalid"), dict(hook(), bearer_token="secret\r\ninjected"),
                 dict(mail(), recipients=["bad\naddress"]), dict(mail(), port=True), dict(hook(), extra=1)]
        for channel in cases:
            with self.subTest(channel=channel), self.assertRaises(ValueError):
                config.validate({"channels": [channel]})

    def test_webhook_real_http_signing_redirect_and_retry_after(self):
        payload = dict(notification_id="a" * 32, severity="critical", service="chain", code="HALTED", kind="firing", at_ms=BASE)
        with webhook_server() as (url, received):
            channel = config.validate({"channels": [dict(hook(), url=url, allow_http=True, signing_secret="key", bearer_token="token") ]})["channels"][0]
            self.assertEqual(transport.send(channel, payload)["state"], "accepted")
            headers, body = received[0]
            timestamp = headers["X-Usdb-Timestamp"]
            signature = hmac.new(b"key", timestamp.encode() + b"." + body, hashlib.sha256).hexdigest()
            self.assertEqual(headers["X-Usdb-Signature"], "sha256=" + signature)
            self.assertEqual(headers["Authorization"], "Bearer token")
        for code, expected in ((429, "retry"), (503, "retry"), (401, "failed"), (302, "failed")):
            with webhook_server(code, {"Retry-After": "120", "Location": "http://example.invalid"}) as (url, received):
                channel["url"] = url
                result = transport.send(channel, payload)
                self.assertEqual(result["state"], expected)
                self.assertEqual(result["retry_after_secs"], 120)
                self.assertEqual(len(received), 1)

    def test_real_smtp_tls_and_starttls_acceptance_and_refusal(self):
        payload = dict(notification_id="a" * 32, severity="critical", service="chain", code="HALTED", kind="firing", at_ms=BASE)
        for mode in ("tls", "starttls"):
            with self.subTest(mode=mode), smtp_server(mode) as (port, messages):
                channel = config.validate({"channels": [{**mail(), "host": "127.0.0.1", "port": port, "tls": mode}]})["channels"][0]
                self.assertEqual(transport.send(channel, payload, "one@example.invalid")["state"], "accepted")
                self.assertEqual(len(messages), 1)
                self.assertIn(b"HALTED", messages[0])
                self.assertEqual(transport.send(channel, payload, "refused@example.invalid"), dict(state="failed", code="SMTP_550"))
                self.assertEqual(len(messages), 1)

    def test_smtp_uses_tls_stable_message_id_and_single_recipient(self):
        channel = config.validate({"channels": [mail()]})["channels"][0]
        payload = dict(notification_id="a" * 32, severity="critical", service="chain", code="HALTED", kind="firing", at_ms=BASE)
        with mock.patch.object(transport.smtplib, "SMTP") as client:
            smtp = client.return_value
            smtp.send_message.return_value = {}
            self.assertEqual(transport.send(channel, payload, "one@example.invalid")["state"], "accepted")
            smtp.starttls.assert_called_once()
            smtp.login.assert_called_once_with("node", "private")
            args, kwargs = smtp.send_message.call_args
            self.assertEqual(kwargs["to_addrs"], ["one@example.invalid"])
            self.assertEqual(args[0]["Message-ID"], "<" + "a" * 32 + "@usdb-node.local>")
            smtp.close.assert_called_once()

    def test_real_worker_sends_and_restarts_without_external_services(self):
        with NotificationFixture() as f, webhook_server() as (url, received):
            f.save([dict(hook(), url=url, allow_http=True)])
            f.sync(0)
            process = worker.Worker(f.directory, node_monitor.scope(f.layout))
            try:
                process.poll()
                # Use wall-clock times for the real worker's expiry and due checks.
                at = worker.milliseconds()
                with f.store.db:
                    f.store.condition("fault", "chain", "TEST_FAULT", True, at, latched=True)
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline and not received:
                    time.sleep(0.1)
                self.assertEqual(len(received), 1)
                self.assertEqual(json.loads(received[0][1])["code"], "TEST_FAULT")
                while time.monotonic() < deadline and not f.jobs("accepted"):
                    time.sleep(0.1)
                self.assertEqual(len(f.jobs("accepted")), 1)
                process.close()
                process.next_start = 0
                process.poll()
                time.sleep(1.2)
                self.assertEqual(len(received), 1)
            finally:
                process.close()


if __name__ == "__main__":
    unittest.main()
