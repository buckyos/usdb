import type { BootstrapSummary, ServiceProbe, ServicesSummary } from './types'

export type Tone = 'neutral' | 'success' | 'warning' | 'danger'

export function formatDate(locale: string, value?: number | null) {
  if (!value) return '-'
  return new Date(value).toLocaleString(locale, { hour12: false })
}

export function formatNumber(locale: string, value?: number | null) {
  if (value === null || value === undefined || Number.isNaN(Number(value))) {
    return '-'
  }
  return new Intl.NumberFormat(locale).format(Number(value))
}

export function serviceTone<T>(probe: ServiceProbe<T>): Tone {
  const data = probe.data as
    | { query_ready?: boolean | null; consensus_ready?: boolean | null }
    | null
    | undefined
  if (!probe.reachable) return 'danger'
  if (data?.consensus_ready) return 'success'
  if (data?.query_ready) return 'warning'
  return 'neutral'
}

export function serviceLabel<T>(probe: ServiceProbe<T>, t: (key: string) => string) {
  const data = probe.data as
    | { query_ready?: boolean | null; consensus_ready?: boolean | null }
    | null
    | undefined
  if (!probe.reachable) return t('service.offline')
  if (data?.consensus_ready) return t('service.consensusReady')
  if (data?.query_ready) return t('service.queryReady')
  return t('service.reachable')
}

export function artifactTone(exists: boolean): Tone {
  return exists ? 'success' : 'danger'
}

export function reachableServiceCount(services: ServicesSummary) {
  return [
    services.btc_node,
    services.balance_history,
    services.usdb_indexer,
    services.ethw,
  ].filter((service) => service.reachable).length
}

export function consensusReadyServiceCount(services: ServicesSummary) {
  return [
    services.balance_history.data?.consensus_ready ?? false,
    services.usdb_indexer.data?.consensus_ready ?? false,
    services.ethw.data?.consensus_ready ?? false,
  ].filter(Boolean).length
}

export function presentArtifactCount(bootstrap: BootstrapSummary) {
  return [
    bootstrap.bootstrap_manifest.exists,
    bootstrap.snapshot_marker.exists,
    bootstrap.ethw_init_marker.exists,
    bootstrap.sourcedao_bootstrap_state.exists,
    bootstrap.sourcedao_bootstrap_marker.exists,
  ].filter(Boolean).length
}

export function completedBootstrapStepCount(bootstrap: BootstrapSummary) {
  return bootstrap.steps.filter((step) => step.state === 'completed').length
}
