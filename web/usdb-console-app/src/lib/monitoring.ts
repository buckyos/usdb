/** Share presentation names across monitoring, service details and resource tables. */
type Translate = (key: string, fallback?: string, variables?: Record<string, string | number>) => string

const services: Record<string, string> = {
  bitcoin: 'bitcoin', 'btc-node': 'bitcoin', btc_node: 'bitcoin', BTC_MEMORY_LIMIT: 'bitcoin',
  balance_history: 'balance_history', 'balance-history': 'balance_history', BH_MEMORY_LIMIT: 'balance_history',
  usdb_indexer: 'usdb_indexer', 'usdb-indexer': 'usdb_indexer', USDB_INDEXER_MEMORY_LIMIT: 'usdb_indexer',
  usdb_chain: 'usdb_chain', 'usdb-chain': 'usdb_chain', USDB_CHAIN_MEMORY_LIMIT: 'usdb_chain',
  control_plane: 'control_plane', 'control-plane': 'control_plane', 'usdb-control-plane': 'control_plane', CONTROL_PLANE_MEMORY_LIMIT: 'control_plane',
  ord: 'ord', 'ord-server': 'ord', ORD_MEMORY_LIMIT: 'ord',
  snapshot: 'snapshot', 'btc-snapshot-bootstrap': 'snapshot',
  'usdb-checkpoint-verify': 'checkpoint', USDB_CHECKPOINT_VERIFY_MEMORY_LIMIT: 'checkpoint',
}

export function serviceTitle(id: string, t: Translate) {
  return services[id] ? t(`monitor.services.${services[id]}`) : id
}

export function componentTitle(id: string, t: Translate, fallback?: string) {
  return t(`monitor.components.${id}`, services[id] ? serviceTitle(id, t) : fallback ?? id)
}

/** Unknown wire values remain visible for diagnosis and are never interpreted as ready. */
export function monitorValue(group: string, value: string | null | undefined, t: Translate) {
  if (!value) return '—'
  const key = value.replace(/([a-z])([A-Z])/g, '$1_$2').toLowerCase()
  return t(`monitor.${group}.${key}`, `${t('monitor.unknown')} (${value})`)
}

/** Render counters with their meaning, rather than appending raw API unit names. */
export function progressCounter(current: number, total: number | null | undefined, unit: string | undefined, locale: string, t: Translate) {
  const number = (value: number) => value.toLocaleString(locale)
  const separator = locale === 'zh-CN' ? '：' : ': '
  const size = (value: number) => {
    const index = value > 0 ? Math.min(4, Math.floor(Math.log(value) / Math.log(1024))) : 0
    return `${(value / 1024 ** index).toLocaleString(locale, { maximumFractionDigits: 1 })} ${['B', 'KiB', 'MiB', 'GiB', 'TiB'][index]}`
  }
  if (unit === 'blocks') return `${t(total == null ? 'monitor.height' : 'monitor.heights')}${separator}${number(current)}${total == null ? '' : ` / ${number(total)}`}`
  const format = unit === 'bytes' ? size : number
  return `${t(unit === 'utxos' ? 'monitor.utxos' : unit === 'bytes' ? 'monitor.bytes' : 'monitor.progress')}${separator}${format(current)}${total == null ? '' : ` / ${format(total)}`}`
}
