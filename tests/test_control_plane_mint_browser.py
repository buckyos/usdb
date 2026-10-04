"""V2 wallet workflow: explicit intent, no production signing, no stale funding approval."""
from common.control_plane_web import console_server, default_report

SOURCE = "1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa"
RECIPIENT = "bc1qxy2kgdygjrsqtzq2n0yrf2493p83kkfjhx0wlh"
PASS = "a" * 64 + "i0"


def main():
    from playwright.sync_api import expect, sync_playwright
    report = default_report()
    report["network"].update(bitcoin_network="btc-mainnet", genesis_hash="0x" + "a" * 64)
    with console_server(report) as (origin, token, _, _), sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(viewport=dict(width=1200, height=1000))
        failures, requests = [], []
        page.on("pageerror", lambda error: failures.append(str(error)))
        verified, available, development, delayed = False, True, False, []
        hold = False

        def overview(route):
            response = route.fetch()
            if response.status != 200:
                route.fulfill(response=response)
                return
            payload = response.json()
            payload["development_enabled"] = development
            route.fulfill(response=response, json=payload)

        def prepare_payload(request):
            path = "same_owner" if request["source_address"] == request["recipient_address"] else "cross_owner" if request["prev"] else "first_opening"
            return dict(eligible=True, execution_available=development, operation_path=path, blockers=[], warnings=[],
                        observation=dict(height=100, source_balance_sats="100000", recipient_balance_sats="0", recipient_ever_valid_owner=False),
                        source_passes=[], retained_pass_ids=[], inscription_payload_json=__import__('json').dumps(dict(p="usdb", op="mint", v=1, **request)))

        def mint(route):
            request = route.request.post_data_json
            requests.append((route.request.url.rsplit('/', 1)[-1], request))
            if requests[-1][0] == "prepare":
                if hold:
                    delayed.append((route, prepare_payload(request)))
                else:
                    route.fulfill(json=prepare_payload(request))
            elif requests[-1][0] == "execute":
                assert development
                assert request["source_address"] == SOURCE and request["recipient_address"] == RECIPIENT
                route.fulfill(json=dict(inscription_id=PASS, source_outpoint="b" * 64 + ":0"))
            elif not available:
                route.fulfill(status=502, json=dict(error="Source evidence unavailable"))
            else:
                route.fulfill(json=dict(verified=verified, blockers=[] if verified else ["Actual source differs"],
                                        observed_height=100, confirmations=12, required_confirmations=11,
                                        source_balance_sats="100000", remaining_source_pass_ids=[], source=dict(source_outpoint="b" * 64 + ":0")))

        page.route("**/api/system/overview", overview)
        page.route("**/api/btc/mint/*", mint)
        page.route("**/api/btc/world-sim/identities", lambda route: route.fulfill(json=dict(available=True, identities=[dict(wallet_name="ord-a", owner_address=SOURCE)])))
        page.goto(origin + "/?lang=zh-CN#/me/btc")
        page.get_by_label("访问令牌").fill(token)
        page.get_by_role("button", name="登录", exact=True).click()
        expect(page.get_by_role("heading", name="MinerPass V2 铸造与核验")).to_be_visible(timeout=15000)
        panel = page.get_by_role("region", name="MinerPass V2")
        panel.get_by_label("来源地址 D", exact=True).fill(SOURCE)
        panel.get_by_label("接收地址 E", exact=True).fill(RECIPIENT)
        panel.get_by_label("usdb_main", exact=True).fill("0x" + "1" * 40)
        panel.get_by_role("button", name="预检并生成草案").click()
        expect(panel.get_by_test_id("mint-prepared")).to_contain_text("首次零余额开户")
        assert requests[-1][1]["source_address"] != requests[-1][1]["recipient_address"]
        expect(panel.get_by_role("button", name="使用 regtest Ord 钱包广播")).to_have_count(0)
        panel.get_by_label("待核验铭文 ID", exact=True).fill(PASS)
        panel.get_by_role("button", name="核验后再入金").click()
        expect(panel.get_by_test_id("mint-verification")).to_contain_text("尚未通过核验，请勿入金")
        verified = True
        panel.get_by_role("button", name="核验后再入金").click()
        expect(panel.get_by_test_id("mint-verification")).to_contain_text("核验通过：")
        # A dependency failure removes earlier success immediately.
        available = False
        panel.get_by_role("button", name="核验后再入金").click()
        expect(panel.get_by_role("alert")).to_contain_text("Source evidence unavailable")
        expect(panel.get_by_test_id("mint-verification")).to_have_count(0)
        available = True
        panel.get_by_role("button", name="核验后再入金").click()
        expect(panel.get_by_test_id("mint-verification")).to_contain_text("核验通过：")
        panel.get_by_label("usdb_main", exact=True).fill("0x" + "2" * 40)
        expect(panel.get_by_test_id("mint-verification")).to_have_count(0)
        expect(panel.get_by_test_id("mint-prepared")).to_have_count(0)
        # Delayed responses cannot approve an edited draft.
        hold = True
        panel.get_by_role("button", name="预检并生成草案").click()
        page.wait_for_timeout(100)
        panel.get_by_label("接收地址 E", exact=True).fill(SOURCE)
        assert delayed
        route, payload = delayed.pop()
        route.fulfill(json=payload)
        page.wait_for_timeout(100)
        expect(panel.get_by_test_id("mint-prepared")).to_have_count(0)
        hold = False
        panel.get_by_role("button", name="预检并生成草案").click()
        expect(panel.get_by_test_id("mint-prepared")).to_contain_text("同地址操作")
        panel.get_by_label("接收地址 E", exact=True).fill(RECIPIENT)
        panel.get_by_label("要继承的 prev", exact=False).fill("c" * 64 + "i0")
        panel.get_by_label("矿工证类型／绑定").select_option("leader_pass_id")
        panel.get_by_label("leader_pass_id", exact=True).fill("d" * 64 + "i0")
        panel.get_by_role("button", name="预检并生成草案").click()
        expect(panel.get_by_test_id("mint-prepared")).to_contain_text("跨地址继承")
        assert "usdb_main" not in requests[-1][1]
        assert requests[-1][1]["leader_pass_id"] == "d" * 64 + "i0"
        assert all(action != "execute" for action, _ in requests)
        # The development broadcast receipt stays unverified until the independent check succeeds.
        development = True
        # Change the document URL so old cached public capabilities cannot redirect this fixture.
        page.goto(origin + "/?lang=zh-CN&fixture=development#/development/btc")
        expect(page.get_by_role("button", name="读取 regtest 开发钱包")).to_be_visible(timeout=15000)
        page.get_by_role("button", name="读取 regtest 开发钱包").click()
        expect(page.locator('option[value="ord-a"]')).to_have_count(1)
        panel = page.get_by_role("region", name="MinerPass V2")
        panel.get_by_label("开发来源钱包").select_option("ord-a")
        panel.get_by_label("接收地址 E", exact=True).fill(RECIPIENT)
        panel.get_by_label("usdb_main", exact=True).fill("0x" + "1" * 40)
        panel.get_by_role("button", name="预检并生成草案").click()
        panel.get_by_role("button", name="使用 regtest Ord 钱包广播").click()
        expect(panel).to_contain_text("已广播，尚未核验；请勿入金")
        expect(panel.get_by_label("待核验铭文 ID", exact=True)).to_have_value(PASS)
        expect(panel.get_by_test_id("mint-verification")).to_have_count(0)
        panel.get_by_role("button", name="核验后再入金").click()
        expect(panel.get_by_test_id("mint-verification")).to_contain_text("核验通过：")
        assert requests[-1][1]["expected_source_outpoint"] == "b" * 64 + ":0"
        page.evaluate("window.dispatchEvent(new Event('focus'))")
        expect(panel.get_by_test_id("mint-verification")).to_have_count(0)
        assert not failures, failures
        browser.close()
    print("MinerPass browser acceptance passed: first/same/cross, collab, failure, stale draft, dev broadcast, no production execution")


if __name__ == "__main__":
    main()
