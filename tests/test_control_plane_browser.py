"""Opt-in browser regression against a built console and isolated Rust server.

Requires Playwright/Chromium, the debug usdb-control-plane binary and all three
web dist directories. No node, wallet, Docker daemon or real RPC is contacted.
"""

import json
from pathlib import Path
import socket
import subprocess
import tempfile
import time
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parents[1]


def main():
    # Keep ordinary unittest discovery independent of the optional browser runtime.
    from playwright.sync_api import expect, sync_playwright

    with tempfile.TemporaryDirectory(prefix="usdb-private-console-browser-") as temporary:
        root = Path(temporary)
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        roots = [ROOT / "web" / name / "dist" for name in ("usdb-console-app", "balance-history-browser", "usdb-indexer-browser")]
        assert all(path.is_dir() for path in roots), "Build all console web apps first"
        config = f'''root_dir = {json.dumps(str(root))}
[server]
host = "127.0.0.1"
port = {port}
[bitcoin]
url = "http://127.0.0.1:1"
[rpc]
balance_history_url = "http://127.0.0.1:1"
usdb_indexer_url = "http://127.0.0.1:1"
usdb_chain_url = "http://127.0.0.1:1"
ord_url = "http://127.0.0.1:1"
[web]
console_root = {json.dumps(str(roots[0]))}
balance_history_explorer_root = {json.dumps(str(roots[1]))}
usdb_indexer_explorer_root = {json.dumps(str(roots[2]))}
'''
        (root / "config.toml").write_text(config)
        report = dict(schema_version="usdb-console-monitor:v1", observed_at_ms=int(time.time() * 1000),
                      observation_available=True, overall_state="SYNCING", node_role="full", release_id="fixture-release",
                      network=dict(name="fixture-network", chain_id=123),
                      components=[dict(id="bitcoin", state="SYNCING", current=935100, total=960000,
                                       progress_phase="foreground", file_preparation=dict(state="VERIFIED"),
                                       background_validation=dict(height=100, target=935000, validated=False, available=True))])
        snapshot = root / "node-progress.json"
        snapshot.write_text(json.dumps(report))
        server = subprocess.Popen([str(ROOT / "src/btc/target/debug/usdb-control-plane"), "--root-dir", str(root), "--skip-process-lock"], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            origin = f"http://127.0.0.1:{port}"
            for _ in range(100):
                if server.poll() is not None:
                    raise AssertionError(server.stderr.read().decode())
                try:
                    with urlopen(origin + "/healthz", timeout=1) as response:
                        if response.status == 200:
                            break
                except OSError:
                    time.sleep(0.1)
            else:
                raise AssertionError("Console did not start")
            token = (root / "access-token").read_text().strip()
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
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
                server.wait(timeout=5)


if __name__ == "__main__":
    main()
