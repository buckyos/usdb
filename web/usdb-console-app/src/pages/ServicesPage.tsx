import { QuickLinkCard } from '../components/QuickLinkCard'
import { ServiceCard } from '../components/ServiceCard'
import { formatNumber, serviceLabel, serviceTone } from '../lib/console'
import { displayBoolean, displayList, displayNumber, displayShortText, displayText } from '../lib/format'
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
      {renderPair(t('fields.network'), displayText(data?.network, t))}
      {renderPair(t('fields.stableHeight'), displayNumber(locale, data?.stable_height, t))}
      {renderPair(t('fields.phase'), displayText(data?.phase, t))}
      {renderPair(t('fields.consensus'), displayBoolean(data?.consensus_ready, t))}
      {renderPair(t('fields.snapshotVerify'), displayText(data?.snapshot_verification_state, t))}
      {renderPair(t('fields.blockers'), displayList(data?.blockers, t))}
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
      {renderPair(t('fields.network'), displayText(data?.network, t))}
      {renderPair(t('fields.syncedHeight'), displayNumber(locale, data?.synced_block_height, t))}
      {renderPair(
        t('fields.stableHeight'),
        displayNumber(locale, data?.balance_history_stable_height, t),
      )}
      {renderPair(t('fields.consensus'), displayBoolean(data?.consensus_ready, t))}
      {renderPair(t('fields.systemState'), displayShortText(data?.system_state_id, t))}
      {renderPair(t('fields.blockers'), displayList(data?.blockers, t))}
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
      {renderPair(t('fields.client'), displayText(data?.client_version, t))}
      {renderPair(t('fields.chainId'), displayText(data?.chain_id, t))}
      {renderPair(t('fields.networkId'), displayText(data?.network_id, t))}
      {renderPair(t('fields.blockNumber'), displayNumber(locale, data?.block_number, t))}
      {renderPair(
        t('fields.syncing'),
        data?.syncing === false
          ? t('common.false')
          : data?.syncing == null
            ? t('common.notYetAvailable')
            : JSON.stringify(data.syncing),
      )}
      {renderPair(t('fields.consensus'), displayBoolean(data?.consensus_ready, t))}
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
          {renderPair(t('fields.chain'), displayText(data?.services.btc_node.data?.chain, t))}
          {renderPair(
            t('fields.blocks'),
            displayNumber(locale, data?.services.btc_node.data?.blocks, t),
          )}
          {renderPair(
            t('fields.headers'),
            displayNumber(locale, data?.services.btc_node.data?.headers, t),
          )}
          {renderPair(
            t('fields.ibd'),
            displayBoolean(data?.services.btc_node.data?.initial_block_download, t),
          )}
          {renderPair(
            t('fields.verifyProgress'),
            data?.services.btc_node.data?.verification_progress == null
              ? t('common.notYetAvailable')
              : `${(data.services.btc_node.data.verification_progress * 100).toFixed(2)}%`,
          )}
          {renderPair(
            t('fields.latency'),
            data?.services.btc_node.latency_ms
              ? `${data.services.btc_node.latency_ms} ms`
              : t('common.notYetAvailable'),
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
