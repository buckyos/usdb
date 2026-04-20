import { BootstrapSteps } from '../components/BootstrapSteps'
import { MetricCard } from '../components/MetricCard'
import { QuickLinkCard } from '../components/QuickLinkCard'
import { ServiceSummaryCard } from '../components/ServiceSummaryCard'
import {
  completedBootstrapStepCount,
  consensusReadyServiceCount,
  consoleModeLabel,
  formatDate,
  presentArtifactCount,
  reachableServiceCount,
  serviceLabel,
  serviceTone,
  type Tone,
} from '../lib/console'
import { displayDateTimeFromUnixSeconds, displayNumber, displayPercent, displayText } from '../lib/format'
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
          value={data ? formatDate(locale, data.generated_at_ms) : t('common.notYetAvailable')}
          helpText={t('help.metrics.updatedAt', '')}
        />
        <MetricCard
          label={t('metrics.btcNetwork')}
          value={
            displayText(
              data?.services.btc_node.data?.chain ??
                data?.services.balance_history.data?.network ??
                data?.services.usdb_indexer.data?.network,
              t,
            )
          }
          helpText={t('help.metrics.btcNetwork', '')}
        />
        <MetricCard
          label={t('metrics.btcHeight')}
          value={displayNumber(locale, data?.services.btc_node.data?.blocks, t)}
          helpText={t('help.metrics.btcHeight', '')}
        />
        <MetricCard
          label={t('metrics.ethwHeight')}
          value={displayNumber(locale, data?.services.ethw.data?.block_number, t)}
          helpText={t('help.metrics.ethwHeight', '')}
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
            items={[
              {
                label: t('fields.chain'),
                value: displayText(data?.services.btc_node.data?.chain, t),
                helpText: t('help.fields.chain', ''),
              },
              {
                label: t('fields.blocks'),
                value: displayNumber(locale, data?.services.btc_node.data?.blocks, t),
                helpText: t('help.fields.blocks', ''),
              },
              {
                label: t('fields.bestBlockHash'),
                value: displayText(data?.services.btc_node.data?.best_block_hash, t),
                monospace: true,
                helpText: t('help.fields.bestBlockHash', ''),
              },
              {
                label: t('fields.blockTime'),
                value: displayDateTimeFromUnixSeconds(
                  locale,
                  data?.services.btc_node.data?.best_block_time,
                  t,
                ),
                helpText: t('help.fields.blockTime', ''),
              },
              {
                label: t('fields.verifyProgress'),
                value: displayPercent(data?.services.btc_node.data?.verification_progress, t),
                helpText: t('help.fields.verifyProgress', ''),
              },
            ]}
          />
          <ServiceSummaryCard
            title="balance-history"
            status={data ? serviceLabel(data.services.balance_history, t) : '-'}
            tone={data ? serviceTone(data.services.balance_history) : 'neutral'}
            items={[
              {
                label: t('fields.stableHeight'),
                value: displayNumber(locale, data?.services.balance_history.data?.stable_height, t),
                helpText: t('help.fields.stableHeight', ''),
              },
              {
                label: t('fields.phase'),
                value: displayText(data?.services.balance_history.data?.phase, t),
                helpText: t('help.fields.phase', ''),
              },
              {
                label: t('fields.stableBlockHash'),
                value: displayText(data?.services.balance_history.data?.stable_block_hash, t),
                monospace: true,
                helpText: t('help.fields.stableBlockHash', ''),
              },
              {
                label: t('fields.latestBlockCommit'),
                value: displayText(data?.services.balance_history.data?.latest_block_commit, t),
                monospace: true,
                helpText: t('help.fields.latestBlockCommit', ''),
              },
              {
                label: t('fields.snapshotVerify'),
                value: displayText(
                  data?.services.balance_history.data?.snapshot_verification_state,
                  t,
                ),
                helpText: t('help.fields.snapshotVerify', ''),
              },
            ]}
          />
          <ServiceSummaryCard
            title="usdb-indexer"
            status={data ? serviceLabel(data.services.usdb_indexer, t) : '-'}
            tone={data ? serviceTone(data.services.usdb_indexer) : 'neutral'}
            items={[
              {
                label: t('fields.syncedHeight'),
                value: displayNumber(locale, data?.services.usdb_indexer.data?.synced_block_height, t),
                helpText: t('help.fields.syncedHeight', ''),
              },
              {
                label: t('fields.stableHeight'),
                value: displayNumber(
                  locale,
                  data?.services.usdb_indexer.data?.balance_history_stable_height,
                  t,
                ),
                helpText: t('help.fields.stableHeight', ''),
              },
              {
                label: t('fields.upstreamSnapshot'),
                value: displayText(data?.services.usdb_indexer.data?.upstream_snapshot_id, t),
                monospace: true,
                helpText: t('help.fields.upstreamSnapshot', ''),
              },
              {
                label: t('fields.localStateCommit'),
                value: displayText(data?.services.usdb_indexer.data?.local_state_commit, t),
                monospace: true,
                helpText: t('help.fields.localStateCommit', ''),
              },
              {
                label: t('fields.systemState'),
                value: displayText(data?.services.usdb_indexer.data?.system_state_id, t),
                monospace: true,
                helpText: t('help.fields.systemState', ''),
              },
            ]}
          />
          <ServiceSummaryCard
            title="ETHW / Geth"
            status={data ? serviceLabel(data.services.ethw, t) : '-'}
            tone={data ? serviceTone(data.services.ethw) : 'neutral'}
            items={[
              {
                label: t('fields.chainId'),
                value: displayText(data?.services.ethw.data?.chain_id, t),
                helpText: t('help.fields.chainId', ''),
              },
              {
                label: t('fields.networkId'),
                value: displayText(data?.services.ethw.data?.network_id, t),
                helpText: t('help.fields.networkId', ''),
              },
              {
                label: t('fields.blockNumber'),
                value: displayNumber(locale, data?.services.ethw.data?.block_number, t),
                helpText: t('help.fields.blockNumber', ''),
              },
              {
                label: t('fields.latestBlockHash'),
                value: displayText(data?.services.ethw.data?.latest_block_hash, t),
                monospace: true,
                helpText: t('help.fields.latestBlockHash', ''),
              },
              {
                label: t('fields.latestBlockTime'),
                value: displayDateTimeFromUnixSeconds(
                  locale,
                  data?.services.ethw.data?.latest_block_time,
                  t,
                ),
                helpText: t('help.fields.latestBlockTime', ''),
              },
              {
                label: t('fields.client'),
                value: displayText(data?.services.ethw.data?.client_version, t),
                helpText: t('help.fields.client', ''),
              },
            ]}
          />
          <ServiceSummaryCard
            title="ord"
            status={data ? serviceLabel(data.services.ord, t) : '-'}
            tone={data ? serviceTone(data.services.ord) : 'neutral'}
            items={[
              {
                label: t('fields.httpStatus'),
                value: displayNumber(locale, data?.services.ord.data?.http_status, t),
                helpText: t('help.fields.httpStatus', ''),
              },
              {
                label: t('fields.backendReady'),
                value: displayText(
                  data?.services.ord.data?.backend_ready == null
                    ? null
                    : data.services.ord.data.backend_ready
                      ? t('common.true')
                      : t('common.false'),
                  t,
                ),
                helpText: t('help.fields.backendReady', ''),
              },
              {
                label: t('fields.indexedHeight'),
                value: displayNumber(locale, data?.services.ord.data?.synced_block_height, t),
                helpText: t('help.fields.indexedHeight', ''),
              },
              {
                label: t('fields.btcTipHeight'),
                value: displayNumber(locale, data?.services.ord.data?.btc_tip_height, t),
                helpText: t('help.fields.btcTipHeight', ''),
              },
              {
                label: t('fields.syncGap'),
                value: displayNumber(locale, data?.services.ord.data?.sync_gap, t),
                helpText: t('help.fields.syncGap', ''),
              },
              {
                label: t('fields.rpcUrl'),
                value: displayText(data?.services.ord.rpc_url, t),
                helpText: t('help.fields.rpcUrl', ''),
              },
            ]}
          />
        </div>
      </section>

      <section className="grid gap-4 md:grid-cols-2">
        <MetricCard
          label={t('metrics.btcConsoleMode')}
          value={consoleModeLabel(data?.capabilities.btc_console_mode, t)}
          helpText={t('help.metrics.btcConsoleMode', '')}
        />
        <MetricCard
          label={t('metrics.ordBackend')}
          value={
            data?.capabilities.ord_available
              ? t('capabilities.ord.available')
              : t('capabilities.ord.unavailable')
          }
          helpText={t('help.metrics.ordBackend', '')}
        />
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
