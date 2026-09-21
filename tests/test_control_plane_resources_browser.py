"""Opt-in browser coverage for resource freshness, failures, capacity warnings and mobile layout."""
import json
import time
from common.control_plane_web import console_server, default_report
from common.resource_observation import resource_observation


def main():
    from playwright.sync_api import expect, sync_playwright
    report = default_report()
    report['host_resources'] = resource_observation()
    with console_server(report) as (origin, token, snapshot, report), sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True)
        page = browser.new_page(viewport=dict(width=1440, height=1100))
        errors = []
        page.on('pageerror', lambda error: errors.append(str(error)))
        page.goto(origin + '/?lang=zh-CN')
        page.get_by_label('访问令牌').fill(token)
        page.get_by_role('button', name='登录', exact=True).click()
        panel = page.get_by_role('region', name='主机资源', exact=True)
        expect(panel).to_contain_text('23.5%', timeout=15000)
        expect(panel).to_contain_text('24.0 GiB / 64.0 GiB')
        expect(panel).to_contain_text('210.5%')
        expect(panel).to_contain_text('容器未运行')
        expect(panel).to_contain_text('磁盘余量紧张')
        expect(panel).to_contain_text('400.0 GiB · 统计超时（上次成功统计）')
        expect(panel.get_by_text('/data', exact=True)).to_have_count(1)
        panel.screenshot(path='/tmp/usdb-console-resources-panel.png')
        page.screenshot(path='/tmp/usdb-console-resources-desktop.png', full_page=True)
        page.set_viewport_size(dict(width=390, height=844))
        assert page.evaluate('document.documentElement.scrollWidth <= window.innerWidth'), 'Resource panel mobile overflow'
        page.screenshot(path='/tmp/usdb-console-resources-mobile.png', full_page=True)
        # Resource failures do not remove node progress or make missing metrics zero.
        report['host_resources']['containers'] = dict(status='unavailable', items=[])
        snapshot.write_text(json.dumps(report))
        page.get_by_role('button', name='刷新', exact=True).click()
        expect(panel).to_contain_text('容器采样失败', timeout=15000)
        expect(panel).to_contain_text('24.0 GiB / 64.0 GiB')
        expect(page.get_by_role('status').first).to_contain_text('采集正常')
        report['observed_at_ms'] = int(time.time() * 1000) - 130000
        snapshot.write_text(json.dumps(report))
        page.reload()
        expect(panel).to_contain_text('资源采样已过期或采集不可用', timeout=15000)
        expect(panel).to_contain_text('历史观测')
        expect(panel.get_by_text('磁盘余量紧张', exact=True)).to_have_count(0)
        expect(page.get_by_role('region', name='节点监控', exact=True)).to_contain_text('刷新网页不会启动采集进程')
        # Older observers remain compatible and get a specific upgrade instruction.
        report.pop('host_resources')
        report['observed_at_ms'] = int(time.time() * 1000)
        snapshot.write_text(json.dumps(report))
        page.reload()
        expect(panel).to_contain_text('暂无资源采样', timeout=15000)
        assert not errors, errors
        browser.close()
    print('Resource browser regression passed: host/container values, disk warnings, cached partial scans, refresh, stale observations, old observer, mobile')


if __name__ == '__main__':
    main()
