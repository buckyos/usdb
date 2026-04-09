import { QuickLinkCard } from '../components/QuickLinkCard'
import { ServiceCard } from '../components/ServiceCard'
import { formatNumber, serviceLabel, serviceTone } from '../lib/console'
import { shortText } from '../lib/format'
import type {
  BalanceHistorySummary,
  EthwSummary,
  OverviewResponse,
  UsdbIndexerSummary,
} from '../lib/types'

interface ServicesPageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
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

export function ServicesPage({ data, locale, t }: ServicesPageProps) {
  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
          {t('pages.services.title')}
        </h2>
        <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {t('pages.services.subtitle')}
        </p>
      </section>

      <section className="grid gap-4 lg:grid-cols-2">
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
            data?.services.btc_node.data?.initial_block_download == null
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
      </section>

      <section className="console-card">
        <div className="mb-4">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('sections.explorers')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('pages.services.explorerBody')}
          </p>
        </div>

        <div className="grid gap-4 md:grid-cols-2">
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
