"""Opt-in browser wallet regression with fake extensions and an isolated authenticated server."""
from common.control_plane_web import console_server, default_report

ADDRESS = "0x" + "1" * 40
SECOND_ADDRESS = "0x" + "2" * 40
GENESIS = "0x" + "a" * 64
PROVIDERS = """(() => {
  const listeners = () => ({
    events: new Map(),
    on(name, fn) { if (!this.events.has(name)) this.events.set(name, new Set()); this.events.get(name).add(fn) },
    removeListener(name, fn) { this.events.get(name)?.delete(fn) },
    emit(name) { for (const fn of this.events.get(name) || []) fn() },
    count() { return [...this.events.values()].reduce((n, set) => n + set.size, 0) },
  });
  const state = window.walletFixture = {
    address: '0x' + '1'.repeat(40), chain: '0x7b', genesis: '0x' + 'a'.repeat(64),
    btcChain: 'BITCOIN_TESTNET', btcAddress: '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa',
    prompts: 0, reject: false, methods: [], delay: false, release: null,
  };
  window.ethereum = { ...listeners(), async request({method, params}) {
    state.methods.push(method);
    if (method === 'eth_requestAccounts') { state.prompts++; if (state.reject) throw {code: 4001}; return [state.address] }
    if (method === 'eth_accounts') return [state.address];
    if (method === 'eth_chainId') return state.chain;
    if (method === 'eth_getBlockByNumber') { if (state.delay) await new Promise(resolve => { state.release = resolve }); return {hash: state.genesis} }
    if (method === 'wallet_switchEthereumChain') { state.chain = params[0].chainId; window.ethereum.emit('chainChanged'); return null }
    throw new Error('Unexpected wallet method: ' + method);
  }};
  window.unisat = { ...listeners(),
    async requestAccounts() { state.prompts++; return [state.btcAddress] },
    async getAccounts() { return [state.btcAddress] },
    async getChain() { return {enum: state.btcChain} },
  };
  localStorage.setItem('usdb.devRegtestWallet.v1', 'obsolete-test-secret');
})();
"""


def main():
    from playwright.sync_api import expect, sync_playwright

    report = default_report()
    report["network"].update(genesis_hash=GENESIS, bitcoin_network="btc-mainnet")
    report["node_identity"] = dict(configured_miner_address="0x" + "3" * 40)
    with console_server(report) as (origin, token, _, _), sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        context = browser.new_context(viewport=dict(width=1365, height=1000))
        context.add_init_script(PROVIDERS)
        page = context.new_page()
        failures = []
        page.on("pageerror", lambda error: failures.append(str(error)))
        queried = []
        query_failure = False
        balance_genesis = GENESIS
        pass_present = False

        def overview(route):
            response = route.fetch()
            if response.status != 200:
                route.fulfill(response=response)
                return
            payload = response.json()
            payload["services"]["usdb_chain"].update(reachable=True, data=dict(chain_id="0x7b", genesis_hash=GENESIS))
            payload["services"]["usdb_indexer"].update(reachable=True, data=dict(network="mainnet", query_ready=True, features=[]))
            route.fulfill(response=response, json=payload)

        def balance(route):
            queried.append(route.request.url)
            from urllib.parse import parse_qs, urlsplit
            address = parse_qs(urlsplit(route.request.url).query)["address"][0]
            route.fulfill(json=dict(available=True, address=address, usdb_chain_id="0x7b", usdb_genesis_hash=balance_genesis,
                                    balance_atoms_hex="0xde0b6b3a7640001", latest_block_number="0x10",
                                    usdb_chain_runtime_profile="public"))

        def passes(route):
            queried.append(route.request.post_data)
            assert route.request.post_data_json["method"] == "get_owner_active_pass_at_height"
            if query_failure:
                route.fulfill(status=503, json=dict(error="fixture unavailable"))
            elif pass_present:
                route.fulfill(json=dict(inscription_id="a" * 64 + "i0", state="active", pass_kind="standard",
                                        usdb_main=SECOND_ADDRESS, resolved_height=935100))
            else:
                route.fulfill(body="null", content_type="application/json")

        page.route("**/api/system/overview", overview)
        page.route("**/api/usdb-chain/address-status?*", balance)
        page.route("**/api/services/usdb-indexer/rpc", passes)
        page.goto(origin + "/?lang=zh-CN#/me/usdb")
        page.get_by_label("访问令牌").fill(token)
        page.get_by_role("button", name="登录", exact=True).click()
        expect(page.get_by_role("heading", name="钱包与身份")).to_be_visible(timeout=15000)
        assert page.evaluate("walletFixture.prompts") == 0
        assert page.evaluate("localStorage.getItem('usdb.devRegtestWallet.v1')") is None
        assert not queried

        page.evaluate("walletFixture.reject = true")
        page.get_by_role("button", name="连接钱包", exact=True).click()
        expect(page.get_by_role("alert")).to_contain_text("已取消钱包授权")
        page.evaluate("walletFixture.reject = false")
        page.get_by_role("button", name="连接钱包", exact=True).click()
        results = page.get_by_role("region", name="本节点查询结果")
        expect(results).to_contain_text("1.000000000000000001 USDB")
        expect(page.get_by_text("网络匹配，可查询本节点数据。", exact=True)).to_be_visible()
        page.evaluate("walletFixture.address = '0x' + '2'.repeat(40); walletFixture.chain = '0x7c'; ethereum.emit('accountsChanged')")
        expect(page.get_by_text("钱包与本节点网络不匹配", exact=False)).to_be_visible()
        expect(results).to_have_count(0)
        assert not any(SECOND_ADDRESS in value for value in queried)
        page.get_by_role("button", name="请求钱包切换至本节点 Chain ID").click()
        expect(results).to_contain_text(SECOND_ADDRESS)
        page.evaluate("walletFixture.genesis = '0x' + 'b'.repeat(64); ethereum.emit('chainChanged')")
        expect(page.get_by_text("钱包与本节点网络不匹配", exact=False)).to_be_visible()
        expect(results).to_have_count(0)
        expect(page.get_by_role("button", name="请求钱包切换至本节点 Chain ID")).to_have_count(0)
        page.evaluate("walletFixture.genesis = '0x' + 'a'.repeat(64); walletFixture.delay = true; ethereum.emit('chainChanged')")
        page.wait_for_function("walletFixture.release !== null")
        page.get_by_role("button", name="断开本页连接").click()
        page.evaluate("walletFixture.delay = false; walletFixture.release()")
        expect(page.get_by_text("未连接", exact=True)).to_be_visible()
        expect(results).to_have_count(0)
        assert page.evaluate("ethereum.count()") == 0

        page.get_by_role("button", name="只读地址查询").click()
        page.get_by_label("只读地址", exact=True).fill(ADDRESS)
        page.get_by_role("button", name="查询地址", exact=True).click()
        expect(results).to_contain_text("1.000000000000000001 USDB")
        balance_genesis = "0x" + "f" * 64
        page.get_by_role("button", name="刷新查询", exact=True).click()
        expect(results).to_contain_text("本节点查询失败")
        expect(results).not_to_contain_text("1.000000000000000001 USDB")
        balance_genesis = GENESIS
        page.get_by_label("只读地址", exact=True).fill("bad-address")
        expect(results).to_have_count(0)
        page.get_by_role("button", name="查询地址", exact=True).click()
        expect(page.get_by_role("alert")).to_contain_text("地址格式")

        page.get_by_role("link", name="BTC 矿工证身份").click()
        expect(page.get_by_text("未连接", exact=True)).to_be_visible()
        page.get_by_role("button", name="连接钱包", exact=True).click()
        expect(page.get_by_text("钱包与本节点网络不匹配", exact=False)).to_be_visible()
        page.evaluate("walletFixture.btcChain = 'BITCOIN_MAINNET'; unisat.emit('networkChanged')")
        expect(results).to_contain_text("未查到有效矿工证")
        pass_present = True
        page.get_by_role("button", name="刷新查询", exact=True).click()
        expect(results).to_contain_text(SECOND_ADDRESS)
        expect(results).to_contain_text("935100")
        query_failure = True
        page.get_by_role("button", name="刷新查询", exact=True).click()
        expect(results).to_contain_text("本节点查询失败")
        expect(results).not_to_contain_text("未查到有效矿工证")
        page.set_viewport_size(dict(width=390, height=844))
        assert page.evaluate("document.documentElement.scrollWidth <= innerWidth"), "Wallet page mobile overflow"
        page.screenshot(path="/tmp/usdb-wallet-identity-mobile.png", full_page=True)
        page.get_by_role("button", name="退出登录").click()
        expect(page.get_by_role("heading", name="USDB 私有控制台")).to_be_visible()
        assert page.evaluate("unisat.count() + ethereum.count()") == 0
        assert not any("sign" in method.lower() or "send" in method.lower() for method in page.evaluate("walletFixture.methods"))
        assert not failures, failures
        context.close()

        # No extension is required for explicit observation, and dev tools stay gated.
        context = browser.new_context()
        page = context.new_page()
        page.route("**/api/system/overview", overview)
        page.route("**/api/services/usdb-indexer/rpc", passes)
        page.goto(origin + "/?lang=zh-CN#/me/btc")
        page.get_by_label("访问令牌").fill(token)
        page.get_by_role("button", name="登录", exact=True).click()
        expect(page.get_by_role("heading", name="钱包与身份")).to_be_visible(timeout=15000)
        expect(page.get_by_role("button", name="连接钱包", exact=True)).to_be_disabled()
        page.get_by_role("button", name="只读地址查询").click()
        page.get_by_label("只读地址", exact=True).fill("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNb")
        page.get_by_role("button", name="查询地址", exact=True).click()
        expect(page.get_by_role("alert")).to_contain_text("地址格式")
        query_failure = False
        page.get_by_label("只读地址", exact=True).fill("1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa")
        page.get_by_role("button", name="查询地址", exact=True).click()
        expect(page.get_by_role("region", name="本节点查询结果")).to_contain_text(SECOND_ADDRESS)
        page.goto(origin + "/?lang=zh-CN#/development/btc")
        expect(page).to_have_url(origin + "/?lang=zh-CN#/me/usdb")
        expect(page.get_by_role("heading", name="钱包与身份")).to_be_visible()
        browser.close()
    print("Wallet browser regression passed: rejection, network/genesis guards, late responses, watch-only, query errors, mobile, logout")


if __name__ == "__main__":
    main()
