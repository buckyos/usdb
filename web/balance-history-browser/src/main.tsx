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

interface AddressAnalysisSummary {
  count: number
  latestHeight: number
  latestBalance: number
  firstHeight: number
  net: number
  inflow: number
  outflow: number
  peakBalance: number
  peakHeight: number
  lowBalance: number
  lowHeight: number
}

const CONTROL_PLANE_RPC_URL = '/api/services/balance-history/rpc'
const DEFAULT_RPC_URL = 'http://127.0.0.1:28010'
const SATS_PER_BTC = 100_000_000
const BTC_DISPLAY_THRESHOLD_SAT = 1_000_000
const RECORDS_PAGE_SIZE = 20
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
    recent100: 'Last 100 blocks',
    recent1000: 'Last 1,000 blocks',
    recent10000: 'Last 10,000 blocks',
    fullHistory: 'Full history',
    singleQueryHelp:
      'Range mode is the most useful analysis mode: it returns every persisted balance movement in the selected block window.',
    latestQueryHelp: 'Latest mode returns the latest known balance record only.',
    heightQueryHelp: 'Height mode returns the latest balance record at or before the selected block height.',
    addressAnalysis: 'Address Analysis',
    currentBalance: 'Current Balance',
    netChange: 'Net Change',
    totalInflow: 'Total Inflow',
    totalOutflow: 'Total Outflow',
    changeCount: 'Movements',
    activeSpan: 'Active Span',
    peakBalance: 'Peak Balance',
    lowBalance: 'Low Balance',
    blockSpan: '{{start}} -> {{end}}',
    blocks: 'blocks',
    satAtHeight: '@ height {{height}}',
    balanceTrend: 'Balance Movement Trail',
    deltaDistribution: 'Block Net Flow',
    balanceAxis: 'Balance',
    blockAxis: 'Block height',
    netFlowAxis: 'Net flow',
    latestWindow: 'Latest stable view',
    heightWindow: 'At or before block {{height}}',
    rangeWindow: 'Window: blocks [{{start}}, {{end}})',
    noMovements: 'No balance movement in this query window.',
    queryResults: 'Query Results',
    blockHeight: 'Block Height',
    direction: 'Direction',
    received: 'Received',
    spent: 'Spent',
    unchanged: 'Unchanged',
    deltaSat: 'Delta',
    balanceSat: 'Balance',
    previous: 'Previous',
    next: 'Next',
    pageStatus: 'Page {{page}} / {{total}}',
    newestFirst: 'Newest blocks first',
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
      '{{count}} records, latest height {{height}}, latest balance {{balance}}, net delta {{delta}}',
    batchTotal: '{{count}} addresses, latest balance total {{balance}}, net delta {{delta}}',
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
    recent100: '最近 100 个区块',
    recent1000: '最近 1,000 个区块',
    recent10000: '最近 10,000 个区块',
    fullHistory: '全历史',
    singleQueryHelp: 'Range 模式最适合分析：它会返回选定区块窗口内每一次已记录的余额变动。',
    latestQueryHelp: 'Latest 模式只返回当前最新的一条余额记录。',
    heightQueryHelp: 'Height 模式返回指定区块高度之前或等于该高度的最新余额记录。',
    addressAnalysis: '地址分析',
    currentBalance: '当前余额',
    netChange: '区间净变化',
    totalInflow: '总流入',
    totalOutflow: '总流出',
    changeCount: '变动次数',
    activeSpan: '活跃跨度',
    peakBalance: '余额峰值',
    lowBalance: '余额低点',
    blockSpan: '{{start}} -> {{end}}',
    blocks: '区块',
    satAtHeight: '@ 高度 {{height}}',
    balanceTrend: '余额变化轨迹',
    deltaDistribution: '区块净流入/流出',
    balanceAxis: '余额',
    blockAxis: '区块高度',
    netFlowAxis: '净流入/流出',
    latestWindow: '最新稳定视图',
    heightWindow: '高度 {{height}} 或之前',
    rangeWindow: '区间：[{{start}}, {{end}})',
    noMovements: '当前查询窗口内没有余额变动。',
    queryResults: '查询结果',
    blockHeight: 'Block Height',
    direction: '方向',
    received: '收入',
    spent: '支出',
    unchanged: '无变化',
    deltaSat: '变化量',
    balanceSat: '余额',
    previous: '上一页',
    next: '下一页',
    pageStatus: '第 {{page}} / {{total}} 页',
    newestFirst: '按最新区块倒序',
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
      '记录 {{count}} 条，最新高度 {{height}}，最新余额 {{balance}}，区间净变化 {{delta}}',
    batchTotal: '共 {{count}} 个地址，最新余额合计 {{balance}}，区间净变化 {{delta}}',
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

function analyzeRows(rows: AddressBalanceRow[]): AddressAnalysisSummary | null {
  if (rows.length === 0) return null

  const latest = rows[rows.length - 1]
  return rows.reduce<AddressAnalysisSummary>(
    (summary, row) => ({
      count: summary.count + 1,
      latestHeight: latest.block_height,
      latestBalance: latest.balance,
      firstHeight: Math.min(summary.firstHeight, row.block_height),
      net: summary.net + row.delta,
      inflow: summary.inflow + Math.max(row.delta, 0),
      outflow: summary.outflow + Math.max(-row.delta, 0),
      peakBalance: row.balance > summary.peakBalance ? row.balance : summary.peakBalance,
      peakHeight: row.balance > summary.peakBalance ? row.block_height : summary.peakHeight,
      lowBalance: row.balance < summary.lowBalance ? row.balance : summary.lowBalance,
      lowHeight: row.balance < summary.lowBalance ? row.block_height : summary.lowHeight,
    }),
    {
      count: 0,
      latestHeight: latest.block_height,
      latestBalance: latest.balance,
      firstHeight: rows[0].block_height,
      net: 0,
      inflow: 0,
      outflow: 0,
      peakBalance: rows[0].balance,
      peakHeight: rows[0].block_height,
      lowBalance: rows[0].balance,
      lowHeight: rows[0].block_height,
    },
  )
}

function movementTone(delta: number) {
  if (delta > 0) return 'positive'
  if (delta < 0) return 'negative'
  return 'neutral'
}

function movementLabel(delta: number, dict: Record<string, string>) {
  if (delta > 0) return dict.received
  if (delta < 0) return dict.spent
  return dict.unchanged
}

function formatAmount(value: number, nf: Intl.NumberFormat, signed = false) {
  const sign = signed && value !== 0 ? (value > 0 ? '+' : '-') : ''
  const abs = Math.abs(value)
  if (abs >= BTC_DISPLAY_THRESHOLD_SAT) {
    const formatter = new Intl.NumberFormat(nf.resolvedOptions().locale, {
      maximumFractionDigits: 8,
      minimumFractionDigits: 0,
    })
    return `${sign}${formatter.format(abs / SATS_PER_BTC)} BTC`
  }
  return `${sign}${nf.format(abs)} sat`
}

function describeQueryWindow({
  mode,
  height,
  start,
  end,
  dict,
  t,
}: {
  mode: QueryMode
  height: string
  start: string
  end: string
  dict: Record<string, string>
  t: (key: string, variables?: Record<string, string | number>) => string
}) {
  if (mode === 'height') return height ? t('heightWindow', { height }) : dict.heightQueryHelp
  if (mode === 'range') return start && end ? t('rangeWindow', { start, end }) : dict.singleQueryHelp
  return dict.latestWindow
}

function queryHelp(mode: QueryMode, dict: Record<string, string>) {
  if (mode === 'height') return dict.heightQueryHelp
  if (mode === 'range') return dict.singleQueryHelp
  return dict.latestQueryHelp
}

function LineChart({
  rows,
  nf,
  emptyText,
  xLabel,
  yLabel,
}: {
  rows: AddressBalanceRow[]
  nf: Intl.NumberFormat
  emptyText: string
  xLabel: string
  yLabel: string
}) {
  const points = React.useMemo(() => {
    if (rows.length === 0) return []
    const values = rows.map((row) => row.balance)
    const min = Math.min(...values)
    const max = Math.max(...values)
    return rows
      .map((row, index) => {
        const x = rows.length === 1 ? 206 : 28 + (index / Math.max(rows.length - 1, 1)) * 350
        const y = 138 - ((row.balance - min) / Math.max(max - min, 1)) * 98
        return { x, y, row }
      })
  }, [rows])

  if (points.length === 0) {
    return <div className="chart-empty">{emptyText}</div>
  }

  const first = rows[0]
  const latest = rows[rows.length - 1]
  return (
    <svg className="chart" viewBox="0 0 400 180" role="img">
      <path d="M28 138 H378" className="chart-axis" />
      <path d="M28 40 V138" className="chart-axis" />
      <polyline points={points.map((point) => `${point.x.toFixed(1)},${point.y.toFixed(1)}`).join(' ')} className="chart-line" />
      {points.map((point) => (
        <circle key={`${point.row.block_height}-${point.row.balance}`} cx={point.x} cy={point.y} r="3.2" className="chart-point" />
      ))}
      <text x="28" y="24" className="chart-label chart-label-strong">{formatAmount(latest.balance, nf)}</text>
      <text x="28" y="36" className="chart-label chart-axis-caption">{yLabel}</text>
      <text x="318" y="166" className="chart-label chart-axis-caption">{xLabel}</text>
      <text x="28" y="156" className="chart-label">{nf.format(first.block_height)}</text>
      <text x="346" y="156" className="chart-label">{nf.format(latest.block_height)}</text>
    </svg>
  )
}

function DeltaChart({
  rows,
  nf,
  emptyText,
  xLabel,
  yLabel,
}: {
  rows: AddressBalanceRow[]
  nf: Intl.NumberFormat
  emptyText: string
  xLabel: string
  yLabel: string
}) {
  if (rows.length === 0) {
    return <div className="chart-empty">{emptyText}</div>
  }

  const first = rows[0]
  const latest = rows[rows.length - 1]
  const maxAbs = Math.max(...rows.map((row) => Math.abs(row.delta)), 1)
  const plotWidth = 350
  const barWidth = Math.max(3, plotWidth / rows.length - 2)
  return (
    <svg className="chart" viewBox="0 0 400 180" role="img">
      <path d="M28 92 H378" className="chart-axis zero-axis" />
      <path d="M28 28 V150" className="chart-axis" />
      {rows.map((row, index) => {
        const height = (Math.abs(row.delta) / maxAbs) * 54
        const x = rows.length === 1 ? 202 : 28 + index * (plotWidth / rows.length)
        const y = row.delta >= 0 ? 92 - height : 92
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
      <text x="28" y="18" className="chart-label chart-label-strong">{formatAmount(maxAbs, nf, true)}</text>
      <text x="28" y="164" className="chart-label chart-label-strong">{formatAmount(-maxAbs, nf, true)}</text>
      <text x="28" y="27" className="chart-label chart-axis-caption">{yLabel}</text>
      <text x="318" y="166" className="chart-label chart-axis-caption">{xLabel}</text>
      <text x="28" y="156" className="chart-label">{nf.format(first.block_height)}</text>
      <text x="346" y="156" className="chart-label">{nf.format(latest.block_height)}</text>
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
  const [singleMode, setSingleMode] = React.useState<QueryMode>('range')
  const [batchMode, setBatchMode] = React.useState<QueryMode>('latest')
  const [singleScriptHash, setSingleScriptHash] = React.useState('')
  const [singleHeight, setSingleHeight] = React.useState('')
  const [singleStart, setSingleStart] = React.useState('')
  const [singleEnd, setSingleEnd] = React.useState('')
  const [singleRows, setSingleRows] = React.useState<AddressBalanceRow[]>([])
  const [singlePage, setSinglePage] = React.useState(0)
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

  React.useEffect(() => {
    if (height == null || singleStart || singleEnd) return
    const rangeEnd = height + 1
    setSingleStart(String(Math.max(0, rangeEnd - 1000)))
    setSingleEnd(String(rangeEnd))
  }, [height, singleEnd, singleStart])

  async function handleRpcSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const next = rpcDraft.trim()
    if (!next) return
    setRpcUrl(next)
    setRpcHint(t('rpcSwitched', { url: next }))
  }

  function applySingleRange(blocks: number | 'all') {
    const currentHeight = height ?? 0
    const rangeEnd = currentHeight + 1
    setSingleMode('range')
    setSingleStart(blocks === 'all' ? '0' : String(Math.max(0, rangeEnd - blocks)))
    setSingleEnd(String(rangeEnd))
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
      setSinglePage(0)
      setSingleHint(t('querySuccess'))
    } catch (error) {
      setSingleRows([])
      setSinglePage(0)
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
  const singleAnalysis = analyzeRows(singleRows)
  const sortedSingleRows = React.useMemo(
    () => [...singleRows].sort((left, right) => right.block_height - left.block_height),
    [singleRows],
  )
  const singleTotalPages = Math.max(1, Math.ceil(sortedSingleRows.length / RECORDS_PAGE_SIZE))
  const safeSinglePage = Math.min(singlePage, singleTotalPages - 1)
  const pagedSingleRows = sortedSingleRows.slice(
    safeSinglePage * RECORDS_PAGE_SIZE,
    (safeSinglePage + 1) * RECORDS_PAGE_SIZE,
  )
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
          balance: formatAmount(batchItems.reduce((acc, item) => acc + item.latestBalance, 0), nf),
          delta: formatDelta(batchItems.reduce((acc, item) => acc + item.net, 0), nf),
        })
      : ''
  const progress = status && status.total > 0 ? Math.min(100, (status.current / status.total) * 100) : 0
  const singleWindowLabel = describeQueryWindow({
    mode: singleMode,
    height: singleHeight,
    start: singleStart,
    end: singleEnd,
    dict,
    t,
  })

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
            {singleMode === 'range' ? (
              <div className="range-presets">
                <button className="ghost compact" type="button" onClick={() => applySingleRange(100)}>{dict.recent100}</button>
                <button className="ghost compact" type="button" onClick={() => applySingleRange(1000)}>{dict.recent1000}</button>
                <button className="ghost compact" type="button" onClick={() => applySingleRange(10000)}>{dict.recent10000}</button>
                <button className="ghost compact" type="button" onClick={() => applySingleRange('all')}>{dict.fullHistory}</button>
              </div>
            ) : null}
            {singleMode === 'height' ? (
              <div className="input-row">
                <input type="number" min="0" placeholder="height" value={singleHeight} onChange={(event) => setSingleHeight(event.target.value)} />
              </div>
            ) : null}
            {singleMode === 'range' ? (
              <div className="input-row">
                <input type="number" min="0" placeholder="start" value={singleStart} onChange={(event) => setSingleStart(event.target.value)} />
                <input type="number" min="0" placeholder="end" value={singleEnd} onChange={(event) => setSingleEnd(event.target.value)} />
              </div>
            ) : null}
            <button type="submit"><Search size={16} />{dict.query}</button>
          </form>
          <p className="hint">{queryHelp(singleMode, dict)}</p>
          {singleHint ? <p className={singleHint.includes('failed') || singleHint.includes('失败') ? 'hint negative' : 'hint'}>{singleHint}</p> : null}
        </article>
      </section>

      <article className="card">
        <div className="card-head">
          <div>
            <p className="eyebrow">BTC Address Flow</p>
            <h2>{dict.addressAnalysis}</h2>
          </div>
          <p className="hint">{singleAnalysis ? t('blockSpan', { start: nf.format(singleAnalysis.firstHeight), end: nf.format(singleAnalysis.latestHeight) }) : dict.noMovements}</p>
        </div>
        <section className="analysis-grid">
          <AnalysisMetric label={dict.currentBalance} value={singleAnalysis ? formatAmount(singleAnalysis.latestBalance, nf) : '-'} />
          <AnalysisMetric label={dict.netChange} value={singleAnalysis ? formatDelta(singleAnalysis.net, nf) : '-'} tone={singleAnalysis ? movementTone(singleAnalysis.net) : 'neutral'} />
          <AnalysisMetric label={dict.totalInflow} value={singleAnalysis ? formatAmount(singleAnalysis.inflow, nf, true) : '-'} tone="positive" />
          <AnalysisMetric label={dict.totalOutflow} value={singleAnalysis ? formatAmount(-singleAnalysis.outflow, nf, true) : '-'} tone="negative" />
          <AnalysisMetric label={dict.changeCount} value={singleAnalysis ? nf.format(singleAnalysis.count) : '-'} />
          <AnalysisMetric label={dict.activeSpan} value={singleAnalysis ? nf.format(singleAnalysis.latestHeight - singleAnalysis.firstHeight) : '-'} suffix={dict.blocks} />
          <AnalysisMetric label={dict.peakBalance} value={singleAnalysis ? formatAmount(singleAnalysis.peakBalance, nf) : '-'} suffix={singleAnalysis ? t('satAtHeight', { height: nf.format(singleAnalysis.peakHeight) }) : undefined} />
          <AnalysisMetric label={dict.lowBalance} value={singleAnalysis ? formatAmount(singleAnalysis.lowBalance, nf) : '-'} suffix={singleAnalysis ? t('satAtHeight', { height: nf.format(singleAnalysis.lowHeight) }) : undefined} />
        </section>
      </article>

      <section className="workspace-grid">
        <article className="card">
          <div className="card-head">
            <div>
              <p className="eyebrow">{singleWindowLabel}</p>
              <h2>{dict.balanceTrend}</h2>
            </div>
            <p className="hint">{singleRows.length > 0 ? `${nf.format(singleRows.length)} ${dict.records}` : dict.noMovements}</p>
          </div>
          <LineChart rows={singleRows} nf={nf} emptyText={dict.noMovements} xLabel={dict.blockAxis} yLabel={dict.balanceAxis} />
        </article>
        <article className="card">
          <div className="card-head">
            <div>
              <p className="eyebrow">{singleWindowLabel}</p>
              <h2>{dict.deltaDistribution}</h2>
            </div>
            <p className="hint">{singleAnalysis ? t('singleSummary', {
              count: nf.format(singleAnalysis.count),
              height: nf.format(singleAnalysis.latestHeight),
              balance: formatAmount(singleAnalysis.latestBalance, nf),
              delta: formatDelta(singleAnalysis.net, nf),
            }) : dict.noMovements}</p>
          </div>
          <DeltaChart rows={singleRows} nf={nf} emptyText={dict.noMovements} xLabel={dict.blockAxis} yLabel={dict.netFlowAxis} />
        </article>
      </section>

      <article className="card">
        <div className="card-head">
          <div>
            <p className="eyebrow">Address Records</p>
            <h2>{dict.queryResults}</h2>
          </div>
          <div className="table-actions">
            <p className="hint">
              {singleSummary
                ? t('singleSummary', {
                    count: nf.format(singleSummary.count),
                    height: nf.format(singleSummary.latestHeight),
                    balance: formatAmount(singleSummary.latestBalance, nf),
                    delta: formatDelta(singleSummary.net, nf),
                  })
                : dict.noData}
            </p>
            <div className="pager">
              <span>{dict.newestFirst}</span>
              <button className="ghost compact" type="button" disabled={safeSinglePage === 0} onClick={() => setSinglePage((page) => Math.max(0, page - 1))}>{dict.previous}</button>
              <span>{t('pageStatus', { page: safeSinglePage + 1, total: singleTotalPages })}</span>
              <button className="ghost compact" type="button" disabled={safeSinglePage + 1 >= singleTotalPages} onClick={() => setSinglePage((page) => Math.min(singleTotalPages - 1, page + 1))}>{dict.next}</button>
            </div>
          </div>
        </div>
        <div className="table-wrap">
          <table>
            <thead><tr><th>{dict.blockHeight}</th><th>{dict.direction}</th><th>{dict.deltaSat}</th><th>{dict.balanceSat}</th></tr></thead>
            <tbody>
              {pagedSingleRows.map((row) => (
                <tr key={row.block_height}>
                  <td>{nf.format(row.block_height)}</td>
                  <td><span className={`movement ${movementTone(row.delta)}`}>{movementLabel(row.delta, dict)}</span></td>
                  <td className={movementTone(row.delta)}>{formatDelta(row.delta, nf)}</td>
                  <td>{formatAmount(row.balance, nf)}</td>
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
          {batchMode === 'height' ? (
            <div className="input-row">
              <input type="number" min="0" placeholder="height" value={batchHeight} onChange={(event) => setBatchHeight(event.target.value)} />
            </div>
          ) : null}
          {batchMode === 'range' ? (
            <div className="input-row">
              <input type="number" min="0" placeholder="start" value={batchStart} onChange={(event) => setBatchStart(event.target.value)} />
              <input type="number" min="0" placeholder="end" value={batchEnd} onChange={(event) => setBatchEnd(event.target.value)} />
            </div>
          ) : null}
          <div className="input-row submit-row">
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
                  <td>{formatAmount(item.latestBalance, nf)}</td>
                  <td className={movementTone(item.net)}>{formatDelta(item.net, nf)}</td>
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

function AnalysisMetric({
  label,
  value,
  suffix,
  tone = 'neutral',
}: {
  label: string
  value: string
  suffix?: string
  tone?: 'positive' | 'negative' | 'neutral'
}) {
  return (
    <article className="analysis-card" data-tone={tone}>
      <p>{label}</p>
      <strong>{value}</strong>
      {suffix ? <span>{suffix}</span> : null}
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
  return formatAmount(value, nf, true)
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
