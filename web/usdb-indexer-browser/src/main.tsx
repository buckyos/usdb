import React from 'react'
import ReactDOM from 'react-dom/client'
import { Activity, Badge, Clock3, Database, Languages, RefreshCw, Search, Zap } from 'lucide-react'
import './index.css'

type Locale = 'en' | 'zh-CN'
type Tab = 'home' | 'pass' | 'energy'
type EnergyScope = 'active' | 'active_dormant' | 'all'
type PassQueryMode = 'id' | 'owner'
type OwnerPassScope = 'all' | 'active' | 'active_dormant'

interface UsdbRpcInfo {
  network: string
}

interface UsdbIndexerSyncStatus {
  genesis_block_height?: number | null
  synced_block_height?: number | null
  balance_history_stable_height?: number | null
  current: number
  total: number
  message?: string | null
}

interface UsdbIndexerReadiness {
  service: string
  rpc_alive: boolean
  query_ready: boolean
  consensus_ready: boolean
  synced_block_height?: number | null
  balance_history_stable_height?: number | null
  upstream_snapshot_id?: string | null
  local_state_commit?: string | null
  system_state_id?: string | null
  current: number
  total: number
  message?: string | null
  blockers: string[]
}

interface PassStatsAtHeight {
  total_count: number
  active_count: number
}

interface RpcActiveBalanceSnapshot {
  total_balance: number
}

interface PassBlockCommitInfo {
  block_height: number
  balance_history_block_height: number
  balance_history_block_commit: string
  mutation_root: string
  block_commit: string
  commit_protocol_version: string
  commit_hash_algo: string
}

interface PassSnapshot {
  inscription_id: string
  inscription_number: number
  mint_block_height: number
  mint_owner: string
  eth_main: string
  eth_collab?: string | null
  prev: string[]
  invalid_code?: string | null
  invalid_reason?: string | null
  owner: string
  state: string
  satpoint: string
  last_event_id: number
  last_event_type: string
  resolved_height: number
}

interface OwnerPassItem {
  inscription_id: string
  inscription_number: number
  mint_block_height: number
  owner: string
  state: string
  latest_event_height: number
  eth_main: string
  eth_collab?: string | null
  satpoint: string
}

interface OwnerPassesAtHeightPage {
  resolved_height: number
  owner: string
  total: number
  items: OwnerPassItem[]
}

interface PassHistoryEvent {
  event_id: number
  block_height: number
  event_type: string
  state: string
  owner: string
  satpoint: string
}

interface PassHistoryPage {
  total: number
  items: PassHistoryEvent[]
}

interface PassEnergyLeaderboardItem {
  inscription_id: string
  record_block_height: number
  state: string
  energy: number
}

interface PassEnergyLeaderboardPage {
  resolved_height: number
  total: number
  items: PassEnergyLeaderboardItem[]
}

interface PassEnergySnapshot {
  inscription_id: string
  query_block_height: number
  record_block_height: number
  state: string
  active_block_height: number
  owner_address: string
  owner_balance: number
  owner_delta: number
  energy: number
}

interface PassEnergyRangeItem {
  record_block_height: number
  state: string
  active_block_height: number
  owner_address: string
  owner_balance: number
  owner_delta: number
  energy: number
}

interface PassEnergyRangePage {
  total: number
  items: PassEnergyRangeItem[]
}

const CONTROL_PLANE_RPC_URL = '/api/services/usdb-indexer/rpc'
const localeStorageKey = 'usdb.usdb-indexer-browser.locale.v2'
const rpcDefaults = {
  mainnet: 'http://127.0.0.1:28020',
  regtest: 'http://127.0.0.1:28120',
  testnet: 'http://127.0.0.1:28220',
  signet: 'http://127.0.0.1:28320',
  testnet4: 'http://127.0.0.1:28420',
}

const dictionaries: Record<Locale, Record<string, string>> = {
  en: {
    language: 'Language',
    kicker: 'USDB Indexer Observatory',
    title: 'USDB Indexer Browser',
    subtitle: 'A React workspace for USDB protocol state, miner-pass lookup, energy rankings, and timelines.',
    runtimeStatus: 'Runtime Status',
    minerPass: 'Miner Pass',
    energy: 'Energy',
    rpcConnection: 'RPC Connection',
    rpcEndpoint: 'RPC Endpoint',
    connect: 'Connect',
    refresh: 'Refresh',
    home: 'Home',
    currentTime: 'Current Time',
    btcNetwork: 'BTC Network',
    syncedHeight: 'Synced Height',
    stableHeight: 'Stable Height',
    activePasses: 'Active Passes',
    totalPasses: 'Total Passes',
    activeBalance: 'Active BTC Balance',
    syncStatus: 'Sync Status',
    consistencyStatus: 'Consistency Status',
    currentProgress: 'Current Progress',
    progressLimit: 'Progress Limit',
    genesisHeight: 'Genesis Height',
    rpcAlive: 'RPC Alive',
    queryReady: 'Query Ready',
    consensusReady: 'Consensus Ready',
    ready: 'Ready',
    notReady: 'Not Ready',
    upstreamSnapshot: 'Upstream Snapshot',
    localStateCommit: 'Local State Commit',
    systemStateId: 'System State ID',
    blockers: 'Blockers',
    none: 'None',
    rpcLatency: 'RPC Latency',
    updated: 'Updated',
    latestCommit: 'Latest Local Pass Commit',
    openInPass: 'Open in Pass',
    passQuery: 'Miner Pass Query',
    queryByPassId: 'By Pass ID',
    queryByOwner: 'By Owner Address',
    ownerPasses: 'Owner Passes',
    ownerAddress: 'Owner BTC Address / Script Hash',
    ownerAddressPlaceholder: 'Enter BTC address or owner script hash',
    ownerStateScope: 'State Scope',
    allStates: 'All states',
    activeOnly: 'Active only',
    activeDormant: 'Active + dormant',
    openDetail: 'Open Detail',
    passDetail: 'Miner Pass Detail',
    blockCommit: 'Block Commit',
    history: 'History',
    query: 'Query',
    previous: 'Previous',
    next: 'Next',
    energyLeaderboard: 'Energy Leaderboard',
    currentEnergy: 'Current Energy State',
    timeline: 'Record Timeline',
    updateRange: 'Update Range',
    noData: 'No data',
    waitingCommit: 'Waiting for the latest local pass block commit.',
    noCommit: 'No local pass block commit is currently available.',
    connected: 'Connected. Last refresh {{time}}',
    rpcError: 'RPC error: {{error}}',
    homeError: 'Home refresh failed: {{error}}',
    switched: 'RPC switched: {{url}}',
    preset: 'RPC filled from network preset: {{url}}',
    hostedProxy: 'Console-hosted mode uses the same-origin control-plane proxy: {{url}}',
    bitcoind: 'This looks like a bitcoind RPC endpoint, which browsers usually block via CORS. Use a usdb-indexer RPC endpoint.',
    passIdRequired: 'Enter an inscription id.',
    ownerRequired: 'Enter an owner address or script hash.',
    heightInvalid: 'Enter a non-negative integer height.',
    passMissing: 'This miner pass does not exist or is not visible at the target height.',
    querySuccess: 'Query completed.',
    queryFailed: 'Query failed: {{error}}',
    historyFailed: 'History query failed: {{error}}',
    ownerPassesFailed: 'Owner pass query failed: {{error}}',
    ownerPassesSuccess: 'Found {{count}} passes at height {{height}}.',
    commitMissing: 'No local pass block commit exists at the target height.',
    commitSuccess: 'Query completed, height={{height}}.',
    leaderboardFailed: 'Leaderboard failed: {{error}}',
    rangeRequired: 'Fill in from/to heights.',
    rangeInvalid: 'from_height cannot be greater than to_height.',
    rangeFailed: 'Range query failed: {{error}}',
    energySuccess: 'Query completed. timeline_total={{total}}',
    inscriptionPlaceholder: 'Enter inscription id, for example txidi0',
    optionalHeight: 'Optional height',
  },
  'zh-CN': {
    language: '语言',
    kicker: 'USDB 索引观测',
    title: 'USDB Indexer Browser',
    subtitle: '面向 USDB 协议状态、矿工证查询、能量排行与时间线的 React 工作台。',
    runtimeStatus: '运行状态',
    minerPass: '矿工证',
    energy: '能量',
    rpcConnection: 'RPC 连接',
    rpcEndpoint: 'RPC Endpoint',
    connect: '连接',
    refresh: '刷新',
    home: '首页',
    currentTime: '当前时间',
    btcNetwork: 'BTC 网络',
    syncedHeight: '同步高度',
    stableHeight: '稳定高度',
    activePasses: '活跃矿工证',
    totalPasses: '矿工证总量',
    activeBalance: '活跃地址 BTC 总额',
    syncStatus: '同步状态',
    consistencyStatus: '一致性状态',
    currentProgress: '当前进度',
    progressLimit: '进度上限',
    genesisHeight: '创世高度',
    rpcAlive: 'RPC 可用',
    queryReady: '查询可用',
    consensusReady: '共识可用',
    ready: '就绪',
    notReady: '未就绪',
    upstreamSnapshot: '上游 Snapshot',
    localStateCommit: '本地 State Commit',
    systemStateId: '系统 State ID',
    blockers: '阻塞原因',
    none: '无',
    rpcLatency: 'RPC 延迟',
    updated: '更新时间',
    latestCommit: '最近本地 Pass Commit',
    openInPass: '带入 Pass 页',
    passQuery: '矿工证查询',
    queryByPassId: '按矿工证 ID',
    queryByOwner: '按 Owner 地址',
    ownerPasses: '地址下的矿工证',
    ownerAddress: 'Owner BTC 地址 / Script Hash',
    ownerAddressPlaceholder: '输入 BTC 地址或 owner script hash',
    ownerStateScope: '状态范围',
    allStates: '全部状态',
    activeOnly: '仅 Active',
    activeDormant: 'Active + Dormant',
    openDetail: '查看详情',
    passDetail: '矿工证详情',
    blockCommit: '区块 Commit',
    history: '历史记录',
    query: '查询',
    previous: '上一页',
    next: '下一页',
    energyLeaderboard: '能量排行',
    currentEnergy: '当前能量状态',
    timeline: 'Record Timeline',
    updateRange: '更新区间',
    noData: '无数据',
    waitingCommit: '等待刷新最近本地 pass block commit。',
    noCommit: '当前还没有可展示的本地 pass block commit。',
    connected: '连接正常，最后刷新 {{time}}',
    rpcError: 'RPC 异常：{{error}}',
    homeError: '首页刷新失败：{{error}}',
    switched: '已切换 RPC: {{url}}',
    preset: '已按网络预设填充 RPC: {{url}}',
    hostedProxy: '控制台内嵌模式使用同源 control-plane 代理：{{url}}',
    bitcoind: '你输入的是 bitcoind RPC 端口，浏览器会触发 CORS。请使用 usdb-indexer RPC。',
    passIdRequired: '请输入 inscription id。',
    ownerRequired: '请输入 owner 地址或 script hash。',
    heightInvalid: '请输入非负整数高度',
    passMissing: '该矿工证不存在或在目标高度不可见。',
    querySuccess: '查询成功。',
    queryFailed: '查询失败：{{error}}',
    historyFailed: '历史查询失败：{{error}}',
    ownerPassesFailed: 'Owner 矿工证查询失败：{{error}}',
    ownerPassesSuccess: '在高度 {{height}} 找到 {{count}} 个矿工证。',
    commitMissing: '目标高度还没有本地 pass block commit 记录。',
    commitSuccess: '查询成功，高度={{height}}。',
    leaderboardFailed: '排行加载失败：{{error}}',
    rangeRequired: '请填写 from/to 高度',
    rangeInvalid: 'from_height 不能大于 to_height',
    rangeFailed: '区间查询失败：{{error}}',
    energySuccess: '查询成功。timeline_total={{total}}',
    inscriptionPlaceholder: '输入 inscription id（例如 txidi0）',
    optionalHeight: '可选高度',
  },
}

function normalizeLocale(locale?: string | null): Locale {
  if (!locale) return 'en'
  return locale === 'zh-CN' || locale.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en'
}

function isHostedByControlPlane() {
  return window.location.pathname.includes('/explorers/usdb-indexer')
}

function normalizeNetwork(network?: string | null) {
  const raw = String(network || '').toLowerCase()
  if (raw.includes('regtest')) return 'regtest'
  if (raw.includes('testnet4')) return 'testnet4'
  if (raw.includes('testnet')) return 'testnet'
  if (raw.includes('signet')) return 'signet'
  return 'mainnet'
}

function readInitialLocale() {
  const params = new URLSearchParams(window.location.search)
  return normalizeLocale(
    params.get('lang') || window.localStorage.getItem(localeStorageKey) || window.navigator.language,
  )
}

function readInitialRpcUrl() {
  const params = new URLSearchParams(window.location.search)
  const requested = (params.get('rpc_url') || params.get('rpc') || '').trim()
  if (isHostedByControlPlane()) {
    return requested.startsWith('/') ? requested : CONTROL_PLANE_RPC_URL
  }
  return (requested || rpcDefaults.mainnet).trim()
}

function interpolate(template: string, variables: Record<string, string | number> = {}) {
  return template.replace(/\{\{(\w+)\}\}/g, (_, key) => String(variables[key] ?? ''))
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error)
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

function parseOptionalHeight(text: string, t: (key: string) => string) {
  if (!text.trim()) return null
  const value = Number(text)
  if (!Number.isInteger(value) || value < 0) throw new Error(t('heightInvalid'))
  return value
}

function shortText(value: unknown, head = 14, tail = 12) {
  const text = String(value ?? '')
  if (!text) return '-'
  if (text.length <= head + tail + 3) return text
  return `${text.slice(0, head)}...${text.slice(-tail)}`
}

function hashTitle(value?: string | null) {
  return value || ''
}

function formatBtc(value: number | null | undefined, nf: Intl.NumberFormat) {
  if (value === null || value === undefined || Number.isNaN(Number(value))) return '-'
  const sat = Number(value)
  if (Math.abs(sat) >= 100_000_000) return `${(sat / 100_000_000).toFixed(8).replace(/\.?0+$/, '')} BTC`
  return `${nf.format(sat)} sat`
}

function formatDelta(value: number | null | undefined, nf: Intl.NumberFormat) {
  if (value === null || value === undefined || Number.isNaN(Number(value))) return '-'
  const sat = Number(value)
  const sign = sat >= 0 ? '+' : '-'
  return `${sign}${formatBtc(Math.abs(sat), nf)}`
}

function isLikelyBitcoindRpcUrl(rawUrl: string) {
  try {
    const parsed = new URL(rawUrl)
    const port = Number(parsed.port || (parsed.protocol === 'https:' ? 443 : 80))
    return ['127.0.0.1', 'localhost'].includes(parsed.hostname) && [8332, 18332, 18443, 38332, 48332, 28032, 28132].includes(port)
  } catch {
    return false
  }
}

function ownerPassStates(scope: OwnerPassScope) {
  if (scope === 'active') return ['active']
  if (scope === 'active_dormant') return ['active', 'dormant']
  return undefined
}

function App() {
  const [locale, setLocale] = React.useState<Locale>(readInitialLocale)
  const [rpcUrl, setRpcUrl] = React.useState(readInitialRpcUrl)
  const [rpcDraft, setRpcDraft] = React.useState(readInitialRpcUrl)
  const [networkPreset, setNetworkPreset] = React.useState(normalizeNetwork(new URLSearchParams(window.location.search).get('network')))
  const [activeTab, setActiveTab] = React.useState<Tab>('home')
  const [now, setNow] = React.useState(new Date())
  const [latency, setLatency] = React.useState('-')
  const [rpcHint, setRpcHint] = React.useState('')
  const [homeError, setHomeError] = React.useState('')
  const [rpcInfo, setRpcInfo] = React.useState<UsdbRpcInfo | null>(null)
  const [syncStatus, setSyncStatus] = React.useState<UsdbIndexerSyncStatus | null>(null)
  const [readiness, setReadiness] = React.useState<UsdbIndexerReadiness | null>(null)
  const [passStats, setPassStats] = React.useState<PassStatsAtHeight | null>(null)
  const [activeBalance, setActiveBalance] = React.useState<RpcActiveBalanceSnapshot | null>(null)
  const [latestCommit, setLatestCommit] = React.useState<PassBlockCommitInfo | null>(null)
  const [passQueryMode, setPassQueryMode] = React.useState<PassQueryMode>('id')
  const [passId, setPassId] = React.useState('')
  const [passHeight, setPassHeight] = React.useState('')
  const [ownerAddress, setOwnerAddress] = React.useState('')
  const [ownerHeight, setOwnerHeight] = React.useState('')
  const [ownerScope, setOwnerScope] = React.useState<OwnerPassScope>('all')
  const [ownerPasses, setOwnerPasses] = React.useState<OwnerPassesAtHeightPage | null>(null)
  const [ownerPassesPage, setOwnerPassesPage] = React.useState(0)
  const [ownerHint, setOwnerHint] = React.useState('')
  const [passSnapshot, setPassSnapshot] = React.useState<PassSnapshot | null>(null)
  const [passCommit, setPassCommit] = React.useState<PassBlockCommitInfo | null>(null)
  const [passHistory, setPassHistory] = React.useState<PassHistoryPage | null>(null)
  const [passHistoryPage, setPassHistoryPage] = React.useState(0)
  const [passHint, setPassHint] = React.useState('')
  const [commitHeight, setCommitHeight] = React.useState('')
  const [commitHint, setCommitHint] = React.useState('')
  const [scope, setScope] = React.useState<EnergyScope>('active')
  const [leaderboard, setLeaderboard] = React.useState<PassEnergyLeaderboardPage | null>(null)
  const [leaderboardPage, setLeaderboardPage] = React.useState(0)
  const [leaderboardHint, setLeaderboardHint] = React.useState('')
  const [energyId, setEnergyId] = React.useState('')
  const [energySnapshot, setEnergySnapshot] = React.useState<PassEnergySnapshot | null>(null)
  const [rangeFrom, setRangeFrom] = React.useState('')
  const [rangeTo, setRangeTo] = React.useState('')
  const [energyRange, setEnergyRange] = React.useState<PassEnergyRangePage | null>(null)
  const [energyRangePage, setEnergyRangePage] = React.useState(0)
  const [energyHint, setEnergyHint] = React.useState('')
  const [rangeHint, setRangeHint] = React.useState('')

  const dict = dictionaries[locale]
  const t = React.useCallback((key: string, variables?: Record<string, string | number>) => {
    return interpolate(dictionaries[locale][key] ?? dictionaries.en[key] ?? key, variables)
  }, [locale])
  const nf = React.useMemo(() => new Intl.NumberFormat(locale), [locale])

  React.useEffect(() => {
    const timer = window.setInterval(() => setNow(new Date()), 1000)
    return () => window.clearInterval(timer)
  }, [])

  React.useEffect(() => {
    document.documentElement.lang = locale
    window.localStorage.setItem(localeStorageKey, locale)
    const params = new URLSearchParams(window.location.search)
    params.set('lang', locale)
    if (networkPreset) params.set('network', networkPreset)
    if (rpcUrl && rpcUrl !== (isHostedByControlPlane() ? CONTROL_PLANE_RPC_URL : rpcDefaults[networkPreset as keyof typeof rpcDefaults])) {
      params.set('rpc_url', rpcUrl)
    } else {
      params.delete('rpc_url')
    }
    const query = params.toString()
    window.history.replaceState(null, '', `${window.location.pathname}${query ? `?${query}` : ''}${window.location.hash}`)
  }, [locale, networkPreset, rpcUrl])

  const rpcCall = React.useCallback(async <T,>(method: string, params: unknown[] = []): Promise<T> => {
    if (isLikelyBitcoindRpcUrl(rpcUrl)) throw new Error(t('bitcoind'))
    const startedAt = performance.now()
    const response = await fetch(rpcUrl, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ jsonrpc: '2.0', id: Date.now(), method, params }),
    })
    setLatency(`${Math.round(performance.now() - startedAt)} ms`)
    if (!response.ok) throw new Error(`HTTP ${response.status}`)
    return decodeRpcPayload(await response.json()) as T
  }, [rpcUrl, t])

  const refreshHome = React.useCallback(async () => {
    setHomeError('')
    try {
      const [nextRpcInfo, nextSyncStatus, nextReadiness, nextPassStats, nextActiveBalance, nextCommit] = await Promise.all([
        rpcCall<UsdbRpcInfo>('get_rpc_info'),
        rpcCall<UsdbIndexerSyncStatus>('get_sync_status'),
        rpcCall<UsdbIndexerReadiness>('get_readiness'),
        rpcCall<PassStatsAtHeight>('get_pass_stats_at_height', [{ at_height: null }]),
        rpcCall<RpcActiveBalanceSnapshot | null>('get_latest_active_balance_snapshot'),
        rpcCall<PassBlockCommitInfo | null>('get_pass_block_commit', [{ block_height: null }]),
      ])
      setRpcInfo(nextRpcInfo)
      setSyncStatus(nextSyncStatus)
      setReadiness(nextReadiness)
      setPassStats(nextPassStats)
      setActiveBalance(nextActiveBalance)
      setLatestCommit(nextCommit)
      setRpcHint(t('connected', { time: new Date().toLocaleTimeString(locale) }))
      setNetworkPreset(normalizeNetwork(nextRpcInfo.network))
    } catch (error) {
      setReadiness(null)
      setHomeError(t('homeError', { error: errorMessage(error) }))
      setRpcHint(t('rpcError', { error: errorMessage(error) }))
    }
  }, [locale, rpcCall, t])

  const loadLeaderboard = React.useCallback(async () => {
    try {
      const page = await rpcCall<PassEnergyLeaderboardPage>('get_pass_energy_leaderboard', [
        { at_height: null, scope, page: leaderboardPage, page_size: 50 },
      ])
      setLeaderboard(page)
      setLeaderboardHint('')
    } catch (error) {
      setLeaderboard(null)
      setLeaderboardHint(t('leaderboardFailed', { error: errorMessage(error) }))
    }
  }, [leaderboardPage, rpcCall, scope, t])

  React.useEffect(() => {
    void refreshHome()
    const timer = window.setInterval(() => void refreshHome(), 5000)
    return () => window.clearInterval(timer)
  }, [refreshHome])

  React.useEffect(() => {
    void loadLeaderboard()
  }, [loadLeaderboard])

  async function handleRpcSubmit(event: React.FormEvent<HTMLFormElement>) {
    event.preventDefault()
    const next = rpcDraft.trim()
    if (!next) return
    if (isHostedByControlPlane() && !next.startsWith('/')) {
      setRpcDraft(CONTROL_PLANE_RPC_URL)
      setRpcUrl(CONTROL_PLANE_RPC_URL)
      setRpcHint(t('hostedProxy', { url: CONTROL_PLANE_RPC_URL }))
      return
    }
    setRpcUrl(next)
    setRpcHint(t('switched', { url: next }))
  }

  function applyNetworkPreset(nextNetwork: string) {
    const normalized = normalizeNetwork(nextNetwork)
    if (isHostedByControlPlane()) {
      setNetworkPreset(normalized)
      setRpcDraft(CONTROL_PLANE_RPC_URL)
      setRpcUrl(CONTROL_PLANE_RPC_URL)
      setRpcHint(t('hostedProxy', { url: CONTROL_PLANE_RPC_URL }))
      return
    }
    const nextUrl = rpcDefaults[normalized as keyof typeof rpcDefaults] ?? rpcDefaults.mainnet
    setNetworkPreset(normalized)
    setRpcDraft(nextUrl)
    setRpcUrl(nextUrl)
    setRpcHint(t('preset', { url: nextUrl }))
  }

  async function queryPass(event?: React.FormEvent<HTMLFormElement>) {
    event?.preventDefault()
    await queryPassById(passId, passHeight)
  }

  async function queryPassById(targetId: string, targetHeight: string) {
    try {
      if (!targetId.trim()) throw new Error(t('passIdRequired'))
      const atHeight = parseOptionalHeight(targetHeight, t)
      const snapshot = await rpcCall<PassSnapshot | null>('get_pass_snapshot', [{ inscription_id: targetId.trim(), at_height: atHeight }])
      if (!snapshot) throw new Error(t('passMissing'))
      setPassId(snapshot.inscription_id)
      setPassHeight(targetHeight)
      setPassSnapshot(snapshot)
      setPassHint(t('querySuccess'))
      setPassHistoryPage(0)
      setCommitHeight(String(snapshot.resolved_height))
      const [history, commit] = await Promise.all([
        rpcCall<PassHistoryPage>('get_pass_history', [
          {
            inscription_id: snapshot.inscription_id,
            from_height: snapshot.mint_block_height,
            to_height: snapshot.resolved_height,
            order: 'desc',
            page: 0,
            page_size: 20,
          },
        ]),
        rpcCall<PassBlockCommitInfo | null>('get_pass_block_commit', [{ block_height: snapshot.resolved_height }]),
      ])
      setPassHistory(history)
      setPassCommit(commit)
    } catch (error) {
      setPassHint(t('queryFailed', { error: errorMessage(error) }))
      setPassSnapshot(null)
      setPassHistory(null)
      setPassCommit(null)
    }
  }

  async function queryOwnerPasses(event?: React.FormEvent<HTMLFormElement>, nextPage = ownerPassesPage) {
    event?.preventDefault()
    try {
      if (!ownerAddress.trim()) throw new Error(t('ownerRequired'))
      const atHeight = parseOptionalHeight(ownerHeight, t)
      const page = await rpcCall<OwnerPassesAtHeightPage>('get_owner_passes_at_height', [
        {
          address: ownerAddress.trim(),
          at_height: atHeight,
          states: ownerPassStates(ownerScope),
          order: 'desc',
          page: nextPage,
          page_size: 20,
        },
      ])
      setOwnerPasses(page)
      setOwnerPassesPage(nextPage)
      setOwnerHint(t('ownerPassesSuccess', { count: nf.format(page.total), height: nf.format(page.resolved_height) }))
    } catch (error) {
      setOwnerPasses(null)
      setOwnerHint(t('ownerPassesFailed', { error: errorMessage(error) }))
    }
  }

  async function openOwnerPass(item: OwnerPassItem) {
    setPassQueryMode('id')
    setPassId(item.inscription_id)
    const height = String(ownerPasses?.resolved_height ?? item.latest_event_height)
    setPassHeight(height)
    await queryPassById(item.inscription_id, height)
  }

  async function loadPassHistory(nextPage: number) {
    if (!passSnapshot) return
    try {
      const history = await rpcCall<PassHistoryPage>('get_pass_history', [
        {
          inscription_id: passSnapshot.inscription_id,
          from_height: passSnapshot.mint_block_height,
          to_height: passSnapshot.resolved_height,
          order: 'desc',
          page: nextPage,
          page_size: 20,
        },
      ])
      setPassHistory(history)
      setPassHistoryPage(nextPage)
    } catch (error) {
      setPassHint(t('historyFailed', { error: errorMessage(error) }))
    }
  }

  async function queryCommit(event?: React.FormEvent<HTMLFormElement>, heightOverride?: number | null) {
    event?.preventDefault()
    try {
      const blockHeight = heightOverride === undefined ? parseOptionalHeight(commitHeight, t) : heightOverride
      const commit = await rpcCall<PassBlockCommitInfo | null>('get_pass_block_commit', [{ block_height: blockHeight }])
      if (!commit) {
        setCommitHint(t('commitMissing'))
        setPassCommit(null)
        return
      }
      setPassCommit(commit)
      setCommitHeight(String(commit.block_height))
      setCommitHint(t('commitSuccess', { height: nf.format(commit.block_height) }))
    } catch (error) {
      setCommitHint(t('queryFailed', { error: errorMessage(error) }))
    }
  }

  async function queryEnergy(event?: React.FormEvent<HTMLFormElement>, inscriptionIdOverride?: string) {
    event?.preventDefault()
    try {
      const targetInscriptionId = (inscriptionIdOverride ?? energyId).trim()
      if (!targetInscriptionId) throw new Error(t('passIdRequired'))
      const [snapshot, pass] = await Promise.all([
        rpcCall<PassEnergySnapshot>('get_pass_energy', [
          { inscription_id: targetInscriptionId, block_height: null, mode: 'at_or_before' },
        ]),
        rpcCall<PassSnapshot | null>('get_pass_snapshot', [{ inscription_id: targetInscriptionId, at_height: null }]),
      ])
      setEnergyId(targetInscriptionId)
      setEnergySnapshot(snapshot)
      const from = Math.max(0, Number(pass?.mint_block_height ?? 0))
      const to = Number(snapshot.query_block_height ?? 0)
      setRangeFrom(String(from))
      setRangeTo(String(to))
      setEnergyRangePage(0)
      await loadEnergyRange(snapshot.inscription_id, from, to, 0)
    } catch (error) {
      setEnergyHint(t('queryFailed', { error: errorMessage(error) }))
      setEnergySnapshot(null)
      setEnergyRange(null)
    }
  }

  async function loadEnergyRange(id = energySnapshot?.inscription_id, fromText = Number(rangeFrom), toText = Number(rangeTo), page = energyRangePage) {
    try {
      if (!id) return
      if (!Number.isFinite(fromText) || !Number.isFinite(toText)) throw new Error(t('rangeRequired'))
      if (fromText > toText) throw new Error(t('rangeInvalid'))
      const range = await rpcCall<PassEnergyRangePage>('get_pass_energy_range', [
        { inscription_id: id, from_height: fromText, to_height: toText, order: 'desc', page, page_size: 50 },
      ])
      setEnergyRange(range)
      setEnergyRangePage(page)
      setRangeHint('')
      setEnergyHint(t('energySuccess', { total: nf.format(range.total) }))
    } catch (error) {
      setRangeHint(t('rangeFailed', { error: errorMessage(error) }))
    }
  }

  const progress = syncStatus && syncStatus.total > 0 ? Math.min(100, (syncStatus.current / syncStatus.total) * 100) : 0
  const passTotalPages = Math.max(1, Math.ceil((passHistory?.total ?? 0) / 20))
  const ownerPassesTotalPages = Math.max(1, Math.ceil((ownerPasses?.total ?? 0) / 20))
  const leaderboardTotalPages = Math.max(1, Math.ceil((leaderboard?.total ?? 0) / 50))
  const rangeTotalPages = Math.max(1, Math.ceil((energyRange?.total ?? 0) / 50))

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
              <span className="status-pill" data-tone="success">{dict.runtimeStatus}</span>
              <span className="status-pill" data-tone="info">{dict.minerPass}</span>
              <span className="status-pill" data-tone="info">{dict.energy}</span>
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
              <select value={networkPreset} onChange={(event) => applyNetworkPreset(event.target.value)}>
                <option value="mainnet">Mainnet</option>
                <option value="regtest">Regtest</option>
                <option value="testnet">Testnet</option>
                <option value="signet">Signet</option>
                <option value="testnet4">Testnet4</option>
              </select>
              <input value={rpcDraft} onChange={(event) => setRpcDraft(event.target.value)} />
              <button type="submit">{dict.connect}</button>
            </div>
          </label>
          <p className={rpcHint.includes('error') || rpcHint.includes('异常') ? 'hint negative' : 'hint'}>{rpcHint}</p>
        </form>
      </header>

      <nav className="tabs" aria-label="USDB indexer sections">
        {(['home', 'pass', 'energy'] as Tab[]).map((tab) => (
          <button key={tab} className={activeTab === tab ? 'tab active' : 'tab'} onClick={() => setActiveTab(tab)}>
            {tab === 'home' ? dict.home : tab === 'pass' ? dict.minerPass : dict.energy}
          </button>
        ))}
      </nav>

      {activeTab === 'home' ? (
        <section>
          <section className="metric-grid">
            <Metric icon={<Clock3 size={18} />} label={dict.currentTime} value={now.toLocaleString(locale, { hour12: false })} />
            <Metric icon={<Database size={18} />} label={dict.btcNetwork} value={rpcInfo?.network ?? '-'} />
            <Metric icon={<Activity size={18} />} label={dict.syncedHeight} value={readiness?.synced_block_height == null ? '-' : nf.format(readiness.synced_block_height)} />
            <Metric icon={<RefreshCw size={18} />} label={dict.stableHeight} value={readiness?.balance_history_stable_height == null ? '-' : nf.format(readiness.balance_history_stable_height)} />
          </section>
          <section className="metric-grid three">
            <Metric icon={<Badge size={18} />} label={dict.activePasses} value={nf.format(passStats?.active_count ?? 0)} />
            <Metric icon={<Badge size={18} />} label={dict.totalPasses} value={nf.format(passStats?.total_count ?? 0)} />
            <Metric icon={<Zap size={18} />} label={dict.activeBalance} value={formatBtc(activeBalance?.total_balance, nf)} />
          </section>
          <article className="card">
            <div className="card-head">
              <div><p className="eyebrow">Sync State</p><h2>{dict.syncStatus}</h2></div>
              <button className="ghost" onClick={() => void refreshHome()}>{dict.refresh}</button>
            </div>
            <p className="status-message">{readiness?.message ?? syncStatus?.message ?? '-'}</p>
            <div className="readiness-pills">
              <span className="status-pill" data-tone={readiness?.rpc_alive ? 'success' : 'danger'}>
                {dict.rpcAlive}: {readiness?.rpc_alive ? dict.ready : dict.notReady}
              </span>
              <span className="status-pill" data-tone={readiness?.query_ready ? 'success' : 'warning'}>
                {dict.queryReady}: {readiness?.query_ready ? dict.ready : dict.notReady}
              </span>
              <span className="status-pill" data-tone={readiness?.consensus_ready ? 'success' : 'warning'}>
                {dict.consensusReady}: {readiness?.consensus_ready ? dict.ready : dict.notReady}
              </span>
            </div>
            <div className="progress-wrap"><div className="progress-bar" style={{ width: `${progress.toFixed(2)}%` }} /></div>
            <div className="kv-grid">
              <Field label={dict.currentProgress} value={nf.format(syncStatus?.current ?? 0)} />
              <Field label={dict.progressLimit} value={nf.format(syncStatus?.total ?? 0)} />
              <Field label={dict.genesisHeight} value={syncStatus?.genesis_block_height == null ? '-' : nf.format(syncStatus.genesis_block_height)} />
              <Field label={dict.rpcLatency} value={latency} />
              <Field label={dict.updated} value={now.toLocaleTimeString(locale)} />
            </div>
            {homeError ? <p className="hint negative">{homeError}</p> : null}
          </article>
          <article className="card">
            <div className="card-head">
              <div><p className="eyebrow">State Identity</p><h2>{dict.consistencyStatus}</h2></div>
              <span className="pill">Consensus</span>
            </div>
            <DetailGrid entries={[
              [dict.upstreamSnapshot, <span className="mono" title={hashTitle(readiness?.upstream_snapshot_id)}>{shortText(readiness?.upstream_snapshot_id, 18, 14)}</span>],
              [dict.localStateCommit, <span className="mono" title={hashTitle(readiness?.local_state_commit)}>{shortText(readiness?.local_state_commit, 18, 14)}</span>],
              [dict.systemStateId, <span className="mono" title={hashTitle(readiness?.system_state_id)}>{shortText(readiness?.system_state_id, 18, 14)}</span>],
              [dict.blockers, readiness?.blockers?.length ? readiness.blockers.join(', ') : dict.none],
            ]} />
          </article>
          <article className="card">
            <div className="card-head">
              <div><p className="eyebrow">Local Commit</p><h2>{dict.latestCommit}</h2></div>
              <button className="ghost" disabled={!latestCommit} onClick={() => {
                if (!latestCommit) return
                setActiveTab('pass')
                setCommitHeight(String(latestCommit.block_height))
                void queryCommit(undefined, latestCommit.block_height)
              }}>{dict.openInPass}</button>
            </div>
            {latestCommit ? <DetailGrid entries={commitEntries(latestCommit, nf)} /> : <p className="empty">{dict.waitingCommit}</p>}
          </article>
        </section>
      ) : null}

      {activeTab === 'pass' ? (
        <section>
          <article className="card">
            <div className="card-head">
              <div><p className="eyebrow">Pass Lookup</p><h2>{dict.passQuery}</h2></div>
              <div className="tabs compact-tabs" aria-label="Pass query mode">
                <button className={passQueryMode === 'id' ? 'tab active' : 'tab'} type="button" onClick={() => setPassQueryMode('id')}>{dict.queryByPassId}</button>
                <button className={passQueryMode === 'owner' ? 'tab active' : 'tab'} type="button" onClick={() => setPassQueryMode('owner')}>{dict.queryByOwner}</button>
              </div>
            </div>
            {passQueryMode === 'id' ? (
              <>
                <form className="query" onSubmit={(event) => void queryPass(event)}>
                  <input required placeholder={dict.inscriptionPlaceholder} value={passId} onChange={(event) => setPassId(event.target.value)} />
                  <input type="number" min="0" placeholder={dict.optionalHeight} value={passHeight} onChange={(event) => setPassHeight(event.target.value)} />
                  <button type="submit"><Search size={16} />{dict.query}</button>
                </form>
                {passHint ? <p className={passHint.includes('失败') || passHint.includes('failed') ? 'hint negative' : 'hint'}>{passHint}</p> : null}
              </>
            ) : (
              <>
                <form className="query" onSubmit={(event) => void queryOwnerPasses(event, 0)}>
                  <input required placeholder={dict.ownerAddressPlaceholder} value={ownerAddress} onChange={(event) => setOwnerAddress(event.target.value)} aria-label={dict.ownerAddress} />
                  <input type="number" min="0" placeholder={dict.optionalHeight} value={ownerHeight} onChange={(event) => setOwnerHeight(event.target.value)} />
                  <select value={ownerScope} onChange={(event) => { setOwnerScope(event.target.value as OwnerPassScope); setOwnerPassesPage(0) }} aria-label={dict.ownerStateScope}>
                    <option value="all">{dict.allStates}</option>
                    <option value="active">{dict.activeOnly}</option>
                    <option value="active_dormant">{dict.activeDormant}</option>
                  </select>
                  <button type="submit"><Search size={16} />{dict.query}</button>
                </form>
                {ownerHint ? <p className={ownerHint.includes('失败') || ownerHint.includes('failed') ? 'hint negative' : 'hint'}>{ownerHint}</p> : null}
              </>
            )}
          </article>
          {passQueryMode === 'owner' ? (
            <article className="card">
              <div className="card-head">
                <div>
                  <p className="eyebrow">{ownerPasses ? `${nf.format(ownerPasses.total)} records @ ${nf.format(ownerPasses.resolved_height)}` : 'Owner Portfolio'}</p>
                  <h2>{dict.ownerPasses}</h2>
                </div>
                <div className="pager">
                  <button className="ghost" disabled={ownerPassesPage === 0} onClick={() => void queryOwnerPasses(undefined, ownerPassesPage - 1)}>{dict.previous}</button>
                  <span>{ownerPassesPage + 1}/{ownerPassesTotalPages}</span>
                  <button className="ghost" disabled={ownerPassesPage + 1 >= ownerPassesTotalPages} onClick={() => void queryOwnerPasses(undefined, ownerPassesPage + 1)}>{dict.next}</button>
                </div>
              </div>
              <DataTable headers={['inscription_id', 'state', 'latest_event_height', 'mint_height', 'eth_main', 'satpoint', 'action']} rows={(ownerPasses?.items ?? []).map((item) => [
                shortText(item.inscription_id, 16, 14),
                item.state,
                nf.format(item.latest_event_height),
                nf.format(item.mint_block_height),
                shortText(item.eth_main, 12, 10),
                shortText(item.satpoint, 16, 12),
                <button className="link-button" onClick={() => void openOwnerPass(item)}>{dict.openDetail}</button>,
              ])} />
            </article>
          ) : null}
          <section className="workspace-grid">
            <article className="card">
              <div className="card-head"><h2>{dict.passDetail}</h2></div>
              {passSnapshot ? <DetailGrid entries={passEntries(passSnapshot, nf)} /> : <p className="empty">{dict.noData}</p>}
            </article>
            <article className="card">
              <div className="card-head"><h2>{dict.blockCommit}</h2></div>
              <form className="query" onSubmit={(event) => void queryCommit(event)}>
                <input type="number" min="0" placeholder="block_height" value={commitHeight} onChange={(event) => setCommitHeight(event.target.value)} />
                <button type="submit">{dict.query}</button>
              </form>
              {commitHint ? <p className={commitHint.includes('失败') || commitHint.includes('failed') ? 'hint negative' : 'hint'}>{commitHint}</p> : null}
              {passCommit ? <DetailGrid entries={commitEntries(passCommit, nf)} /> : null}
            </article>
          </section>
          <article className="card">
            <div className="card-head">
              <h2>{dict.history}</h2>
              <div className="pager">
                <button className="ghost" disabled={passHistoryPage === 0} onClick={() => void loadPassHistory(passHistoryPage - 1)}>{dict.previous}</button>
                <span>{passHistoryPage + 1}/{passTotalPages}</span>
                <button className="ghost" disabled={passHistoryPage + 1 >= passTotalPages} onClick={() => void loadPassHistory(passHistoryPage + 1)}>{dict.next}</button>
              </div>
            </div>
            <DataTable headers={['event_id', 'height', 'event_type', 'state', 'owner', 'satpoint']} rows={(passHistory?.items ?? []).map((event) => [
              event.event_id,
              event.block_height,
              event.event_type,
              event.state,
              shortText(event.owner),
              shortText(event.satpoint),
            ])} />
          </article>
        </section>
      ) : null}

      {activeTab === 'energy' ? (
        <section className="workspace-grid energy-grid">
          <article className="card">
            <div className="card-head">
              <h2>{dict.energyLeaderboard}</h2>
              <div className="pager">
                <select value={scope} onChange={(event) => { setScope(event.target.value as EnergyScope); setLeaderboardPage(0) }}>
                  <option value="active">active</option>
                  <option value="active_dormant">active+dormant</option>
                  <option value="all">all</option>
                </select>
                <button className="ghost" disabled={leaderboardPage === 0} onClick={() => setLeaderboardPage((page) => Math.max(0, page - 1))}>{dict.previous}</button>
                <span>{leaderboardPage + 1}/{leaderboardTotalPages}</span>
                <button className="ghost" disabled={leaderboardPage + 1 >= leaderboardTotalPages} onClick={() => setLeaderboardPage((page) => page + 1)}>{dict.next}</button>
              </div>
            </div>
            {leaderboardHint ? <p className="hint negative">{leaderboardHint}</p> : null}
            <DataTable headers={['rank', 'energy', 'inscription_id', 'state', 'height']} rows={(leaderboard?.items ?? []).map((item, index) => [
              leaderboardPage * 50 + index + 1,
              nf.format(item.energy),
              <button className="link-button" onClick={() => { setActiveTab('energy'); void queryEnergy(undefined, item.inscription_id) }}>{shortText(item.inscription_id, 14, 14)}</button>,
              item.state,
              nf.format(item.record_block_height),
            ])} />
          </article>
          <article className="card">
            <div className="card-head"><h2>{dict.currentEnergy}</h2></div>
            <form className="query" onSubmit={(event) => void queryEnergy(event)}>
              <input required placeholder={dict.inscriptionPlaceholder} value={energyId} onChange={(event) => setEnergyId(event.target.value)} />
              <button type="submit">{dict.query}</button>
            </form>
            {energyHint ? <p className={energyHint.includes('失败') || energyHint.includes('failed') ? 'hint negative' : 'hint'}>{energyHint}</p> : null}
            {energySnapshot ? <DetailGrid entries={energyEntries(energySnapshot, nf)} /> : <p className="empty">{dict.noData}</p>}
            <div className="card-head inner-head">
              <h2>{dict.timeline}</h2>
              <div className="pager">
                <button className="ghost" disabled={energyRangePage === 0} onClick={() => void loadEnergyRange(undefined, Number(rangeFrom), Number(rangeTo), energyRangePage - 1)}>{dict.previous}</button>
                <span>{energyRangePage + 1}/{rangeTotalPages}</span>
                <button className="ghost" disabled={energyRangePage + 1 >= rangeTotalPages} onClick={() => void loadEnergyRange(undefined, Number(rangeFrom), Number(rangeTo), energyRangePage + 1)}>{dict.next}</button>
              </div>
            </div>
            <form className="query" onSubmit={(event) => { event.preventDefault(); void loadEnergyRange(undefined, Number(rangeFrom), Number(rangeTo), 0) }}>
              <input type="number" min="0" placeholder="from_height" value={rangeFrom} onChange={(event) => setRangeFrom(event.target.value)} />
              <input type="number" min="0" placeholder="to_height" value={rangeTo} onChange={(event) => setRangeTo(event.target.value)} />
              <button type="submit">{dict.updateRange}</button>
            </form>
            {rangeHint ? <p className="hint negative">{rangeHint}</p> : null}
            <DataTable headers={['record_height', 'state', 'active_height', 'owner', 'owner_balance', 'owner_delta', 'energy']} rows={(energyRange?.items ?? []).map((item) => [
              nf.format(item.record_block_height),
              item.state,
              nf.format(item.active_block_height),
              shortText(item.owner_address),
              formatBtc(item.owner_balance, nf),
              formatDelta(item.owner_delta, nf),
              nf.format(item.energy),
            ])} />
          </article>
        </section>
      ) : null}
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

function Field({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>
}

function DetailGrid({ entries }: { entries: Array<[string, React.ReactNode]> }) {
  return (
    <div className="detail-grid">
      {entries.map(([key, value]) => (
        <div className="detail-item" key={key}>
          <span className="k">{key}</span>
          <span className="v">{value}</span>
        </div>
      ))}
    </div>
  )
}

function DataTable({ headers, rows }: { headers: string[]; rows: React.ReactNode[][] }) {
  return (
    <div className="table-wrap">
      <table>
        <thead><tr>{headers.map((header) => <th key={header}>{header}</th>)}</tr></thead>
        <tbody>{rows.map((row, index) => <tr key={index}>{row.map((cell, cellIndex) => <td key={cellIndex}>{cell}</td>)}</tr>)}</tbody>
      </table>
    </div>
  )
}

function commitEntries(commit: PassBlockCommitInfo, nf: Intl.NumberFormat): Array<[string, React.ReactNode]> {
  return [
    ['block_height', nf.format(commit.block_height)],
    ['balance_history_block_height', nf.format(commit.balance_history_block_height)],
    ['commit_protocol_version', commit.commit_protocol_version],
    ['commit_hash_algo', commit.commit_hash_algo],
    ['mutation_root', shortText(commit.mutation_root, 20, 16)],
    ['block_commit', shortText(commit.block_commit, 20, 16)],
    ['balance_history_block_commit', shortText(commit.balance_history_block_commit, 20, 16)],
  ]
}

function passEntries(pass: PassSnapshot, nf: Intl.NumberFormat): Array<[string, React.ReactNode]> {
  return [
    ['inscription_id', pass.inscription_id],
    ['inscription_number', nf.format(pass.inscription_number)],
    ['resolved_height', nf.format(pass.resolved_height)],
    ['state', pass.state],
    ['owner', shortText(pass.owner, 18, 16)],
    ['mint_block_height', nf.format(pass.mint_block_height)],
    ['mint_owner', shortText(pass.mint_owner, 18, 16)],
    ['eth_main', pass.eth_main],
    ['eth_collab', pass.eth_collab || '-'],
    ['prev', pass.prev.join(', ') || '-'],
    ['invalid_code', pass.invalid_code || '-'],
    ['invalid_reason', pass.invalid_reason || '-'],
    ['satpoint', shortText(pass.satpoint, 18, 16)],
    ['last_event_id', nf.format(pass.last_event_id)],
    ['last_event_type', pass.last_event_type],
  ]
}

function energyEntries(snapshot: PassEnergySnapshot, nf: Intl.NumberFormat): Array<[string, React.ReactNode]> {
  return [
    ['inscription_id', snapshot.inscription_id],
    ['current_height', nf.format(snapshot.query_block_height)],
    ['current_state', snapshot.state],
    ['current_energy', nf.format(snapshot.energy)],
    ['current_owner', shortText(snapshot.owner_address, 18, 16)],
    ['owner_balance', formatBtc(snapshot.owner_balance, nf)],
    ['owner_delta', formatDelta(snapshot.owner_delta, nf)],
  ]
}

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
)
