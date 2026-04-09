import { BootstrapSteps } from '../components/BootstrapSteps'
import { MetricCard } from '../components/MetricCard'
import { QuickLinkCard } from '../components/QuickLinkCard'
import { ServiceSummaryCard } from '../components/ServiceSummaryCard'
import { artifactTone, completedBootstrapStepCount, consensusReadyServiceCount, formatDate, formatNumber, presentArtifactCount, reachableServiceCount, serviceLabel, serviceTone, type Tone } from '../lib/console'
import type { OverviewResponse } from '../lib/types'

interface OverviewPageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

function overallBootstrapTone(state?: string): Tone {
  if (state === 'completed') return 'success'
  if (state === 'error') return 'danger'
  return 'warning'
}

export function OverviewPage({ data, locale, t }: OverviewPageProps) {
  const totalServices = 4
  const consensusReadyTotal = 3
  const reachableCount = data ? reachableServiceCount(data.services) : 0
  const consensusReadyCount = data ? consensusReadyServiceCount(data.services) : 0
  const completedSteps = data ? completedBootstrapStepCount(data.bootstrap) : 0
  const presentArtifacts = data ? presentArtifactCount(data.bootstrap) : 0

  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
          {t('pages.overview.title')}
        </h2>
        <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {t('pages.overview.subtitle')}
        </p>
      </section>

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

      <section className="console-card">
        <div className="mb-4">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('pages.overview.serviceSummaryTitle')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('service.summary', undefined, {
              readyCount: reachableCount,
              total: totalServices,
            })}
          </p>
        </div>

        <div className="grid gap-4 lg:grid-cols-2">
          <ServiceSummaryCard
            title="btc-node"
            status={data ? serviceLabel(data.services.btc_node, t) : '-'}
            tone={data ? serviceTone(data.services.btc_node) : 'neutral'}
            summary={t('overview.btcSummary', undefined, {
              blocks: formatNumber(locale, data?.services.btc_node.data?.blocks),
              chain: data?.services.btc_node.data?.chain ?? '-',
            })}
          />
          <ServiceSummaryCard
            title="balance-history"
            status={data ? serviceLabel(data.services.balance_history, t) : '-'}
            tone={data ? serviceTone(data.services.balance_history) : 'neutral'}
            summary={t('overview.bhSummary', undefined, {
              stableHeight: formatNumber(locale, data?.services.balance_history.data?.stable_height),
              phase: data?.services.balance_history.data?.phase ?? '-',
            })}
          />
          <ServiceSummaryCard
            title="usdb-indexer"
            status={data ? serviceLabel(data.services.usdb_indexer, t) : '-'}
            tone={data ? serviceTone(data.services.usdb_indexer) : 'neutral'}
            summary={t('overview.indexerSummary', undefined, {
              syncedHeight: formatNumber(locale, data?.services.usdb_indexer.data?.synced_block_height),
              systemState: data?.services.usdb_indexer.data?.system_state_id ?? '-',
            })}
          />
          <ServiceSummaryCard
            title="ETHW / Geth"
            status={data ? serviceLabel(data.services.ethw, t) : '-'}
            tone={data ? serviceTone(data.services.ethw) : 'neutral'}
            summary={t('overview.ethwSummary', undefined, {
              blockNumber: formatNumber(locale, data?.services.ethw.data?.block_number),
              chainId: data?.services.ethw.data?.chain_id ?? '-',
            })}
          />
        </div>
      </section>

      <section className="grid gap-4 xl:grid-cols-[1.2fr_1fr]">
        <div className="console-card">
          <div className="mb-4">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('pages.overview.bootstrapTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('pages.overview.bootstrapBody')}
            </p>
          </div>

          <div className="grid gap-4 md:grid-cols-2">
            <MetricCard
              label={t('overview.reachableServices')}
              value={`${reachableCount}/${totalServices}`}
            />
            <MetricCard
              label={t('overview.consensusReady')}
              value={`${consensusReadyCount}/${consensusReadyTotal}`}
            />
            <MetricCard
              label={t('overview.completedSteps')}
              value={`${completedSteps}/${data?.bootstrap.steps.length ?? 4}`}
            />
            <MetricCard
              label={t('overview.artifactsPresent')}
              value={`${presentArtifacts}/5`}
            />
          </div>
        </div>

        <BootstrapSteps
          overallLabel={data ? t(`states.${data.bootstrap.overall_state}`) : '-'}
          overallTone={overallBootstrapTone(data?.bootstrap.overall_state)}
          steps={data?.bootstrap.steps ?? []}
          t={t}
        />
      </section>

      <section className="console-card">
        <div className="mb-4">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('pages.overview.quickLinksTitle')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('pages.overview.quickLinksBody')}
          </p>
        </div>

        <div className="grid gap-4 md:grid-cols-2 xl:grid-cols-4">
          <QuickLinkCard
            to="/services"
            title={t('quick.services')}
            body={t('quick.servicesBody')}
          />
          <QuickLinkCard
            to="/bootstrap"
            title={t('quick.bootstrap')}
            body={t('quick.bootstrapBody')}
          />
          <QuickLinkCard
            href={data?.explorers.balance_history ?? '/explorers/balance-history/'}
            title={t('explorers.balanceHistory')}
            body={t('explorers.balanceHistoryBody')}
          />
          <QuickLinkCard
            href={data?.explorers.usdb_indexer ?? '/explorers/usdb-indexer/'}
            title={t('explorers.usdbIndexer')}
            body={t('explorers.usdbIndexerBody')}
          />
        </div>
      </section>
    </div>
  )
}
