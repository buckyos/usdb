"""Opt-in console notification regression; built console/control-plane and Playwright required."""
import json
from pathlib import Path
import sys
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
from common.control_plane_web import console_server, default_report
from common.node_notifications import NotificationFixture, webhook_server
import node_monitor
import node_notification_config as config
from node_notifications import Worker


def main():
    from playwright.sync_api import expect, sync_playwright
    with NotificationFixture() as fixture, webhook_server() as (url, received):
        fixture.save([]); fixture.sync(0)
        process = Worker(fixture.directory, node_monitor.scope(fixture.layout))
        process.poll()
        report = default_report()
        report["monitor"] = node_monitor.summary(fixture.store, "running")
        try:
            with console_server(report, notification_directory=fixture.config_path.parent) as (origin, token, snapshot, report), sync_playwright() as playwright:
                browser = playwright.chromium.launch(headless=True)
                page = browser.new_page(viewport=dict(width=1365, height=1000))
                errors = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                page.goto(origin + "/?lang=zh-CN")
                page.get_by_label("访问令牌").fill(token)
                page.get_by_role("button", name="登录", exact=True).click()
                page.get_by_text("通知配置与投递", exact=True).click()
                page.get_by_role("button", name="添加 Webhook", exact=True).click()
                hook = page.get_by_role("group", name="Webhook", exact=True)
                hook.get_by_label("渠道名称", exact=True).fill("ops")
                hook.get_by_label("URL", exact=True).fill(url)
                hook.get_by_label("Bearer token（可选）").fill("fake-browser-secret")
                hook.get_by_label("允许明文 HTTP（仅可信网络）").check()
                page.get_by_role("button", name="保存配置", exact=True).click()
                expect(page.get_by_text("已保存；运行中的 monitor 会自动应用。")).to_be_visible()
                expect(hook.get_by_label("URL", exact=True)).to_have_value("")
                saved = config.read_json(fixture.config_path)
                assert saved["channels"][0]["url"] == url
                assert saved["channels"][0]["bearer_token"] == "fake-browser-secret"
                assert fixture.config_path.stat().st_mode & 0o777 == 0o600
                # Changing unrelated fields must preserve masked credentials.
                page.get_by_label("warning 通知间隔（秒）").fill("600")
                page.get_by_role("button", name="保存配置", exact=True).click()
                expect(page.get_by_role("button", name="保存配置", exact=True)).to_be_disabled()
                assert config.read_json(fixture.config_path)["channels"][0]["bearer_token"] == "fake-browser-secret"
                hook.get_by_role("button", name="发送测试通知", exact=True).click()
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not fixture.jobs("accepted"):
                    page.wait_for_timeout(100)
                assert len(received) == 1 and len(fixture.jobs("accepted")) == 1
                assert json.loads(received[0][1])["kind"] == "test"
                report["observed_at_ms"] = int(time.time() * 1000)
                report["monitor"]["notifications"] = config.read_json(fixture.directory / "delivery-status.json")
                snapshot.write_text(json.dumps(report))
                page.reload()
                page.get_by_text("通知配置与投递", exact=True).click()
                expect(page.get_by_text("接收端已接受", exact=True)).to_be_visible()
                page.get_by_role("button", name="添加 SMTP 邮件", exact=True).click()
                mail = page.get_by_role("group", name="SMTP 邮件", exact=True)
                mail.get_by_label("渠道名称", exact=True).fill("mail")
                mail.get_by_label("SMTP 服务器", exact=True).fill("smtp.example.invalid")
                mail.get_by_label("用户名（可选）").fill("node")
                mail.get_by_label("密码 / 邮箱授权码").fill("fake-mail-secret")
                mail.get_by_label("发件邮箱", exact=True).fill("node@example.invalid")
                mail.get_by_label("收件邮箱（每行一个）").fill("one@example.invalid\ntwo@example.invalid")
                mail.get_by_label("启用", exact=True).uncheck()
                page.get_by_role("button", name="保存配置", exact=True).click()
                expect(page.get_by_role("button", name="保存配置", exact=True)).to_be_disabled()
                saved = config.read_json(fixture.config_path)
                assert saved["channels"][1]["recipients"] == ["one@example.invalid", "two@example.invalid"]
                assert saved["channels"][1]["password"] == "fake-mail-secret"
                assert not saved["channels"][1]["enabled"]
                expect(mail.get_by_label("密码 / 邮箱授权码")).to_have_value("")
                page.set_viewport_size(dict(width=390, height=844))
                assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth"), "Notification form overflows mobile viewport"
                page.screenshot(path="/tmp/usdb-notifications-mobile.png", full_page=True)
                page.set_viewport_size(dict(width=1365, height=1000))
                page.screenshot(path="/tmp/usdb-notifications-desktop.png", full_page=True)
                assert not errors, errors
                browser.close()
        finally:
            process.close()
    print("Notification browser regression passed: authenticated save, masked credentials, live test delivery, SMTP form, mobile layout")


if __name__ == "__main__":
    main()
