import { NavLink, useParams } from 'react-router-dom'
import { InlineHelpTooltip } from '../components/InlineHelpTooltip'
import { FieldValueList } from '../components/FieldValueList'
import { serviceLabel, serviceTone } from '../lib/console'
import {
  displayBoolean,
  displayDateTimeFromUnixSeconds,
  displayList,
  displayNumber,
  displayShortText,
  displayPercent,
  displayText,
} from '../lib/format'
import type {
  BalanceHistorySummary,
  BtcNodeSummary,
  EthwSummary,
  OrdSummary,
  OverviewResponse,
  ServiceProbe,
  UsdbIndexerSummary,
} from '../lib/types'

interface ServicesPageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

type Translate = (key: string, fallback?: string, variables?: Record<string, string | number>) => string
type ServiceId = 'btc-node' | 'balance-history' | 'usdb-indexer' | 'ethw' | 'ord'

const SERVICE_IDS: ServiceId[] = ['btc-node', 'balance-history', 'usdb-indexer', 'ethw', 'ord']

function normalizeNumericIdentifier(value?: string | null) {
  if (!value) return null
  const raw = value.trim()
  if (!raw) return null
  try {
    return raw.startsWith('0x') || raw.startsWith('0X') ? BigInt(raw).toString(10) : BigInt(raw).toString(10)
  } catch {
    return raw
  }
}

function shouldShowEthwNetworkId(chainId?: string | null, networkId?: string | null) {
  if (!networkId) return false
  if (!chainId) return true
  return normalizeNumericIdentifier(chainId) !== normalizeNumericIdentifier(networkId)
}

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
    <FieldValueList
      items={[
        {
          label: t('fields.chain'),
          value: displayText(data?.chain, t),
          helpText: t('help.fields.chain'),
        },
        {
          label: t('fields.blocks'),
          value: displayNumber(locale, data?.blocks, t),
          helpText: t('help.fields.blocks'),
        },
        {
          label: t('fields.headers'),
          value: displayNumber(locale, data?.headers, t),
          helpText: t('help.fields.headers'),
        },
        {
          label: t('fields.bestBlockHash'),
          value: displayText(data?.best_block_hash, t),
          helpText: t('help.fields.bestBlockHash'),
        },
        {
          label: t('fields.blockTime'),
          value: displayDateTimeFromUnixSeconds(locale, data?.best_block_time, t),
          helpText: t('help.fields.blockTime'),
        },
        {
          label: t('fields.ibd'),
          value: displayBoolean(data?.initial_block_download, t),
          helpText: t('help.fields.ibd'),
        },
        {
          label: t('fields.verifyProgress'),
          value: displayPercent(data?.verification_progress, t),
          helpText: t('help.fields.verifyProgress'),
        },
        {
          label: t('fields.latency'),
          value: latencyMs == null ? t('common.notYetAvailable') : `${latencyMs} ms`,
          helpText: t('help.fields.latency'),
        },
      ]}
    />
  )
}

function renderBalanceHistoryDetails(
  locale: string,
  t: Translate,
  data?: BalanceHistorySummary | null,
) {
  return (
    <FieldValueList
      items={[
        {
          label: t('fields.network'),
          value: displayText(data?.network, t),
          helpText: t('help.fields.network'),
        },
        {
          label: t('fields.stableHeight'),
          value: displayNumber(locale, data?.stable_height, t),
          helpText: t('help.fields.stableHeight'),
        },
        {
          label: t('fields.phase'),
          value: displayText(data?.phase, t),
          helpText: t('help.fields.phase'),
        },
        {
          label: t('fields.consensus'),
          value: displayBoolean(data?.consensus_ready, t),
          helpText: t('help.fields.consensus'),
        },
        {
          label: t('fields.stableBlockHash'),
          value: displayText(data?.stable_block_hash, t),
          helpText: t('help.fields.stableBlockHash'),
        },
        {
          label: t('fields.latestBlockCommit'),
          value: displayText(data?.latest_block_commit, t),
          helpText: t('help.fields.latestBlockCommit'),
        },
        {
          label: t('fields.snapshotVerify'),
          value: displayText(data?.snapshot_verification_state, t),
          helpText: t('help.fields.snapshotVerify'),
        },
        {
          label: t('fields.snapshotSigningKey'),
          value: displayText(data?.snapshot_signing_key_id, t),
          helpText: t('help.fields.snapshotSigningKey'),
        },
        {
          label: t('fields.statusMessage'),
          value: displayText(data?.message, t),
          helpText: t('help.fields.statusMessage'),
        },
        {
          label: t('fields.blockers'),
          value: displayList(data?.blockers, t),
          helpText: t('help.fields.blockers'),
        },
      ]}
    />
  )
}

function renderUsdbIndexerDetails(
  locale: string,
  t: Translate,
  data?: UsdbIndexerSummary | null,
) {
  return (
    <FieldValueList
      items={[
        {
          label: t('fields.network'),
          value: displayText(data?.network, t),
          helpText: t('help.fields.network'),
        },
        {
          label: t('fields.syncedHeight'),
          value: displayNumber(locale, data?.synced_block_height, t),
          helpText: t('help.fields.syncedHeight'),
        },
        {
          label: t('fields.stableHeight'),
          value: displayNumber(locale, data?.balance_history_stable_height, t),
          helpText: t('help.fields.stableHeight'),
        },
        {
          label: t('fields.consensus'),
          value: displayBoolean(data?.consensus_ready, t),
          helpText: t('help.fields.consensus'),
        },
        {
          label: t('fields.upstreamSnapshot'),
          value: displayText(data?.upstream_snapshot_id, t),
          helpText: t('help.fields.upstreamSnapshot'),
        },
        {
          label: t('fields.localStateCommit'),
          value: displayText(data?.local_state_commit, t),
          helpText: t('help.fields.localStateCommit'),
        },
        {
          label: t('fields.systemState'),
          value: displayText(data?.system_state_id, t),
          helpText: t('help.fields.systemState'),
        },
        {
          label: t('fields.statusMessage'),
          value: displayText(data?.message, t),
          helpText: t('help.fields.statusMessage'),
        },
        {
          label: t('fields.blockers'),
          value: displayList(data?.blockers, t),
          helpText: t('help.fields.blockers'),
        },
      ]}
    />
  )
}

function renderExplorerServiceDetails(
  serviceId: 'balance-history' | 'usdb-indexer',
  data: OverviewResponse | undefined,
  locale: string,
  t: Translate,
) {
  const explorerUrl =
    serviceId === 'balance-history'
      ? (data?.explorers.balance_history ?? '/explorers/balance-history/')
      : (data?.explorers.usdb_indexer ?? '/explorers/usdb-indexer/')
  const serviceData =
    serviceId === 'balance-history'
      ? data?.services.balance_history.data
      : data?.services.usdb_indexer.data

  return (
    <article className="console-card">
      <div className="mb-5 flex flex-wrap items-start justify-between gap-4">
        <div>
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('services.workspace.serviceStatusTitle', 'Service Status')}
          </h3>
          <p className="mt-2 max-w-3xl text-sm leading-6 text-[color:var(--cp-muted)]">
            {t(
              'services.workspace.explorerRedirectBody',
              'This page now keeps only service health and runtime metadata. Open the standalone explorer app for full query and protocol tooling.',
            )}
          </p>
        </div>
        <a
          className="console-action-button inline-flex items-center gap-2 no-underline"
          href={explorerUrl}
          target="_blank"
          rel="noreferrer"
        >
          {t('actions.openApp')}
        </a>
      </div>

      {serviceId === 'balance-history'
        ? renderBalanceHistoryDetails(locale, t, serviceData as BalanceHistorySummary | undefined)
        : renderUsdbIndexerDetails(locale, t, serviceData as UsdbIndexerSummary | undefined)}
    </article>
  )
}

function renderEthwDetails(locale: string, t: Translate, data?: EthwSummary | null) {
  return (
    <FieldValueList
      items={[
        {
          label: t('fields.client'),
          value: displayText(data?.client_version, t),
          helpText: t('help.fields.client'),
        },
        {
          label: t('fields.chainId'),
          value: displayText(data?.chain_id, t),
          helpText: t('help.fields.chainId'),
        },
        ...(shouldShowEthwNetworkId(data?.chain_id, data?.network_id)
          ? [
              {
                label: t('fields.networkId'),
                value: displayText(data?.network_id, t),
                helpText: t('help.fields.networkId'),
              },
            ]
          : []),
        {
          label: t('fields.blockNumber'),
          value: displayNumber(locale, data?.block_number, t),
          helpText: t('help.fields.blockNumber'),
        },
        {
          label: t('fields.latestBlockHash'),
          value: displayText(data?.latest_block_hash, t),
          helpText: t('help.fields.latestBlockHash'),
        },
        {
          label: t('fields.latestBlockTime'),
          value: displayDateTimeFromUnixSeconds(locale, data?.latest_block_time, t),
          helpText: t('help.fields.latestBlockTime'),
        },
        {
          label: t('fields.syncing'),
          value:
            data?.syncing === false
              ? t('common.false')
              : data?.syncing == null
                ? t('common.notYetAvailable')
                : JSON.stringify(data.syncing),
          helpText: t('help.fields.syncing'),
        },
        {
          label: t('fields.consensus'),
          value: displayBoolean(data?.consensus_ready, t),
          helpText: t('help.fields.consensus'),
        },
      ]}
    />
  )
}

function renderOrdDetails(
  locale: string,
  t: Translate,
  data?: OrdSummary | null,
  rpcUrl?: string | null,
) {
  return (
    <FieldValueList
      items={[
        {
          label: t('fields.rpcUrl'),
          value: displayText(rpcUrl, t),
          helpText: t('help.fields.rpcUrl'),
        },
        {
          label: t('fields.httpStatus'),
          value: displayText(data?.http_status, t),
          helpText: t('help.fields.httpStatus'),
        },
        {
          label: t('fields.backendReady'),
          value: displayBoolean(data?.backend_ready, t),
          helpText: t('help.fields.backendReady'),
        },
        {
          label: t('fields.indexedHeight'),
          value: displayNumber(locale, data?.synced_block_height, t),
          helpText: t('help.fields.indexedHeight'),
        },
        {
          label: t('fields.btcTipHeight'),
          value: displayNumber(locale, data?.btc_tip_height, t),
          helpText: t('help.fields.btcTipHeight'),
        },
        {
          label: t('fields.syncGap'),
          value: displayNumber(locale, data?.sync_gap, t),
          helpText: t('help.fields.syncGap'),
        },
      ]}
    />
  )
}

function getSelectedService(raw: string | undefined): ServiceId {
  if (raw && SERVICE_IDS.includes(raw as ServiceId)) {
    return raw as ServiceId
  }
  return 'btc-node'
}

function getServiceSummaryLine(
  serviceId: ServiceId,
  data: OverviewResponse | undefined,
  locale: string,
  t: Translate,
) {
  if (!data) return t('common.notYetAvailable')

  switch (serviceId) {
    case 'btc-node':
      return t('services.workspace.btcSummary', undefined, {
        blocks: displayNumber(locale, data.services.btc_node.data?.blocks ?? null, t),
        chain: displayText(data.services.btc_node.data?.chain, t),
      })
    case 'balance-history':
      return t('services.workspace.balanceHistorySummary', undefined, {
        height: displayNumber(locale, data.services.balance_history.data?.stable_height ?? null, t),
        phase: displayText(data.services.balance_history.data?.phase, t),
      })
    case 'usdb-indexer':
      return t('services.workspace.usdbIndexerSummary', undefined, {
        height: displayNumber(locale, data.services.usdb_indexer.data?.synced_block_height ?? null, t),
        state: displayShortText(data.services.usdb_indexer.data?.system_state_id, t, {
          head: 10,
          tail: 8,
        }),
      })
    case 'ethw':
      return t('services.workspace.ethwSummary', undefined, {
        block: displayNumber(locale, data.services.ethw.data?.block_number ?? null, t),
        chainId: displayText(data.services.ethw.data?.chain_id, t),
      })
    case 'ord':
      if (
        data.services.ord.data?.synced_block_height != null &&
        data.services.ord.data?.btc_tip_height != null
      ) {
        return t('services.workspace.ordSummarySync', undefined, {
          height: displayNumber(locale, data.services.ord.data.synced_block_height, t),
          btcHeight: displayNumber(locale, data.services.ord.data.btc_tip_height, t),
          gap: displayNumber(locale, data.services.ord.data.sync_gap ?? null, t),
        })
      }
      return data.capabilities.ord_available
        ? t('services.workspace.ordSummaryEnabled')
        : t('services.workspace.ordSummaryReadOnly')
  }
}

function getServiceMeta(serviceId: ServiceId, t: Translate) {
  switch (serviceId) {
    case 'btc-node':
      return {
        title: 'btc-node',
        headline: t('services.workspace.btcTitle'),
        body: t('services.workspace.btcBody'),
      }
    case 'balance-history':
      return {
        title: 'balance-history',
        headline: t('services.balanceHistory.title'),
        body: t('services.balanceHistory.subtitle'),
      }
    case 'usdb-indexer':
      return {
        title: 'usdb-indexer',
        headline: t('services.usdbIndexer.title'),
        body: t('services.usdbIndexer.subtitle'),
      }
    case 'ethw':
      return {
        title: 'ETHW / Geth',
        headline: t('services.workspace.ethwTitle'),
        body: t('services.workspace.ethwBody'),
      }
    case 'ord':
      return {
        title: 'ord',
        headline: t('services.workspace.ordTitle'),
        body: t('services.workspace.ordBody'),
      }
  }
}

function renderServiceContent(
  serviceId: ServiceId,
  data: OverviewResponse | undefined,
  locale: string,
  t: Translate,
) {
  switch (serviceId) {
    case 'btc-node':
      return (
        <article className="console-card">
          <div className="mb-4">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('services.workspace.btcRuntimeTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('services.workspace.btcRuntimeBody')}
            </p>
          </div>
          {renderBtcNodeDetails(
            locale,
            t,
            data?.services.btc_node.data,
            data?.services.btc_node.latency_ms,
          )}
          {data?.services.btc_node.error ? (
            <p className="mt-4 text-sm text-[color:var(--cp-danger)] break-all">
              {data.services.btc_node.error}
            </p>
          ) : null}
        </article>
      )
    case 'balance-history':
      return renderExplorerServiceDetails('balance-history', data, locale, t)
    case 'usdb-indexer':
      return renderExplorerServiceDetails('usdb-indexer', data, locale, t)
    case 'ethw':
      return (
        <article className="console-card">
          <div className="mb-4">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('services.workspace.ethwRuntimeTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('services.workspace.ethwRuntimeBody')}
            </p>
          </div>
          {renderEthwDetails(locale, t, data?.services.ethw.data)}
          {data?.services.ethw.error ? (
            <p className="mt-4 text-sm text-[color:var(--cp-danger)] break-all">
              {data.services.ethw.error}
            </p>
          ) : null}
        </article>
      )
    case 'ord':
      return (
        <article className="console-card">
          <div className="mb-4">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('services.workspace.ordRuntimeTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('services.workspace.ordRuntimeBody')}
            </p>
          </div>
          {renderOrdDetails(locale, t, data?.services.ord.data, data?.services.ord.rpc_url)}
          <div className="mt-5 rounded-[20px] border border-[color:var(--cp-border)] bg-[color:var(--cp-surface)] px-4 py-4">
            <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
              {t('services.workspace.ordCapabilityTitle')}
            </h4>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {data?.capabilities.ord_available
                ? t('services.workspace.ordCapabilityEnabled')
                : t('services.workspace.ordCapabilityReadOnly')}
            </p>
          </div>
          {data?.services.ord.error ? (
            <p className="mt-4 text-sm text-[color:var(--cp-danger)] break-all">
              {data.services.ord.error}
            </p>
          ) : null}
        </article>
      )
  }
}

function getProbe(
  serviceId: ServiceId,
  data: OverviewResponse | undefined,
): ServiceProbe<unknown> | undefined {
  if (!data) return undefined

  switch (serviceId) {
    case 'btc-node':
      return data.services.btc_node
    case 'balance-history':
      return data.services.balance_history
    case 'usdb-indexer':
      return data.services.usdb_indexer
    case 'ethw':
      return data.services.ethw
    case 'ord':
      return data.services.ord
  }
}

export function ServicesPage({ data, locale, t }: ServicesPageProps) {
  const params = useParams<{ serviceId?: string }>()
  const selectedService = getSelectedService(params.serviceId)
  const meta = getServiceMeta(selectedService, t)
  const selectedProbe = getProbe(selectedService, data)

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

      <section className="grid gap-5 lg:grid-cols-[minmax(280px,320px)_minmax(0,1fr)]">
        <aside className="console-card h-fit min-w-0 overflow-hidden">
          <div className="mb-4">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('services.workspace.selectorTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('services.workspace.selectorBody')}
            </p>
          </div>

          <div className="grid gap-3">
            {SERVICE_IDS.map((serviceId) => {
              const probe = getProbe(serviceId, data)
              const serviceMeta = getServiceMeta(serviceId, t)
              const tone = probe ? serviceTone(probe) : 'neutral'
              return (
                <NavLink
                  key={serviceId}
                  to={`/services/${serviceId}`}
                  className={({ isActive }) =>
                    isActive
                      ? 'console-service-selector active'
                      : 'console-service-selector'
                  }
                >
                  <div className="flex min-w-0 items-start justify-between gap-3">
                    <div className="min-w-0 flex-1">
                      <strong className="block text-sm font-semibold text-[color:var(--cp-text)]">
                        {serviceMeta.title}
                      </strong>
                      <p className="mt-2 break-words text-sm leading-6 text-[color:var(--cp-muted)]">
                        {getServiceSummaryLine(serviceId, data, locale, t)}
                      </p>
                    </div>
                    <span className="status-pill shrink-0" data-tone={tone}>
                      {probe ? serviceLabel(probe, t) : t('common.notYetAvailable')}
                    </span>
                  </div>
                </NavLink>
              )
            })}
          </div>
        </aside>

        <div className="grid min-w-0 gap-5">
          <section className="console-card min-w-0">
            <div className="flex flex-wrap items-start justify-between gap-4">
              <div className="min-w-0 flex-1">
                <p className="shell-kicker m-0">{t('services.workspace.kicker')}</p>
                <h3 className="mt-2 text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
                  {meta.headline}
                </h3>
                <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
                  {meta.body}
                </p>
              </div>
              {selectedProbe ? (
                <span className="status-pill" data-tone={serviceTone(selectedProbe)}>
                  {serviceLabel(selectedProbe, t)}
                </span>
              ) : null}
            </div>
          </section>

          {renderServiceContent(selectedService, data, locale, t)}
        </div>
      </section>
    </div>
  )
}
