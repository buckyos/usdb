import { useI18n } from '../i18n/provider'
import type { HostResources as ResourceSnapshot } from '../lib/types'
import { serviceTitle } from '../lib/monitoring'

/** Binary units match host tools; missing observations never become zero usage. */
function bytes(value?: number | null) {
  if (value == null || !Number.isFinite(value) || value < 0) return '—'
  const units = ['B', 'KiB', 'MiB', 'GiB', 'TiB', 'PiB']
  const unit = Math.min(units.length - 1, value > 0 ? Math.floor(Math.log(value) / Math.log(1024)) : 0)
  return `${(value / 1024 ** unit).toFixed(unit ? 1 : 0)} ${units[unit]}`
}

/** Present cached host observations; refreshing the page never grants host execution. */
export function HostResources({ data, fresh, ageMs, observedAt }: {
  data?: ResourceSnapshot; fresh: boolean; ageMs: number; observedAt?: number
}) {
  const { locale, t } = useI18n()
  const zh = locale === 'zh-CN'
  const text = (cn: string, en: string) => zh ? cn : en
  const time = (value?: number) => value == null ? '—' : new Date(value).toLocaleString(locale)
  const recent = (value?: number, ttl = 120000) => fresh && value != null && observedAt != null && ageMs + Math.max(0, observedAt - value) <= ttl
  const percentage = (value?: number | null) => value == null ? '—' : `${value.toFixed(1)}%`
  const host = data?.host
  const status = (value: string) => ({
    available: text('已采集', 'Observed'), pending: text('正在统计', 'Measuring'), missing: text('目录尚未创建', 'Directory not created'),
    timeout: text('统计超时', 'Scan timed out'), unavailable: text('无法采集', 'Unavailable'), invalid_path: text('目录配置无效', 'Invalid data path'),
    not_running: text('容器未运行', 'Container not running'),
    permission_denied: text('目录读取权限不足', 'Insufficient directory permissions'),
  }[value] ?? text('状态未知', 'Unknown'))
  return <section className="min-w-0 grid gap-4" aria-label={text('主机资源', 'Host resources')}>
    <div><h3 className="font-semibold">{text('主机资源与数据磁盘', 'Host resources and data storage')}</h3>
      <p className="mt-2 text-sm text-[color:var(--cp-muted)]">{text('刷新读取最近一次采样；CPU、内存随监控周期更新，目录占用每 5 分钟统计一次。', 'Refresh reads the latest sample. CPU and memory follow the observer cycle; directory sizes are measured every 5 minutes.')}</p>
    </div>
    {!data || data.status === 'unavailable' ? <p role="alert">{text('暂无资源采样。请升级节点工具并启动 console monitor；未知不等于占用为零。', 'No resource sample. Update the node tools and start console monitor; unknown does not mean zero usage.')}</p> : <>
      {!recent(data.observed_at_ms) && <p role="alert">{text('资源采样已过期或采集不可用，以下为历史数值。恢复采集后再判断当前负载与磁盘余量。', 'The resource observation is stale or unavailable. Values below are historical; restore monitoring before assessing current load or free space.')}</p>}
      <p className="text-xs">{text('主机采样时间', 'Host sampled')}: {time(host?.observed_at_ms)}</p>
      {host?.status === 'available' ? <dl className="grid gap-3 text-sm sm:grid-cols-2 xl:grid-cols-4">
        {[[text('CPU 总使用率', 'Total host CPU'), percentage(host.cpu_percent)],
          [text('内存已用 / 总量', 'Memory used / total'), `${bytes(host.memory_used_bytes)} / ${bytes(host.memory_total_bytes)}`],
          [text('可用内存', 'Available memory'), bytes(host.memory_available_bytes)],
          [text('Swap 已用 / 总量', 'Swap used / total'), `${bytes(host.swap_used_bytes)} / ${bytes(host.swap_total_bytes)}`]].map(([label, value]) =>
          <div key={label}><dt className="text-[color:var(--cp-muted)]">{label}</dt><dd className="mt-1">{value}</dd></div>)}
      </dl> : <p>{text('主机 CPU / 内存无法采集', 'Host CPU / memory unavailable')}</p>}
      <p className="text-xs text-[color:var(--cp-muted)]">{text('主机 CPU 为全核归一化占用；内存已用 = 总量 − 可用内存（包含可回收缓存的评估）。', 'Host CPU is normalized across all cores. Used memory is total minus MemAvailable, accounting for reclaimable cache.')} {host?.cpu_interval_ms != null && `${text('CPU 采样区间', 'CPU sample interval')}: ${(host.cpu_interval_ms / 1000).toFixed(1)} s`}</p>
      <details className="min-w-0 text-sm" open>
        <summary>{text('容器实际占用', 'Container utilization')}</summary>
        <p className="my-2 text-xs">{text('容器 CPU 的 100% 表示占用一个逻辑核；内存为 Docker 报告的工作集，包含约数，与配置上限分开显示。', 'Container CPU at 100% uses one logical core. Memory is the approximate working set reported by Docker, separate from configured ceilings.')} {time(data.containers?.observed_at_ms)}</p>
        {data.containers?.status !== 'available' ? <p>{text('容器采样失败，请检查 Docker 及节点账号权限。', 'Container sampling failed; check Docker and the node account permissions.')}</p> : <div className="overflow-x-auto">
          <table className="w-full text-left"><thead><tr>{[text('服务', 'Service'), text('状态', 'Status'), 'CPU', text('内存使用 / 运行上限', 'Memory / runtime limit')].map(label => <th className="p-2" key={label}>{label}</th>)}</tr></thead>
            <tbody>{data.containers.items.map((item, index) => <tr className="border-t" key={`${item.service}-${index}`}>
              <td className="p-2">{serviceTitle(item.service, t)}<span className="block text-xs text-[color:var(--cp-muted)]">{item.service}</span></td><td className="p-2">{status(item.status)}</td><td className="p-2">{percentage(item.cpu_percent)}</td>
              <td className="p-2 whitespace-nowrap">{bytes(item.memory_used_bytes)} / {bytes(item.memory_limit_bytes)}</td>
            </tr>)}</tbody></table>
          {!data.containers.items.length && <p>{text('没有发现本节点的容器。', 'No containers found for this node.')}</p>}
        </div>}
      </details>
      <div className="grid gap-3 sm:grid-cols-2">{data.filesystems?.map(disk => {
        const current = recent(disk.observed_at_ms)
        const warning = disk.warning !== 'ok'
        return <article key={disk.id} className="rounded border border-[color:var(--cp-border)] p-3 text-sm">
          <div className="flex flex-wrap justify-between gap-2"><strong className="break-all">{disk.mount_path}</strong>
            <span data-tone={current ? disk.warning === 'critical' ? 'danger' : warning ? 'warning' : 'success' : 'neutral'} className="status-pill">{!current ? text('历史观测', 'Historical') : disk.warning === 'critical' ? text('磁盘余量紧张', 'Disk critically low') : warning ? text('磁盘余量预警', 'Low disk space') : text('磁盘余量充足', 'Disk space available')}</span></div>
          <p className="mt-2">{text('可用 / 总容量', 'Available / capacity')}: {bytes(disk.available_bytes)} / {bytes(disk.total_bytes)}</p>
          <p>{text('文件系统已用', 'Filesystem used')}: {bytes(disk.used_bytes)}</p>
          <p className="text-xs mt-2">{time(disk.observed_at_ms)}</p>
        </article>
      })}</div>
      <p className="text-xs text-[color:var(--cp-muted)]">{text('同一文件系统只展示一次，包含同盘其他应用占用。低于 10% 或 100 GiB 提醒；低于 5% 或 50 GiB 标红，仅作容量提示。', 'Each filesystem appears once, including space used by other applications. Warn below 10% or 100 GiB; critical below 5% or 50 GiB. Advisory only.')}</p>
      <div className="grid gap-3"><h4 className="font-semibold">{text('关键数据目录', 'Service data directories')}</h4>
        {data.directories?.map(item => <article key={item.service} className="rounded border border-[color:var(--cp-border)] p-3 text-sm">
          <div className="flex flex-wrap justify-between gap-2"><strong title={item.service}>{serviceTitle(item.service, t)}</strong><span>{bytes(item.used_bytes)} · {status(item.status)}{item.used_bytes != null && (!recent(item.observed_at_ms, 600000) || item.status !== 'available') ? text('（上次成功统计）', ' (last successful scan)') : ''}</span></div>
          <p className="mt-2 break-all font-mono text-xs">{item.path ?? '—'}</p>
          <p className="mt-2 text-xs">{text('大小统计时间', 'Size observed')}: {time(item.observed_at_ms)}{item.filesystem_status === 'unavailable' && ` · ${text('磁盘容量无法读取', 'Filesystem capacity unavailable')}`}</p>
        </article>)}
        {!data.directories?.length && <p>{text('尚未配置可采集的数据目录。', 'No supported data directories configured.')}</p>}
      </div>
      <p className="text-xs text-[color:var(--cp-muted)]">{text('目录大小为已分配磁盘空间，统计有时限且不跨文件系统；超时或权限不足会明确显示，保留上次成功值。共享或重叠目录不可直接相加。', 'Directory size measures allocated disk blocks with a timeout and stays on one filesystem. Incomplete scans keep the last successful value and show their status. Shared or overlapping directories must not be summed.')}</p>
    </>}
  </section>
}
