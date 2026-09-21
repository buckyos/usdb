"""Opt-in browser checks for shared terminology and honest historical progress labels."""
import json
import time
from common.control_plane_web import console_server, default_report


def main():
    from playwright.sync_api import expect, sync_playwright
    report = default_report()
    report['components'] = [
        dict(id='snapshot', state='READY', progress_phase='ready', progress_percent=100,
             file_preparation=dict(state='VERIFIED')),
        dict(id='script_registry', state='SKIPPED'),
        dict(id='bitcoin', state='READY', progress_phase='foreground', current=967980, total=967980,
             unit='blocks', progress_percent=100,
             background_validation=dict(height=None, target=935000, validated=True, available=True)),
        dict(id='balance_history', state='READY', progress_phase='Indexing', current=967970, total=967970,
             unit='blocks', progress_percent=100),
        dict(id='usdb_indexer', state='READY', current=967970, total=967970, unit='blocks', progress_percent=100),
        dict(id='usdb_chain', state='READY', current=7765, unit='blocks'),
        dict(id='control_plane', state='READY'),
    ]
    with console_server(report) as (origin, token, snapshot, report), sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(viewport=dict(width=1440, height=1100))
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.goto(origin + '/?lang=zh-CN')
        page.get_by_label('访问令牌').fill(token)
        page.get_by_role('button', name='登录', exact=True).click()
        panel = page.get_by_role('region', name='节点监控', exact=True)
        balance = panel.get_by_role('article', name='Bitcoin 历史余额索引', exact=True)
        expect(balance).to_contain_text('当前阶段：构建并更新索引', timeout=15000)
        expect(balance).to_contain_text('已同步 / 目标高度：967,970 / 967,970')
        expect(panel.get_by_role('article', name='USDB 链节点', exact=True)).to_contain_text('区块高度：7,765')
        expect(panel).to_contain_text('已完成基准区块 935,000 及之前的历史验证')
        expect(panel.get_by_role('article', name='Bitcoin 脚本注册表', exact=True)).to_contain_text('当前配置无需单独执行此步骤')
        balance.locator('..').screenshot(path='/tmp/usdb-console-terminology-fresh.png')
        # A component can be stale even when the host snapshot itself is fresh.
        report['components'][3]['display_state'] = 'STALE'
        snapshot.write_text(json.dumps(report))
        page.reload()
        expect(balance).to_contain_text('观测已过期', timeout=15000)
        expect(balance.get_by_role('progressbar')).to_have_attribute('data-current', 'false')
        expect(panel.get_by_role('status').first).to_contain_text('采集正常')
        # When the entire snapshot expires, every progress bar becomes historical.
        report['observed_at_ms'] = int(time.time() * 1000) - 130000
        snapshot.write_text(json.dumps(report))
        page.reload()
        expect(panel.get_by_role('status').first).to_contain_text('当前状态未知', timeout=15000)
        expect(balance).to_contain_text('上次观测阶段：构建并更新索引')
        assert all(bar.get_attribute('data-current') == 'false' for bar in panel.get_by_role('progressbar').all())
        balance.locator('..').screenshot(path='/tmp/usdb-console-terminology-stale.png')
        page.set_viewport_size(dict(width=390, height=844))
        assert page.evaluate('document.documentElement.scrollWidth <= innerWidth'), 'Monitoring layout overflows on mobile'
        balance.locator('..').screenshot(path='/tmp/usdb-console-terminology-mobile.png')
        # Future statuses remain diagnosable without silently becoming READY.
        report['observed_at_ms'] = int(time.time() * 1000)
        report['components'][3].update(state='FUTURE_STATE', display_state='FUTURE_STATE', progress_phase='NewStage')
        snapshot.write_text(json.dumps(report))
        page.reload()
        expect(balance).to_contain_text('未知 (FUTURE_STATE)', timeout=15000)
        expect(balance).to_contain_text('未知 (NewStage)')
        page.goto(origin + '/?lang=zh-CN#/services/balance-history')
        expect(page.get_by_role('heading', name='Bitcoin 历史余额索引', exact=True)).to_be_visible(timeout=15000)
        expect(page.get_by_text('balance-history', exact=True)).to_be_visible()
        page.goto(origin + '/?lang=en#/overview')
        expect(page.get_by_role('article', name='Bitcoin historical balance index', exact=True)).to_contain_text('Unknown (NewStage)', timeout=15000)
        assert not errors, errors
        browser.close()
    print('Terminology browser checks passed: shared names, phases, height counters, stale observations, unknown states, bilingual display and mobile layout')


if __name__ == '__main__':
    main()
