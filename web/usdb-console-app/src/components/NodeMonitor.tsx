import { useEffect, useState } from 'react'
import { MintingBackend } from './MintingBackend'
import { useI18n } from '../i18n/provider'
import type { MonitorSnapshot } from '../lib/types'

const labels: Record<string, [string, string]> = {
  snapshot: ['UTXO snapshot preparation', 'UTXO 快照准备'], bitcoin: ['Bitcoin foreground', 'Bitcoin 前台同步'],
  balance_history: ['Balance history', '余额历史'], usdb_indexer: ['USDB indexer', 'USDB 索引'],
  usdb_chain: ['USDB chain', 'USDB 链'], control_plane: ['Private console', '私有控制台'],
  script_registry: ['Script registry', '脚本注册表'], images: ['Runtime images', '运行镜像'],
}

/** The host's readiness model is authoritative; RPC connectivity is shown separately. */
export function NodeMonitor({ snapshot }: { snapshot?: MonitorSnapshot }) {
  const { locale } = useI18n()
  const zh = locale === 'zh-CN'
  const report = snapshot?.report
  const [age, setAge] = useState(snapshot?.age_ms ?? 0)
  useEffect(() => {
    const received = performance.now()
    const timer = window.setInterval(() => setAge((snapshot?.age_ms ?? 0) + performance.now() - received), 1000)
    return () => window.clearInterval(timer)
  }, [snapshot])
  const fresh = snapshot?.status === 'available' && age <= 120000
  const show = (value: unknown) => value === undefined || value === null ? '—' : String(value)
  const freshness = snapshot?.status === 'available' && !fresh ? 'stale' : snapshot?.status ?? 'loading'
  const statusNames: Record<string, string> = zh ? { available: '采集正常', missing: '监控进程未启用', stale: '数据已过期', unavailable: '采集不可用', invalid: '状态文件无效', loading: '正在读取' } : {}
  return (
    <section className="console-card grid gap-5" aria-label={zh ? '节点监控' : 'Node monitoring'}>
      <div className="flex flex-wrap justify-between gap-3">
        <h2 className="text-xl font-semibold">{zh ? '节点监控' : 'Node monitoring'}</h2>
        <strong role="status">{statusNames[freshness] ?? freshness} · {fresh ? show(report?.overall_state) : (zh ? '当前状态未知' : 'Current state unknown')}</strong>
      </div>
      <p className="text-sm">{zh ? '采集时间' : 'Observed'}: {report?.observed_at_ms ? new Date(report.observed_at_ms).toLocaleString(locale) : '—'}</p>
      {!fresh && <p role="alert">{zh ? '以下仅为最后一次观测。请在节点执行 usdb-node status --progress-json，并检查 console monitor 服务。' : 'Any values below are the last observation only. Run usdb-node status --progress-json and check the console monitor service.'}</p>}
      {report && <>
        <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4 text-sm">
          {[
            [zh ? '网络 / 角色' : 'Network / role', `${show(report.network?.name)} / ${show(report.node_role)}`],
            ['Chain ID', show(report.network?.chain_id)],
            [zh ? '版本' : 'Release', show(report.release_id)],
            [zh ? '启动控制器' : 'Bootstrap controller', show(report.controller?.state)],
            [zh ? '资源阶段' : 'Resource phase', `${show(report.resources?.mode)} / ${show(report.resources?.phase)}`],
            [zh ? '资源切换' : 'Resource transition', report.resources?.transition_pending ? (zh ? '进行中' : 'Pending') : '—'],
            [zh ? '挖矿状态' : 'Mining', show(report.mining?.state)],
          ].map(([label, value]) => <div key={label}><dt className="text-[color:var(--cp-muted)]">{label}</dt><dd className="mt-1 break-all">{value}</dd></div>)}
        </dl>
        <p className="break-all text-xs">Genesis: {show(report.network?.genesis_hash)}</p>
        {report.resources?.configured_limits_bytes && <details className="text-sm">
          <summary>{zh ? '容器内存上限（配置值，并非实时占用）' : 'Container memory ceilings (configured, not live usage)'}</summary>
          <dl className="mt-3 grid gap-2 sm:grid-cols-2">{Object.entries(report.resources.configured_limits_bytes).map(([name, bytes]) => <div key={name}><dt>{name}</dt><dd>{(bytes / 1024 ** 3).toFixed(2)} GiB</dd></div>)}</dl>
        </details>}
        <MintingBackend snapshot={snapshot} />
        <div className="grid gap-4 lg:grid-cols-2">
          {report.components?.map(component => <article key={component.id} className="rounded border border-[color:var(--cp-border)] p-4">
            <div className="flex justify-between gap-2"><h3>{labels[component.id]?.[zh ? 1 : 0] ?? component.label ?? component.id}</h3><strong>{fresh ? (component.display_state ?? component.state) : 'STALE'}</strong></div>
            <p className="mt-2 text-sm">{show(component.progress_phase)}</p>
            {component.current != null && <p className="mt-2">{component.current.toLocaleString(locale)} / {component.total?.toLocaleString(locale) ?? '—'} {component.unit}</p>}
            {component.progress_percent != null && <progress className="mt-2 w-full" aria-label={component.id} value={component.progress_percent} max={100} />}
            {component.file_preparation?.state && <p className="mt-2 text-sm">{zh ? '快照文件' : 'Snapshot file'}: {component.file_preparation.state === 'VERIFIED' ? (zh ? '下载及 SHA-256 校验完成' : 'Download and SHA-256 verification complete') : (zh ? '下载完成，正在校验' : 'Downloaded; verification in progress')}</p>}
            {component.background_validation?.target != null && <div className="mt-3 border-t border-[color:var(--cp-border)] pt-3 text-sm">
              <p>{zh ? 'Bitcoin 后台历史校验（不阻塞前台就绪）' : 'Bitcoin background validation (independent of foreground readiness)'}</p>
              <p>{component.background_validation.available ? `${show(component.background_validation.height)} / ${show(component.background_validation.target)}` : (zh ? '暂无观测' : 'Unavailable')} · {component.background_validation.validated ? (zh ? '已校验' : 'Validated') : (zh ? '尚未确认完成' : 'Completion not confirmed')}</p>
            </div>}
            {['FAILED', 'BLOCKED'].includes(component.state) && <p className="mt-2 text-sm">{zh ? '请在节点查看 status、doctor 和对应服务日志。' : 'Inspect node status, doctor, and the corresponding service logs.'}</p>}
          </article>)}
        </div>
      </>}
    </section>
  )
}
