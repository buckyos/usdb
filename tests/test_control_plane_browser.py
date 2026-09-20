"""Opt-in browser regression; requires built web apps, Rust binary and Playwright."""
import json
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
            expect(page.get_by_role("status")).to_contain_text("采集正常")
            expect(page.get_by_text("下载及 SHA-256 校验完成", exact=False)).to_be_visible()
            expect(page.get_by_text("Bitcoin 后台历史校验（不阻塞前台就绪）")).to_be_visible()
            assert token not in page.evaluate("JSON.stringify(localStorage) + JSON.stringify(sessionStorage) + document.cookie")
            assert all(cookie["httpOnly"] and cookie["sameSite"] == "Strict" for cookie in context.cookies())
            report["observed_at_ms"] -= 130000
            snapshot.write_text(json.dumps(report))
            page.reload()
            expect(page.get_by_role("status")).to_contain_text("数据已过期", timeout=15000)
            expect(page.get_by_role("status")).to_contain_text("当前状态未知")
            page.set_viewport_size(dict(width=390, height=844))
            assert page.evaluate("document.documentElement.scrollWidth <= window.innerWidth"), "Mobile layout overflow"
            page.get_by_role("button", name="退出登录").click()
            expect(page.get_by_role("heading", name="USDB 私有控制台")).to_be_visible()
            assert page.evaluate("fetch('/api/system/overview').then(r => r.status)") == 401
            assert not errors, errors
            browser.close()
        print("Private console browser regression passed: login, unavailable RPC, independent progress, stale snapshot, mobile, logout")


if __name__ == "__main__":
    main()
