import { QuickLinkCard } from '../components/QuickLinkCard'
import { ServiceCard } from '../components/ServiceCard'
import { InlineHelpTooltip } from '../components/InlineHelpTooltip'
import { serviceLabel, serviceTone } from '../lib/console'
import { displayBoolean, displayDateTimeFromUnixSeconds, displayList, displayNumber, displayPercent, displayText } from '../lib/format'
import type {
  BalanceHistorySummary,
  BtcNodeSummary,
  EthwSummary,
  OverviewResponse,
  UsdbIndexerSummary,
} from '../lib/types'

interface ServicesPageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

type Translate = (key: string, fallback?: string, variables?: Record<string, string | number>) => string

function renderPair(label: string, value: string, helpText?: string) {
  return (
    <div className="border-t border-[color:var(--cp-border)] pt-3 sm:flex sm:gap-2">
      <span className="shrink-0 inline-flex items-center gap-2 text-sm font-medium text-[color:var(--cp-muted)]">
        <span>{label}:</span>
        <InlineHelpTooltip text={helpText} />
      </span>
      <strong className="block break-all text-sm text-[color:var(--cp-text)]">
        {value}
      </strong>
    </div>
  )
}

function renderBtcNodeDetails(
  locale: string,
  t: Translate,
  data?: BtcNodeSummary | null,
  latencyMs?: number | null,
) {
  return (
    <>
      {renderPair(t('fields.chain'), displayText(data?.chain, t))}
      {renderPair(t('fields.blocks'), displayNumber(locale, data?.blocks, t), t('help.fields.blocks', ''))}
      {renderPair(t('fields.headers'), displayNumber(locale, data?.headers, t), t('help.fields.headers', ''))}
      {renderPair(t('fields.bestBlockHash'), displayText(data?.best_block_hash, t), t('help.fields.bestBlockHash', ''))}
      {renderPair(
        t('fields.blockTime'),
        displayDateTimeFromUnixSeconds(locale, data?.best_block_time, t),
        t('help.fields.blockTime', ''),
      )}
      {renderPair(t('fields.ibd'), displayBoolean(data?.initial_block_download, t), t('help.fields.ibd', ''))}
      {renderPair(
        t('fields.verifyProgress'),
        displayPercent(data?.verification_progress, t),
        t('help.fields.verifyProgress', ''),
      )}
      {renderPair(
        t('fields.latency'),
        latencyMs == null ? t('common.notYetAvailable') : `${latencyMs} ms`,
        t('help.fields.latency', ''),
      )}
    </>
  )
}

function renderBalanceHistoryDetails(
  locale: string,
  t: Translate,
  data?: BalanceHistorySummary | null,
) {
  return (
    <>
      {renderPair(t('fields.network'), displayText(data?.network, t), t('help.fields.network', ''))}
      {renderPair(t('fields.stableHeight'), displayNumber(locale, data?.stable_height, t), t('help.fields.stableHeight', ''))}
      {renderPair(t('fields.phase'), displayText(data?.phase, t), t('help.fields.phase', ''))}
      {renderPair(t('fields.consensus'), displayBoolean(data?.consensus_ready, t), t('help.fields.consensus', ''))}
      {renderPair(t('fields.stableBlockHash'), displayText(data?.stable_block_hash, t), t('help.fields.stableBlockHash', ''))}
      {renderPair(t('fields.latestBlockCommit'), displayText(data?.latest_block_commit, t), t('help.fields.latestBlockCommit', ''))}
      {renderPair(t('fields.snapshotVerify'), displayText(data?.snapshot_verification_state, t), t('help.fields.snapshotVerify', ''))}
      {renderPair(t('fields.snapshotSigningKey'), displayText(data?.snapshot_signing_key_id, t), t('help.fields.snapshotSigningKey', ''))}
      {renderPair(t('fields.statusMessage'), displayText(data?.message, t), t('help.fields.statusMessage', ''))}
      {renderPair(t('fields.blockers'), displayList(data?.blockers, t), t('help.fields.blockers', ''))}
    </>
  )
}

function renderUsdbIndexerDetails(
  locale: string,
  t: Translate,
  data?: UsdbIndexerSummary | null,
) {
  return (
    <>
      {renderPair(t('fields.network'), displayText(data?.network, t), t('help.fields.network', ''))}
      {renderPair(t('fields.syncedHeight'), displayNumber(locale, data?.synced_block_height, t), t('help.fields.syncedHeight', ''))}
      {renderPair(
        t('fields.stableHeight'),
        displayNumber(locale, data?.balance_history_stable_height, t),
        t('help.fields.stableHeight', ''),
      )}
      {renderPair(t('fields.consensus'), displayBoolean(data?.consensus_ready, t), t('help.fields.consensus', ''))}
      {renderPair(t('fields.upstreamSnapshot'), displayText(data?.upstream_snapshot_id, t), t('help.fields.upstreamSnapshot', ''))}
      {renderPair(t('fields.localStateCommit'), displayText(data?.local_state_commit, t), t('help.fields.localStateCommit', ''))}
      {renderPair(t('fields.systemState'), displayText(data?.system_state_id, t), t('help.fields.systemState', ''))}
      {renderPair(t('fields.statusMessage'), displayText(data?.message, t), t('help.fields.statusMessage', ''))}
      {renderPair(t('fields.blockers'), displayList(data?.blockers, t), t('help.fields.blockers', ''))}
    </>
  )
}

function renderEthwDetails(
  locale: string,
  t: Translate,
  data?: EthwSummary | null,
) {
  return (
    <>
      {renderPair(t('fields.client'), displayText(data?.client_version, t), t('help.fields.client', ''))}
      {renderPair(t('fields.chainId'), displayText(data?.chain_id, t), t('help.fields.chainId', ''))}
      {renderPair(t('fields.networkId'), displayText(data?.network_id, t), t('help.fields.networkId', ''))}
      {renderPair(t('fields.blockNumber'), displayNumber(locale, data?.block_number, t), t('help.fields.blockNumber', ''))}
      {renderPair(
        t('fields.syncing'),
        data?.syncing === false
          ? t('common.false')
          : data?.syncing == null
            ? t('common.notYetAvailable')
            : JSON.stringify(data.syncing),
        t('help.fields.syncing', ''),
      )}
      {renderPair(t('fields.consensus'), displayBoolean(data?.consensus_ready, t), t('help.fields.consensus', ''))}
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
          {renderBtcNodeDetails(
            locale,
            t,
            data?.services.btc_node.data,
            data?.services.btc_node.latency_ms,
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
