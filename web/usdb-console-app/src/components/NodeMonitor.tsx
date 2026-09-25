import { useEffect, useState } from 'react'
import { MintingBackend } from './MintingBackend'
import { HostResources } from './HostResources'
import { useI18n } from '../i18n/provider'
import type { MonitorSnapshot } from '../lib/types'
import { componentTitle, monitorValue, progressCounter, serviceTitle } from '../lib/monitoring'

/** The host's readiness model is authoritative; RPC connectivity is shown separately. */
export function NodeMonitor({ snapshot }: { snapshot?: MonitorSnapshot }) {
  const { locale, t } = useI18n()
  const zh = locale === 'zh-CN'
  const report = snapshot?.report
  const core = report?.monitor
  const [age, setAge] = useState(snapshot?.age_ms ?? 0)
  useEffect(() => {
    const received = performance.now()
    setAge(snapshot?.age_ms ?? 0)
    const timer = window.setInterval(() => setAge((snapshot?.age_ms ?? 0) + performance.now() - received), 1000)
    return () => window.clearInterval(timer)
  }, [snapshot])
  const fresh = snapshot?.status === 'available' && age <= 120000
  const show = (value: unknown) => value === undefined || value === null ? '—' : String(value)
  const freshness = snapshot?.status === 'available' && !fresh ? 'stale' : snapshot?.status ?? 'loading'
  const statusNames: Record<string, string> = zh
    ? { available: '采集正常', missing: '暂无监控数据', stale: '观测已过期', unavailable: '无法获取观测', invalid: '观测文件无效', loading: '正在读取' }
    : { available: 'Observation available', missing: 'No monitoring data', stale: 'Observation expired', unavailable: 'Observation unavailable', invalid: 'Invalid observation file', loading: 'Loading' }
  return (
    <section className="console-card grid gap-5" aria-label={zh ? '节点监控' : 'Node monitoring'}>
      <div className="flex flex-wrap justify-between gap-3">
        <h2 className="text-xl font-semibold">{zh ? '节点监控' : 'Node monitoring'}</h2>
        <strong role="status">{statusNames[freshness] ?? freshness} · {fresh ? monitorValue('states', report?.overall_state, t) : (zh ? '当前状态未知' : 'Current state unknown')}</strong>
      </div>
      <p className="text-sm">{zh ? '采集时间' : 'Observed'}: {report?.observed_at_ms ? new Date(report.observed_at_ms).toLocaleString(locale) : '—'}</p>
      {core && <div className="grid gap-2 text-sm">
        <p>{zh ? '核心监控' : 'Core monitor'}: {core.state} · {zh ? '事件存储' : 'Event storage'}: {core.storage}</p>
        <p>{zh ? '本版持续记录本地事件，通知投递尚未接入。' : 'This release records local events; notification delivery is not yet implemented.'}</p>
        {core.state === 'disabled' && <p>{zh ? '监控已按节点配置关闭，历史记录保留。需要启用时在节点终端配置 monitor，再执行 up。' : 'Monitoring is disabled by node configuration; history is retained. Configure monitor on the node, then run up to enable it.'}</p>}
        {core.storage === 'unavailable' && <p role="alert">{zh ? '事件持久化未确认正常，请检查 monitor status 和服务日志。' : 'Event persistence is not confirmed healthy. Inspect monitor status and its journal.'}</p>}
        {!!core.alerts.length && <div className="grid gap-2">
          <strong>{zh ? '已记录的活动告警' : 'Recorded active alerts'}</strong>
          {core.alerts.map(alert => <div key={alert.alert_id} className="rounded border border-[color:var(--cp-border)] p-3">
            <p>{alert.severity} · {serviceTitle(alert.service, t)} · {alert.code} · {alert.state}</p>
            <p>{zh ? '首次 / 最近发现' : 'First / last seen'}: {new Date(alert.first_seen_ms).toLocaleString(locale)} / {new Date(alert.last_seen_ms).toLocaleString(locale)}</p>
            <p>{alert.acknowledged_at_ms ? (zh ? '已确认接手' : 'Acknowledged') : (zh ? '未确认' : 'Unacknowledged')}{alert.latched && (zh ? '；需人工核实恢复，确认不解除保护。' : '; manual recovery verification required; acknowledgement does not release protection.')}</p>
            <p className="break-all">ID: {alert.alert_id}</p>
          </div>)}
        </div>}
        <details>
          <summary>{zh ? '最近本地事件' : 'Recent local events'} ({core.events.length})</summary>
          <p className="mt-2">{zh ? '完整历史和筛选请使用 usdb-node monitor events；事件记录不替代服务完整日志。' : 'Use usdb-node monitor events for full history and filters. Events complement the service logs.'}</p>
          {core.events.map(event => <details key={event.event_id} className="mt-2 rounded border border-[color:var(--cp-border)] p-2">
            <summary>{new Date(event.at_ms).toLocaleString(locale)} · {event.severity} · {event.service} · {event.code}</summary>
            <p className="mt-2 break-all">ID: {event.event_id}</p>
            <pre className="mt-2 whitespace-pre-wrap break-all">{JSON.stringify(event.evidence, null, 2)}</pre>
          </details>)}
        </details>
      </div>}
      {!fresh && <div role="alert" className="grid gap-2 text-sm">
        <p>{zh ? '当前状态未知表示宿主机观测缺失或已过期，不代表所有服务离线。以下数值仅为最后一次观测，刷新网页不会启动采集进程。' : 'Unknown means host observations are missing or stale, not that every service is offline. Values below are the last observation; refreshing this page does not start the observer.'}</p>
        <p>{zh ? '已有节点升级后，先在节点终端执行 usdb-node up --no-watch，检查并补齐标准后台服务（可能需要 sudo 密码）。自定义服务按命令提示检查，不直接覆盖。' : 'After upgrading, run usdb-node up --no-watch to check and repair managed background services (sudo may be required). Follow its guidance for custom units without overwriting them.'}</p>
        <p>{zh ? '检查监控：usdb-node monitor status；systemctl status usdb-node-monitor-<bundle-id>.service。前台模式可运行 usdb-node monitor run。' : 'Inspect usdb-node monitor status and systemctl status usdb-node-monitor-<bundle-id>.service. Foreground deployments can run usdb-node monitor run.'}</p>
      </div>}
      {report && <>
        {report.observations?.incidents.status === 'unavailable' && <p role="alert" className="text-sm">
          {zh ? '无法读取持久事故记录，当前不能确认是否存在停机事故。' : 'Durable incident records are unavailable; the halt state is unknown.'}
        </p>}
        {report.observations?.incidents.events.map(event => <div key={event.event_id ?? event.code} role="alert" className="rounded border border-[color:var(--cp-border)] p-4 text-sm">
          <strong>{!fresh && (zh ? '上次观测：' : 'Last observed: ')}{zh ? '严重事故，需要人工处理' : 'Critical incident; manual intervention required'} · {event.code}</strong>
          <p className="mt-2">{zh ? '深度 BTC 重组保护已记录持久停机。请保留事故记录并联系网络运维；重启不能解除此状态。' : 'The deep BTC reorg guard recorded a durable halt. Preserve the incident record and contact the network operator; restarting does not clear it.'}</p>
          <p className="mt-2 break-all">ID: {show(event.event_id)} · {zh ? '发生时间' : 'Detected'}: {show(event.detected_at)}</p>
          {event.evidence_status !== 'available' && <p>{zh ? '事故文件存在，但详情无效或不可读取；不能据此忽略停机标记。' : 'The marker exists but its details are invalid or unreadable; the halt remains in effect.'}</p>}
        </div>)}
        <dl className="grid gap-3 sm:grid-cols-2 lg:grid-cols-4 text-sm">
          {[
            [zh ? '网络 / 节点角色' : 'Network / node role', `${show(report.network?.name)} / ${monitorValue('roles', report.node_role, t)}`],
            [t('fields.chainId'), show(report.network?.chain_id)],
            [zh ? '版本' : 'Release', show(report.release_id)],
            [zh ? '后台启动任务' : 'Bootstrap controller', monitorValue('controllers', report.controller?.runtime_state ?? report.controller?.state, t)],
            [zh ? '资源分配' : 'Resource allocation', `${monitorValue('resourceModes', report.resources?.mode, t)} / ${monitorValue('resourcePhases', report.resources?.phase, t)}`],
            [zh ? '资源切换' : 'Resource transition', report.resources?.transition_pending == null ? '—' : report.resources.transition_pending ? (zh ? '进行中' : 'Pending') : (zh ? '无待切换' : 'No pending transition')],
            [zh ? '挖矿状态' : 'Mining', monitorValue('states', report.mining?.state, t)],
          ].map(([label, value]) => <div key={label}><dt className="text-[color:var(--cp-muted)]">{label}</dt><dd className="mt-1 break-all">{value}</dd></div>)}
        </dl>
        <p className="break-all text-xs">{zh ? '创世区块哈希' : 'Genesis block hash'}: {show(report.network?.genesis_hash)}</p>
        {report.resources?.configured_limits_bytes && <details className="text-sm">
          <summary>{zh ? '容器内存上限（配置值，并非实时占用）' : 'Container memory ceilings (configured, not live usage)'}</summary>
          <dl className="mt-3 grid gap-2 sm:grid-cols-2">{Object.entries(report.resources.configured_limits_bytes).map(([name, bytes]) => <div key={name}><dt title={name}>{serviceTitle(name, t)}</dt><dd>{(bytes / 1024 ** 3).toFixed(2)} GiB</dd></div>)}</dl>
        </details>}
        <div className="grid gap-4 lg:grid-cols-2">
          {report.components?.map(component => {
            const title = componentTitle(component.id, t, component.label)
            const state = component.display_state ?? component.state
            const current = fresh && !component.observation_unavailable && !['STALE', 'UNAVAILABLE'].includes(state)
            const displayedState = !fresh ? 'STALE' : component.observation_unavailable && state !== 'STALE' ? 'UNAVAILABLE' : state
            const observation = report.observations?.services[component.id]
            const readiness = observation?.readiness
            const runtime = observation?.runtime
            return <article key={component.id} className="min-w-0 rounded border border-[color:var(--cp-border)] p-4" aria-label={title}>
            <div className="flex flex-wrap justify-between gap-2"><h3 className="font-medium">{title}</h3><strong title={displayedState} className="status-pill" data-tone={!current ? 'neutral' : ['FAILED', 'BLOCKED'].includes(state) ? 'danger' : state === 'READY' ? 'success' : 'neutral'}>{monitorValue('states', displayedState, t)}</strong></div>
            {!current && <p className="mt-2 text-xs text-[color:var(--cp-muted)]">{t('monitor.historical')}</p>}
            {readiness && <div className="mt-2 text-sm">
              <p>{zh ? '可查询 / 共识就绪' : 'Query / consensus ready'}: {show(readiness.query_ready)} / {show(readiness.consensus_ready)}</p>
              {!!readiness.blockers?.length && <p className="break-words">{zh ? '未就绪原因' : 'Readiness blockers'}: {readiness.blockers.join(', ')}</p>}
              {readiness.failure && <p>{zh ? '就绪状态未知' : 'Readiness unknown'}: {readiness.failure.code}</p>}
            </div>}
            {runtime?.details_available && <p className="mt-2 text-sm">
              {zh ? '容器累计重启' : 'Container restarts'}: {show(runtime.restart_count)} · OOM: {show(runtime.oom_killed)}
            </p>}
            {component.progress_phase && <p className="mt-2 text-sm">{t(current ? 'monitor.phase' : 'monitor.lastPhase')}{zh ? '：' : ': '}{monitorValue('phases', component.progress_phase, t)}</p>}
            {component.current != null && <p className="mt-2 break-words">{progressCounter(component.current, component.total, component.unit, locale, t)}</p>}
            {component.progress_percent != null && <progress className="monitor-progress mt-2 w-full" data-current={current} aria-label={`${title} ${t('monitor.progress')}`} value={component.progress_percent} max={100} />}
            {component.state === 'SKIPPED' && <p className="mt-2 text-sm">{t('monitor.skipped')}</p>}
            {component.file_preparation?.state && <p className="mt-2 text-sm">{zh ? '快照文件' : 'Snapshot file'}: {monitorValue('snapshotFile', component.file_preparation.state, t)}</p>}
            {component.background_validation?.target != null && <div className="mt-3 border-t border-[color:var(--cp-border)] pt-3 text-sm">
              <p>{t('monitor.history')}</p>
              <p>{component.background_validation.available && component.background_validation.validated
                ? t('monitor.historyDone', '', { height: component.background_validation.target.toLocaleString(locale) })
                : component.background_validation.available
                  ? `${t('monitor.historyHeight')}：${component.background_validation.height?.toLocaleString(locale) ?? '—'} / ${component.background_validation.target.toLocaleString(locale)} · ${t('monitor.historyPending')}`
                  : (zh ? '暂无观测' : 'No observation')}</p>
              <p className="mt-1 text-xs text-[color:var(--cp-muted)]">{t('monitor.historyHint')}</p>
            </div>}
            {['FAILED', 'BLOCKED'].includes(component.state) && <p className="mt-2 text-sm">{zh ? '请在节点查看 status、doctor 和对应服务日志。' : 'Inspect node status, doctor, and the corresponding service logs.'}</p>}
          </article>})}
        </div>
      </>}
      <HostResources data={report?.host_resources} fresh={fresh} ageMs={age} observedAt={report?.observed_at_ms} />
      <MintingBackend snapshot={snapshot} />
    </section>
  )
}
