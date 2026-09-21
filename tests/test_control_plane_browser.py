"""Opt-in browser regression; requires built web apps, Rust binary and Playwright."""
import json
import re
import time
from common.control_plane_web import console_server


def main():
    from playwright.sync_api import expect, sync_playwright

    with console_server() as (origin, token, snapshot, report):
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True)
            context = browser.new_context(viewport=dict(width=1365, height=1000))
            page = context.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(origin + "/?lang=zh-CN")
            expect(page.get_by_role("heading", name="USDB 私有控制台")).to_be_visible()
            page.get_by_label("访问令牌").fill("wrong")
            page.get_by_role("button", name="登录", exact=True).click()
            expect(page.get_by_role("alert")).to_contain_text("登录失败")
            page.get_by_label("访问令牌").fill(token)
            page.get_by_role("button", name="登录", exact=True).click()
            expect(page.get_by_role("heading", name="节点监控")).to_be_visible(timeout=15000)
            expect(page.get_by_role("status").first).to_contain_text("采集正常")
            expect(page.get_by_text("下载及 SHA-256 校验完成", exact=False)).to_be_visible()
            expect(page.get_by_text("Bitcoin 历史区块验证")).to_be_visible()
            assert token not in page.evaluate("JSON.stringify(localStorage) + JSON.stringify(sessionStorage) + document.cookie")
            assert all(cookie["httpOnly"] and cookie["sameSite"] == "Strict" for cookie in context.cookies())
            panel = page.get_by_role("region", name="矿工证铸造依赖", exact=True)
            snapshot.unlink()
            page.reload()
            expect(page.get_by_role("status").first).to_contain_text("暂无监控数据", timeout=15000)
            expect(panel.get_by_role("alert")).to_contain_text("不能据此判断 Ord 离线")
            # Release mode has one authoritative Ord panel, not a contradictory HTTP-only card.
            expect(page.get_by_role("heading", name="ord", exact=True)).to_have_count(0)
            for stage, label in [("DISABLED", "未启用"), ("WAITING_HISTORY", "等待 Bitcoin 历史区块验证"),
                                 ("WAITING_TXINDEX", "等待交易索引追平"), ("INDEXING", "Ord 索引及主链一致性校验中"),
                                 ("FAILED", "Ord 运行失败"), ("READY", "索引后端已就绪")]:
                report["observed_at_ms"] = int(time.time() * 1000)
                report["minting"] = dict(enabled=stage != "DISABLED", state=stage,
                    observed_at_ms=report["observed_at_ms"], core_height=100, history_height=100,
                    txindex_height=100, ord_height=100, ord_gap=0, canonical=stage == "READY",
                    txindex_synced=True, history_validated=True, backend_ready=stage == "READY",
                    transactions_enabled=False, disk_free_bytes=100 * 1024**3)
                if stage == "WAITING_TXINDEX":
                    report["minting"].update(txindex_height=84, txindex_synced=False, ord_height=None, ord_gap=None)
                snapshot.write_text(json.dumps(report))
                page.reload()
                expect(panel.get_by_role("status")).to_have_text(label, timeout=15000)
                expect(panel).to_contain_text("正式钱包签名和广播尚未开放")
                if stage == "WAITING_TXINDEX":
                    expect(panel).to_contain_text("Ord 本体及 HTTP 服务尚未启动")
                    page.goto(origin + "/?lang=zh-CN#/services/ord")
                    expect(panel.get_by_role("status")).to_have_text(label, timeout=15000)
                    expect(page.get_by_role("link", name=re.compile(r"^Ord 铭文索引")).get_by_text(label, exact=True)).to_be_visible()
                    page.set_viewport_size(dict(width=390, height=844))
                    assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth"), "Ord service layout overflow"
                    page.screenshot(path="/tmp/usdb-console-ord-waiting-mobile.png", full_page=True)
                    page.set_viewport_size(dict(width=1365, height=1000))
                    page.goto(origin + "/?lang=zh-CN#/overview")
            page.wait_for_function("async () => (await (await fetch('/api/system/overview')).json()).services.ord.data.query_ready === true")
            data = page.evaluate("fetch('/api/system/overview').then(r => r.json())")
            assert data["capabilities"]["btc_console_mode"] == "read_only"
            report["minting"]["observed_at_ms"] -= 65000
            snapshot.write_text(json.dumps(report))
            page.reload()
            expect(panel.get_by_role("status")).to_have_text("当前状态未知", timeout=15000)
            page.wait_for_function("async () => (await (await fetch('/api/system/overview')).json()).services.ord.data.query_ready === false")
            report["observed_at_ms"] -= 130000
            snapshot.write_text(json.dumps(report))
            page.reload()
            expect(page.get_by_role("status").first).to_contain_text("观测已过期", timeout=15000)
            expect(page.get_by_role("status").first).to_contain_text("当前状态未知")
            page.set_viewport_size(dict(width=390, height=844))
            assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth"), "Mobile layout overflow"
            page.get_by_role("button", name="退出登录").click()
            expect(page.get_by_role("heading", name="USDB 私有控制台")).to_be_visible()
            assert page.evaluate("fetch('/api/system/overview').then(r => r.status)") == 401
            assert not errors, errors
            browser.close()
        print("Private console browser regression passed: login, missing observer, Ord dependency states, service labels, unavailable RPC, independent progress, stale snapshot, mobile, logout")


if __name__ == "__main__":
    main()
