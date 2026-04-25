import React from 'react'
import ReactDOM from 'react-dom/client'
import { Activity, BarChart3, Languages, RefreshCw, Search, Server, Table2 } from 'lucide-react'
import './index.css'

type Locale = 'en' | 'zh-CN'
type QueryMode = 'latest' | 'height' | 'range'

interface AddressBalanceRow {
  block_height: number
  balance: number
  delta: number
}

interface BalanceHistorySyncStatus {
  phase: string
  current: number
  total: number
  message?: string | null
}

const CONTROL_PLANE_RPC_URL = '/api/services/balance-history/rpc'
const DEFAULT_RPC_URL = 'http://127.0.0.1:28010'
const localeStorageKey = 'usdb.balance-history-browser.locale.v2'

const dictionaries: Record<Locale, Record<string, string>> = {
  en: {
    language: 'Language',
    kicker: 'USDB Tooling',
    title: 'Balance History Explorer',
    subtitle:
      'A React workspace for balance-history RPC: service status, single address lookup, trend charts, and batch summaries.',
    serviceStatus: 'Service Status',
    singleAddress: 'Single Address',
    batchSummary: 'Batch Summary',
    rpcConnection: 'RPC Connection',
    rpcEndpoint: 'RPC Endpoint',
    rpcHint: 'Use same-origin proxy in the console, or point this page at a standalone balance-history RPC endpoint.',
    connect: 'Connect',
    refresh: 'Refresh',
    network: 'Network',
    syncedHeight: 'Synced Height',
    syncPhase: 'Sync Phase',
    rpcLatency: 'RPC Latency',
    serviceHealth: 'Service Health',
    waiting: 'Waiting for data...',
    current: 'Current',
    total: 'Total',
    phase: 'Phase',
    queryWorkspace: 'Query Workspace',
    scriptHash: 'Script Hash',
    scriptHashPlaceholder: 'Enter USDBScriptHash',
    height: 'Height',
    range: 'Range',
    latest: 'Latest',
    query: 'Query',
    balanceTrend: 'Address Balance Trend',
    deltaDistribution: 'Delta Distribution',
    queryResults: 'Query Results',
    blockHeight: 'Block Height',
    deltaSat: 'Delta (sat)',
    balanceSat: 'Balance (sat)',
    batchQuery: 'Batch Query',
    scriptHashes: 'Script Hashes (one per line)',
    scriptHashesPlaceholder: 'One USDBScriptHash per line',
    records: 'Records',
    latestHeight: 'Latest Height',
    latestBalance: 'Latest Balance',
    netDelta: 'Net Delta',
    noData: 'No data',
    connected: 'Connected. Last refresh: {{time}}',
    connectFailed: 'Connection failed: {{error}}',
    rpcSwitched: 'RPC switched: {{url}}',
    querySuccess: 'Query completed.',
    queryFailed: 'Query failed: {{error}}',
    batchFailed: 'Batch query failed: {{error}}',
    heightRequired: 'A valid height is required in Height mode.',
    rangeRequired: 'A valid range is required in Range mode, and end must be greater than start.',
    scriptHashRequired: 'Enter a Script Hash first.',
    batchRequired: 'Enter at least one Script Hash.',
    singleSummary:
      '{{count}} records, latest height {{height}}, latest balance {{balance}} sat, net delta {{delta}} sat',
    batchTotal: '{{count}} addresses, latest balance total {{balance}} sat, net delta {{delta}} sat',
  },
  'zh-CN': {
    language: '语言',
    kicker: 'USDB 工具',
    title: 'Balance History Explorer',
    subtitle:
      '面向 balance-history RPC 的 React 工作台：状态监控、单地址查询、趋势图表和批量汇总。',
    serviceStatus: '服务状态',
    singleAddress: '单地址查询',
    batchSummary: '批量汇总',
    rpcConnection: 'RPC 连接',
    rpcEndpoint: 'RPC Endpoint',
    rpcHint: '在控制台内默认使用同源代理，也可以指向独立 balance-history RPC endpoint。',
    connect: '连接',
    refresh: '刷新',
    network: '网络类型',
    syncedHeight: '同步高度',
    syncPhase: '同步阶段',
    rpcLatency: 'RPC 延迟',
    serviceHealth: '服务健康',
    waiting: '等待加载...',
    current: '当前进度',
    total: '进度上限',
    phase: '阶段',
    queryWorkspace: '查询工作台',
    scriptHash: 'Script Hash',
    scriptHashPlaceholder: '输入 USDBScriptHash',
    height: 'Height',
    range: 'Range',
    latest: 'Latest',
    query: '查询',
    balanceTrend: '地址余额趋势',
    deltaDistribution: '变化量分布',
    queryResults: '查询结果',
    blockHeight: 'Block Height',
    deltaSat: 'Delta (sat)',
    balanceSat: 'Balance (sat)',
    batchQuery: '批量查询',
    scriptHashes: 'Script Hashes（每行一个）',
    scriptHashesPlaceholder: '每行一个 USDBScriptHash',
    records: 'Records',
    latestHeight: 'Latest Height',
    latestBalance: 'Latest Balance',
    netDelta: 'Net Delta',
    noData: '无数据',
    connected: '连接正常，最后刷新：{{time}}',
    connectFailed: '连接失败：{{error}}',
    rpcSwitched: '已切换 RPC: {{url}}',
    querySuccess: '查询成功',
    queryFailed: '查询失败：{{error}}',
    batchFailed: '批量查询失败：{{error}}',
    heightRequired: 'Height 模式下请填写有效的 height',
    rangeRequired: 'Range 模式下请填写合法区间，且 end > start',
    scriptHashRequired: '请先输入 Script Hash',
    batchRequired: '请至少输入一个 Script Hash',
    singleSummary:
      '记录 {{count}} 条，最新高度 {{height}}，最新余额 {{balance}} sat，区间净变化 {{delta}} sat',
    batchTotal: '共 {{count}} 个地址，最新余额合计 {{balance}} sat，区间净变化 {{delta}} sat',
  },
}

function normalizeLocale(locale?: string | null): Locale {
  if (!locale) return 'en'
  return locale === 'zh-CN' || locale.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en'
}

function isHostedByControlPlane() {
  return window.location.pathname.includes('/explorers/balance-history')
}

function defaultRpcUrl() {
  return isHostedByControlPlane() ? CONTROL_PLANE_RPC_URL : DEFAULT_RPC_URL
}

function readInitialRpcUrl() {
  const params = new URLSearchParams(window.location.search)
  return (params.get('rpc_url') || params.get('rpc') || defaultRpcUrl()).trim()
}

function readInitialLocale() {
  const params = new URLSearchParams(window.location.search)
  return normalizeLocale(
    params.get('lang') || window.localStorage.getItem(localeStorageKey) || window.navigator.language,
  )
}

function interpolate(template: string, variables: Record<string, string | number> = {}) {
  return template.replace(/\{\{(\w+)\}\}/g, (_, key) => String(variables[key] ?? ''))
}

function decodeRpcPayload(payload: unknown): unknown {
  if (payload && typeof payload === 'object' && 'error' in payload && payload.error) {
    const error = payload.error as { message?: string } | string
    throw new Error(typeof error === 'string' ? error : error.message ?? JSON.stringify(error))
  }
  if (payload && typeof payload === 'object' && 'result' in payload) {
    return payload.result
  }
  return payload
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
}

function buildSelector(mode: QueryMode, height: string, start: string, end: string, t: (key: string) => string) {
  if (mode === 'height') {
    const blockHeight = Number(height)
    if (!Number.isFinite(blockHeight) || blockHeight < 0) {
      throw new Error(t('heightRequired'))
    }
    return { block_height: blockHeight, block_range: null }
  }

  if (mode === 'range') {
    const rangeStart = Number(start)
    const rangeEnd = Number(end)
    if (
      !Number.isFinite(rangeStart) ||
      !Number.isFinite(rangeEnd) ||
      rangeStart < 0 ||
      rangeEnd <= rangeStart
    ) {
      throw new Error(t('rangeRequired'))
    }
    return { block_height: null, block_range: { start: rangeStart, end: rangeEnd } }
  }

  return { block_height: null, block_range: null }
}

function summarizeRows(rows: AddressBalanceRow[]) {
  if (rows.length === 0) return null
  const latest = rows[rows.length - 1]
  const net = rows.reduce((acc, row) => acc + row.delta, 0)
  return { count: rows.length, latestHeight: latest.block_height, latestBalance: latest.balance, net }
}

function LineChart({ rows, locale, emptyText }: { rows: AddressBalanceRow[]; locale: Locale; emptyText: string }) {
  const points = React.useMemo(() => {
    if (rows.length === 0) return ''
    const values = rows.map((row) => row.balance)
    const min = Math.min(...values)
    const max = Math.max(...values)
    return rows
      .map((row, index) => {
        const x = 28 + (index / Math.max(rows.length - 1, 1)) * 344
        const y = 152 - ((row.balance - min) / Math.max(max - min, 1)) * 116
        return `${x.toFixed(1)},${y.toFixed(1)}`
      })
      .join(' ')
  }, [rows])

  if (!points) {
    return <div className="chart-empty">{emptyText}</div>
  }

  const latest = rows[rows.length - 1]
  return (
    <svg className="chart" viewBox="0 0 400 180" role="img">
      <path d="M28 152 H372" className="chart-axis" />
      <polyline points={points} className="chart-line" />
      <text x="28" y="24" className="chart-label">
        {new Intl.NumberFormat(locale).format(latest.balance)} sat
      </text>
    </svg>
  )
}

function DeltaChart({ rows, emptyText }: { rows: AddressBalanceRow[]; emptyText: string }) {
  if (rows.length === 0) {
    return <div className="chart-empty">{emptyText}</div>
  }

  const maxAbs = Math.max(...rows.map((row) => Math.abs(row.delta)), 1)
  const barWidth = Math.max(4, 344 / rows.length - 2)
  return (
    <svg className="chart" viewBox="0 0 400 180" role="img">
      <path d="M28 90 H372" className="chart-axis" />
      {rows.map((row, index) => {
        const height = (Math.abs(row.delta) / maxAbs) * 62
        const x = 28 + index * (344 / rows.length)
        const y = row.delta >= 0 ? 90 - height : 90
        return (
          <rect
            key={`${row.block_height}-${index}`}
            className={row.delta >= 0 ? 'chart-bar-positive' : 'chart-bar-negative'}
            x={x}
            y={y}
            width={barWidth}
            height={Math.max(height, 1)}
            rx="2"
          />
        )
      })}
    </svg>
  )
}

function App() {
  const [locale, setLocale] = React.useState<Locale>(readInitialLocale)
  const [rpcUrl, setRpcUrl] = React.useState(readInitialRpcUrl)
  const [rpcDraft, setRpcDraft] = React.useState(readInitialRpcUrl)
  const [status, setStatus] = React.useState<BalanceHistorySyncStatus | null>(null)
  const [network, setNetwork] = React.useState('-')
  const [height, setHeight] = React.useState<number | null>(null)
  const [latency, setLatency] = React.useState<string>('-')
  const [rpcHint, setRpcHint] = React.useState('')
  const [singleMode, setSingleMode] = React.useState<QueryMode>('latest')
  const [batchMode, setBatchMode] = React.useState<QueryMode>('latest')
  const [singleScriptHash, setSingleScriptHash] = React.useState('')
  const [singleHeight, setSingleHeight] = React.useState('')
  const [singleStart, setSingleStart] = React.useState('')
  const [singleEnd, setSingleEnd] = React.useState('')
  const [singleRows, setSingleRows] = React.useState<AddressBalanceRow[]>([])
  const [singleHint, setSingleHint] = React.useState('')
  const [batchScriptHashes, setBatchScriptHashes] = React.useState('')
  const [batchHeight, setBatchHeight] = React.useState('')
  const [batchStart, setBatchStart] = React.useState('')
  const [batchEnd, setBatchEnd] = React.useState('')
  const [batchRows, setBatchRows] = React.useState<AddressBalanceRow[][]>([])
  const [batchHint, setBatchHint] = React.useState('')

  const dict = dictionaries[locale]
  const t = React.useCallback((key: string, variables?: Record<string, string | number>) => {
    return interpolate(dictionaries[locale][key] ?? dictionaries.en[key] ?? key, variables)
  }, [locale])
  const nf = React.useMemo(() => new Intl.NumberFormat(locale), [locale])

  React.useEffect(() => {
    document.documentElement.lang = locale
    window.localStorage.setItem(localeStorageKey, locale)
    const params = new URLSearchParams(window.location.search)
    params.set('lang', locale)
    const query = params.toString()
    window.history.replaceState(null, '', `${window.location.pathname}${query ? `?${query}` : ''}${window.location.hash}`)
  }, [locale])

  React.useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    if (rpcUrl && rpcUrl !== defaultRpcUrl()) {
      params.set('rpc_url', rpcUrl)
    } else {
      params.delete('rpc_url')
    }
    const query = params.toString()
    window.history.replaceState(null, '', `${window.location.pathname}${query ? `?${query}` : ''}${window.location.hash}`)
  }, [rpcUrl])

  const rpcCall = React.useCallback(async <T,>(method: string, params: unknown[] = []): Promise<T> => {
    const startedAt = performance.now()
    const response = await fetch(rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', method, params, id: Date.now() }),
    })
    setLatency(`${Math.round(performance.now() - startedAt)} ms`)
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    return decodeRpcPayload(await response.json()) as T
  }, [rpcUrl])

  const refreshStatus = React.useCallback(async () => {
    try {
      const [nextNetwork, nextHeight, nextStatus] = await Promise.all([
        rpcCall<string>('get_network_type'),
        rpcCall<number>('get_block_height'),
        rpcCall<BalanceHistorySyncStatus>('get_sync_status'),
      ])
      setNetwork(String(nextNetwork))
      setHeight(nextHeight)
      setStatus(nextStatus)
      setRpcHint(t('connected', { time: new Date().toLocaleTimeString(locale) }))
    } catch (error) {
      setRpcHint(t('connectFailed', { error: errorMessage(error) }))
    }
  }, [locale, rpcCall, t])

  React.useEffect(() => {
    void refreshStatus()
    const timer = window.setInterval(() => void refreshStatus(), 5000)
    return () => window.clearInterval(timer)
  }, [refreshStatus])

  async function handleRpcSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const next = rpcDraft.trim()
    if (!next) return
    setRpcUrl(next)
    setRpcHint(t('rpcSwitched', { url: next }))
  }

  async function runSingleQuery(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    try {
      if (!singleScriptHash.trim()) throw new Error(t('scriptHashRequired'))
      const selector = buildSelector(singleMode, singleHeight, singleStart, singleEnd, t)
      const rows = await rpcCall<AddressBalanceRow[]>('get_address_balance', [
        { script_hash: singleScriptHash.trim(), ...selector },
      ])
      setSingleRows(Array.isArray(rows) ? rows : [])
      setSingleHint(t('querySuccess'))
    } catch (error) {
      setSingleRows([])
      setSingleHint(t('queryFailed', { error: errorMessage(error) }))
    }
  }

  async function runBatchQuery(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    try {
      const scriptHashes = batchScriptHashes.split('\n').map((item) => item.trim()).filter(Boolean)
      if (scriptHashes.length === 0) throw new Error(t('batchRequired'))
      const selector = buildSelector(batchMode, batchHeight, batchStart, batchEnd, t)
      const rows = await rpcCall<AddressBalanceRow[][]>('get_addresses_balances', [
        { script_hashes: scriptHashes, ...selector },
      ])
      setBatchRows(Array.isArray(rows) ? rows : [])
      setBatchHint('')
    } catch (error) {
      setBatchRows([])
      setBatchHint(t('batchFailed', { error: errorMessage(error) }))
    }
  }

  const singleSummary = summarizeRows(singleRows)
  const batchItems = batchScriptHashes
    .split('\n')
    .map((item) => item.trim())
    .filter(Boolean)
    .map((hash, index) => {
      const rows = batchRows[index] ?? []
      const latest = rows[rows.length - 1]
      return {
        hash,
        records: rows.length,
        latestHeight: latest?.block_height ?? 0,
        latestBalance: latest?.balance ?? 0,
        net: rows.reduce((acc, row) => acc + row.delta, 0),
      }
    })
  const batchTotal =
    batchItems.length > 0
      ? t('batchTotal', {
          count: nf.format(batchItems.length),
          balance: nf.format(batchItems.reduce((acc, item) => acc + item.latestBalance, 0)),
          delta: formatDelta(batchItems.reduce((acc, item) => acc + item.net, 0), nf),
        })
      : ''
  const progress = status && status.total > 0 ? Math.min(100, (status.current / status.total) * 100) : 0

  return (
    <main className="explorer-shell">
      <div className="console-noise" />
      <header className="page-intro">
        <section className="masthead">
          <div className="title-block">
            <p className="shell-kicker">{dict.kicker}</p>
            <h1>{dict.title}</h1>
            <p className="subtitle">{dict.subtitle}</p>
          </div>
          <div className="header-actions">
            <label className="toolbar-control">
              <Languages size={15} />
              <span>{dict.language}</span>
              <select value={locale} onChange={(event) => setLocale(normalizeLocale(event.target.value))}>
                <option value="en">English</option>
                <option value="zh-CN">简体中文</option>
              </select>
            </label>
            <div className="hero-tags">
              <span className="status-pill" data-tone="success">{dict.serviceStatus}</span>
              <span className="status-pill" data-tone="info">{dict.singleAddress}</span>
              <span className="status-pill" data-tone="info">{dict.batchSummary}</span>
            </div>
          </div>
        </section>

        <form className="rpc-box endpoint-strip" onSubmit={handleRpcSubmit}>
          <div className="section-title">
            <div>
              <p className="eyebrow">Runtime Endpoint</p>
              <h2>{dict.rpcConnection}</h2>
            </div>
            <span className="pill">JSON-RPC</span>
          </div>
          <label className="endpoint-field">
            {dict.rpcEndpoint}
            <div className="input-row">
              <input value={rpcDraft} onChange={(event) => setRpcDraft(event.target.value)} />
              <button type="submit">{dict.connect}</button>
            </div>
          </label>
          <p className={rpcHint.includes('failed') || rpcHint.includes('失败') ? 'hint negative' : 'hint'}>
            {rpcHint || dict.rpcHint}
          </p>
        </form>
      </header>

      <section className="metric-grid">
        <Metric icon={<Server size={18} />} label={dict.network} value={network} />
        <Metric icon={<Activity size={18} />} label={dict.syncedHeight} value={height == null ? '-' : nf.format(height)} />
        <Metric icon={<RefreshCw size={18} />} label={dict.syncPhase} value={status?.phase ?? '-'} />
        <Metric icon={<BarChart3 size={18} />} label={dict.rpcLatency} value={latency} />
      </section>

      <section className="workspace-grid">
        <article className="card">
          <div className="card-head">
            <div>
              <p className="eyebrow">{dict.serviceHealth}</p>
              <h2>{dict.serviceStatus}</h2>
            </div>
            <button className="ghost" type="button" onClick={() => void refreshStatus()}>{dict.refresh}</button>
          </div>
          <p className="status-message">{status?.message || dict.waiting}</p>
          <div className="progress-wrap"><div className="progress-bar" style={{ width: `${progress.toFixed(2)}%` }} /></div>
          <dl className="kv">
            <dt>{dict.current}</dt><dd>{nf.format(status?.current ?? 0)}</dd>
            <dt>{dict.total}</dt><dd>{nf.format(status?.total ?? 0)}</dd>
            <dt>{dict.phase}</dt><dd>{status?.phase ?? '-'}</dd>
          </dl>
        </article>

        <article className="card">
          <div className="card-head">
            <div>
              <p className="eyebrow">{dict.queryWorkspace}</p>
              <h2>{dict.singleAddress}</h2>
            </div>
            <span className="pill">Single</span>
          </div>
          <form className="form-stack" onSubmit={runSingleQuery}>
            <label>{dict.scriptHash}<input required placeholder={dict.scriptHashPlaceholder} value={singleScriptHash} onChange={(event) => setSingleScriptHash(event.target.value)} /></label>
            <ModePicker mode={singleMode} setMode={setSingleMode} dict={dict} />
            <div className="input-row">
              <input type="number" min="0" placeholder="height" value={singleHeight} onChange={(event) => setSingleHeight(event.target.value)} />
              <input type="number" min="0" placeholder="start" value={singleStart} onChange={(event) => setSingleStart(event.target.value)} />
              <input type="number" min="0" placeholder="end" value={singleEnd} onChange={(event) => setSingleEnd(event.target.value)} />
            </div>
            <button type="submit"><Search size={16} />{dict.query}</button>
          </form>
          {singleHint ? <p className={singleHint.includes('failed') || singleHint.includes('失败') ? 'hint negative' : 'hint'}>{singleHint}</p> : null}
        </article>
      </section>

      <section className="workspace-grid">
        <article className="card">
          <div className="card-head"><h2>{dict.balanceTrend}</h2></div>
          <LineChart rows={singleRows} locale={locale} emptyText={dict.noData} />
        </article>
        <article className="card">
          <div className="card-head"><h2>{dict.deltaDistribution}</h2></div>
          <DeltaChart rows={singleRows} emptyText={dict.noData} />
        </article>
      </section>

      <article className="card">
        <div className="card-head">
          <div>
            <p className="eyebrow">Address Records</p>
            <h2>{dict.queryResults}</h2>
          </div>
          <p className="hint">
            {singleSummary
              ? t('singleSummary', {
                  count: nf.format(singleSummary.count),
                  height: nf.format(singleSummary.latestHeight),
                  balance: nf.format(singleSummary.latestBalance),
                  delta: formatDelta(singleSummary.net, nf),
                })
              : dict.noData}
          </p>
        </div>
        <div className="table-wrap">
          <table>
            <thead><tr><th>{dict.blockHeight}</th><th>{dict.deltaSat}</th><th>{dict.balanceSat}</th></tr></thead>
            <tbody>
              {singleRows.map((row) => (
                <tr key={row.block_height}>
                  <td>{nf.format(row.block_height)}</td>
                  <td className={row.delta >= 0 ? 'positive' : 'negative'}>{formatDelta(row.delta, nf)}</td>
                  <td>{nf.format(row.balance)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </article>

      <article className="card">
        <div className="card-head">
          <div>
            <p className="eyebrow">Batch Workspace</p>
            <h2>{dict.batchQuery}</h2>
          </div>
          <span className="pill">Batch</span>
        </div>
        <form className="form-stack" onSubmit={runBatchQuery}>
          <label>{dict.scriptHashes}<textarea rows={5} placeholder={dict.scriptHashesPlaceholder} value={batchScriptHashes} onChange={(event) => setBatchScriptHashes(event.target.value)} /></label>
          <ModePicker mode={batchMode} setMode={setBatchMode} dict={dict} />
          <div className="input-row">
            <input type="number" min="0" placeholder="height" value={batchHeight} onChange={(event) => setBatchHeight(event.target.value)} />
            <input type="number" min="0" placeholder="start" value={batchStart} onChange={(event) => setBatchStart(event.target.value)} />
            <input type="number" min="0" placeholder="end" value={batchEnd} onChange={(event) => setBatchEnd(event.target.value)} />
            <button type="submit"><Table2 size={16} />{dict.batchQuery}</button>
          </div>
        </form>
        <p className={batchHint ? 'hint negative' : 'hint'}>{batchHint || batchTotal}</p>
        <div className="table-wrap">
          <table>
            <thead><tr><th>{dict.scriptHash}</th><th>{dict.records}</th><th>{dict.latestHeight}</th><th>{dict.latestBalance}</th><th>{dict.netDelta}</th></tr></thead>
            <tbody>
              {batchItems.map((item) => (
                <tr key={item.hash}>
                  <td className="mono">{item.hash}</td>
                  <td>{nf.format(item.records)}</td>
                  <td>{nf.format(item.latestHeight)}</td>
                  <td>{nf.format(item.latestBalance)}</td>
                  <td className={item.net >= 0 ? 'positive' : 'negative'}>{formatDelta(item.net, nf)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      </article>
    </main>
  )
}

function Metric({ icon, label, value }: { icon: React.ReactNode; label: string; value: string }) {
  return (
    <article className="card metric-card">
      <div className="metric-icon">{icon}</div>
      <h2>{label}</h2>
      <p className="metric-value">{value}</p>
    </article>
  )
}

function ModePicker({
  mode,
  setMode,
  dict,
}: {
  mode: QueryMode
  setMode: (mode: QueryMode) => void
  dict: Record<string, string>
}) {
  return (
    <div className="inline-options">
      {(['latest', 'height', 'range'] as QueryMode[]).map((item) => (
        <label key={item}>
          <input type="radio" checked={mode === item} onChange={() => setMode(item)} />
          {dict[item]}
        </label>
      ))}
    </div>
  )
}

function formatDelta(value: number, nf: Intl.NumberFormat) {
  const sign = value >= 0 ? '+' : '-'
  return `${sign}${nf.format(Math.abs(value))}`
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
