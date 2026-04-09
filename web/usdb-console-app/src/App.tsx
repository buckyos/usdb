import { Globe, RefreshCcw } from 'lucide-react'
import useSWR from 'swr'
import { BootstrapSteps } from './components/BootstrapSteps'
import { ArtifactCard } from './components/ArtifactCard'
import { MetricCard } from './components/MetricCard'
import { ServiceCard } from './components/ServiceCard'
import { fetchOverview } from './lib/api'
import { shortText } from './lib/format'
import type {
  BalanceHistorySummary,
  BtcNodeSummary,
  EthwSummary,
  ServiceProbe,
  UsdbIndexerSummary,
} from './lib/types'
import { useI18n } from './i18n/provider'

type Tone = 'neutral' | 'success' | 'warning' | 'danger'

function formatDate(locale: string, value?: number | null) {
  if (!value) return '-'
  return new Date(value).toLocaleString(locale, { hour12: false })
}

function formatNumber(locale: string, value?: number | null) {
  if (value === null || value === undefined || Number.isNaN(Number(value))) {
    return '-'
  }
  return new Intl.NumberFormat(locale).format(Number(value))
}

function serviceTone<T>(probe: ServiceProbe<T>): Tone {
  const data = probe.data as
    | { query_ready?: boolean | null; consensus_ready?: boolean | null }
    | null
    | undefined
  if (!probe.reachable) return 'danger'
  if (data?.consensus_ready) return 'success'
  if (data?.query_ready) return 'warning'
  return 'neutral'
}

function serviceLabel<T>(probe: ServiceProbe<T>, t: (key: string) => string) {
  const data = probe.data as
    | { query_ready?: boolean | null; consensus_ready?: boolean | null }
    | null
    | undefined
  if (!probe.reachable) return t('service.offline')
  if (data?.consensus_ready) return t('service.consensusReady')
  if (data?.query_ready) return t('service.queryReady')
  return t('service.reachable')
}

function artifactTone(exists: boolean): Tone {
  return exists ? 'success' : 'danger'
}

export function App() {
  const { locale, setLocale, t } = useI18n()
  const { data, error, isLoading, mutate } = useSWR('/api/system/overview', fetchOverview, {
    refreshInterval: 8000,
    revalidateOnFocus: false,
  })

  const totalServices = 4
  const readyCount = data
    ? [
        data.services.btc_node,
        data.services.balance_history,
        data.services.usdb_indexer,
        data.services.ethw,
      ].filter((service) => service.reachable).length
    : 0

  const overallTone: Tone =
    data?.bootstrap.overall_state === 'completed'
      ? 'success'
      : data?.bootstrap.overall_state === 'error'
        ? 'danger'
        : 'warning'

  return (
    <>
      <div className="console-noise" />
      <main className="console-shell">
        <header className="mb-6 grid gap-5 lg:grid-cols-[1.7fr_1fr] lg:items-end">
          <div>
            <p className="shell-kicker m-0">{t('hero.kicker')}</p>
            <h1 className="mt-2 font-display text-[clamp(2.4rem,5vw,4.8rem)] font-semibold leading-[0.95] tracking-[-0.05em] text-[color:var(--cp-text)]">
              {t('hero.title')}
            </h1>
            <p className="mt-4 max-w-4xl text-base leading-7 text-[color:var(--cp-muted)]">
              {t('hero.subtitle')}
            </p>
          </div>

          <div className="console-card">
            <div className="mb-4 flex items-center justify-between gap-3">
              <label className="flex items-center gap-3 text-sm text-[color:var(--cp-muted)]">
                <Globe className="h-4 w-4" />
                <span>{t('actions.language')}</span>
                <select
                  className="rounded-xl border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-3 py-2 text-sm text-[color:var(--cp-text)]"
                  value={locale}
                  onChange={(event) => setLocale(event.target.value as 'en' | 'zh-CN')}
                >
                  <option value="en">{t('locale.en')}</option>
                  <option value="zh-CN">{t('locale.zh-CN')}</option>
                </select>
              </label>

              <button
                type="button"
                className="inline-flex items-center gap-2 rounded-full bg-[color:var(--cp-text)] px-4 py-2 text-sm font-semibold text-white"
                onClick={() => void mutate()}
                disabled={isLoading}
              >
                <RefreshCcw className={`h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
                {isLoading ? t('actions.reloading') : t('actions.refresh')}
              </button>
            </div>

            <p className="text-sm leading-6 text-[color:var(--cp-muted)]">{t('hero.hint')}</p>
          </div>
        </header>

        <section className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
          <MetricCard
            label={t('metrics.updatedAt')}
            value={data ? formatDate(locale, data.generated_at_ms) : '-'}
          />
          <MetricCard
            label={t('metrics.btcNetwork')}
            value={
              data?.services.btc_node.data?.chain ??
              data?.services.balance_history.data?.network ??
              data?.services.usdb_indexer.data?.network ??
              '-'
            }
          />
          <MetricCard
            label={t('metrics.btcHeight')}
            value={formatNumber(locale, data?.services.btc_node.data?.blocks)}
          />
          <MetricCard
            label={t('metrics.ethwHeight')}
            value={formatNumber(locale, data?.services.ethw.data?.block_number)}
          />
        </section>

        <section className="console-card mt-5">
          <div className="mb-4 flex items-start justify-between gap-3">
            <div>
              <h2 className="text-base font-semibold text-[color:var(--cp-text)]">
                {t('sections.services')}
              </h2>
              <p className="mt-2 text-sm text-[color:var(--cp-muted)]">
                {t('service.summary', undefined, {
                  readyCount,
                  total: totalServices,
                })}
              </p>
            </div>
          </div>

          <div className="grid gap-4 xl:grid-cols-2 2xl:grid-cols-4">
            <ServiceCard
              title="btc-node"
              status={data ? serviceLabel(data.services.btc_node, t) : '-'}
              tone={data ? serviceTone(data.services.btc_node) : 'neutral'}
              rpcUrl={data?.services.btc_node.rpc_url ?? '-'}
              error={data?.services.btc_node.error}
            >
              {renderPair(t('fields.chain'), data?.services.btc_node.data?.chain ?? '-')}
              {renderPair(t('fields.blocks'), formatNumber(locale, data?.services.btc_node.data?.blocks))}
              {renderPair(t('fields.headers'), formatNumber(locale, data?.services.btc_node.data?.headers))}
              {renderPair(
                t('fields.ibd'),
                data?.services.btc_node.data?.initial_block_download === undefined
                  ? '-'
                  : String(data.services.btc_node.data.initial_block_download),
              )}
              {renderPair(
                t('fields.verifyProgress'),
                data?.services.btc_node.data?.verification_progress == null
                  ? '-'
                  : `${(data.services.btc_node.data.verification_progress * 100).toFixed(2)}%`,
              )}
              {renderPair(
                t('fields.latency'),
                data?.services.btc_node.latency_ms ? `${data.services.btc_node.latency_ms} ms` : '-',
              )}
            </ServiceCard>

            <ServiceCard
              title="balance-history"
              status={data ? serviceLabel(data.services.balance_history, t) : '-'}
              tone={data ? serviceTone(data.services.balance_history) : 'neutral'}
              rpcUrl={data?.services.balance_history.rpc_url ?? '-'}
              error={data?.services.balance_history.error}
            >
              {renderBalanceHistoryDetails(locale, t, data?.services.balance_history.data)}
            </ServiceCard>

            <ServiceCard
              title="usdb-indexer"
              status={data ? serviceLabel(data.services.usdb_indexer, t) : '-'}
              tone={data ? serviceTone(data.services.usdb_indexer) : 'neutral'}
              rpcUrl={data?.services.usdb_indexer.rpc_url ?? '-'}
              error={data?.services.usdb_indexer.error}
            >
              {renderUsdbIndexerDetails(locale, t, data?.services.usdb_indexer.data)}
            </ServiceCard>

            <ServiceCard
              title="ETHW / Geth"
              status={data ? serviceLabel(data.services.ethw, t) : '-'}
              tone={data ? serviceTone(data.services.ethw) : 'neutral'}
              rpcUrl={data?.services.ethw.rpc_url ?? '-'}
              error={data?.services.ethw.error}
            >
              {renderEthwDetails(locale, t, data?.services.ethw.data)}
            </ServiceCard>
          </div>
        </section>

        <section className="mt-5 grid gap-4 xl:grid-cols-3">
          <ArtifactCard
            title={t('artifacts.bootstrapManifest')}
            summary={
              data?.bootstrap.bootstrap_manifest ?? {
                path: '-',
                exists: false,
                error: null,
                data: null,
              }
            }
            status={data?.bootstrap.bootstrap_manifest.exists ? t('artifact.present') : t('artifact.missing')}
            tone={artifactTone(Boolean(data?.bootstrap.bootstrap_manifest.exists))}
          />
          <ArtifactCard
            title={t('artifacts.snapshotMarker')}
            summary={
              data?.bootstrap.snapshot_marker ?? {
                path: '-',
                exists: false,
                error: null,
                data: null,
              }
            }
            status={data?.bootstrap.snapshot_marker.exists ? t('artifact.present') : t('artifact.missing')}
            tone={artifactTone(Boolean(data?.bootstrap.snapshot_marker.exists))}
          />
          <ArtifactCard
            title={t('artifacts.ethwInitMarker')}
            summary={
              data?.bootstrap.ethw_init_marker ?? {
                path: '-',
                exists: false,
                error: null,
                data: null,
              }
            }
            status={data?.bootstrap.ethw_init_marker.exists ? t('artifact.present') : t('artifact.missing')}
            tone={artifactTone(Boolean(data?.bootstrap.ethw_init_marker.exists))}
          />
          <ArtifactCard
            title={t('artifacts.sourcedaoState')}
            summary={
              data?.bootstrap.sourcedao_bootstrap_state ?? {
                path: '-',
                exists: false,
                error: null,
                data: null,
              }
            }
            status={data?.bootstrap.sourcedao_bootstrap_state.exists ? t('artifact.present') : t('artifact.missing')}
            tone={artifactTone(Boolean(data?.bootstrap.sourcedao_bootstrap_state.exists))}
          />
          <ArtifactCard
            title={t('artifacts.sourcedaoMarker')}
            summary={
              data?.bootstrap.sourcedao_bootstrap_marker ?? {
                path: '-',
                exists: false,
                error: null,
                data: null,
              }
            }
            status={data?.bootstrap.sourcedao_bootstrap_marker.exists ? t('artifact.present') : t('artifact.missing')}
            tone={artifactTone(Boolean(data?.bootstrap.sourcedao_bootstrap_marker.exists))}
          />
        </section>

        <div className="mt-5">
          <BootstrapSteps
            overallLabel={data ? t(`states.${data.bootstrap.overall_state}`) : '-'}
            overallTone={overallTone}
            steps={data?.bootstrap.steps ?? []}
            t={t}
          />
        </div>

        <section className="console-card mt-5">
          <div className="mb-4">
            <h2 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('sections.explorers')}
            </h2>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('explorers.hint')}
            </p>
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <a
              href={data?.explorers.balance_history ?? '/explorers/balance-history/'}
              className="console-subtle-card block no-underline"
            >
              <strong className="block text-base font-semibold text-[color:var(--cp-text)]">
                {t('explorers.balanceHistory')}
              </strong>
              <span className="mt-2 block text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('explorers.balanceHistoryBody')}
              </span>
            </a>

            <a
              href={data?.explorers.usdb_indexer ?? '/explorers/usdb-indexer/'}
              className="console-subtle-card block no-underline"
            >
              <strong className="block text-base font-semibold text-[color:var(--cp-text)]">
                {t('explorers.usdbIndexer')}
              </strong>
              <span className="mt-2 block text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('explorers.usdbIndexerBody')}
              </span>
            </a>
          </div>
        </section>

        {error ? (
          <section className="console-card mt-5 border-[color:var(--cp-danger)]">
            <p className="text-sm text-[color:var(--cp-danger)]">
              {t('errors.loadOverview')} {error.message}
            </p>
          </section>
        ) : null}
      </main>
    </>
  )
}

function renderPair(label: string, value: string) {
  return (
    <div className="border-t border-[color:var(--cp-border)] pt-3">
      <span className="mb-1 block text-[11px] font-semibold uppercase tracking-[0.12em] text-[color:var(--cp-muted)]">
        {label}
      </span>
      <strong className="block break-all text-sm text-[color:var(--cp-text)]">{value}</strong>
    </div>
  )
}

function renderBalanceHistoryDetails(
  locale: string,
  t: (key: string) => string,
  data?: BalanceHistorySummary | null,
) {
  return (
    <>
      {renderPair(t('fields.network'), data?.network ?? '-')}
      {renderPair(t('fields.stableHeight'), formatNumber(locale, data?.stable_height))}
      {renderPair(t('fields.phase'), data?.phase ?? '-')}
      {renderPair(t('fields.consensus'), String(Boolean(data?.consensus_ready)))}
      {renderPair(t('fields.snapshotVerify'), data?.snapshot_verification_state ?? '-')}
      {renderPair(t('fields.blockers'), data?.blockers?.join(', ') ?? '-')}
    </>
  )
}

function renderUsdbIndexerDetails(
  locale: string,
  t: (key: string) => string,
  data?: UsdbIndexerSummary | null,
) {
  return (
    <>
      {renderPair(t('fields.network'), data?.network ?? '-')}
      {renderPair(t('fields.syncedHeight'), formatNumber(locale, data?.synced_block_height))}
      {renderPair(t('fields.stableHeight'), formatNumber(locale, data?.balance_history_stable_height))}
      {renderPair(t('fields.consensus'), String(Boolean(data?.consensus_ready)))}
      {renderPair(t('fields.systemState'), shortText(data?.system_state_id ?? '-'))}
      {renderPair(t('fields.blockers'), data?.blockers?.join(', ') ?? '-')}
    </>
  )
}

function renderEthwDetails(
  locale: string,
  t: (key: string) => string,
  data?: EthwSummary | null,
) {
  return (
    <>
      {renderPair(t('fields.client'), data?.client_version ?? '-')}
      {renderPair(t('fields.chainId'), data?.chain_id ?? '-')}
      {renderPair(t('fields.networkId'), data?.network_id ?? '-')}
      {renderPair(t('fields.blockNumber'), formatNumber(locale, data?.block_number))}
      {renderPair(
        t('fields.syncing'),
        data?.syncing === false ? t('common.false') : JSON.stringify(data?.syncing ?? '-'),
      )}
      {renderPair(t('fields.consensus'), String(Boolean(data?.consensus_ready)))}
    </>
  )
}
