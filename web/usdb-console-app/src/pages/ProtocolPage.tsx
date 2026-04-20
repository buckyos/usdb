import { useEffect, useMemo, useState, type FormEvent } from 'react'
import { FieldValueList } from '../components/FieldValueList'
import {
  fetchUsdbLatestActiveBalanceSnapshot,
  fetchUsdbPassBlockCommit,
  fetchUsdbPassEnergy,
  fetchUsdbPassEnergyLeaderboard,
  fetchUsdbPassEnergyRange,
  fetchUsdbPassHistory,
  fetchUsdbPassSnapshot,
  fetchUsdbPassStats,
  fetchUsdbRpcInfo,
  fetchUsdbSyncStatus,
} from '../lib/api'
import {
  displayBalanceDeltaSmart,
  displayBalanceSmart,
  displayNumber,
  displayText,
} from '../lib/format'
import type {
  OverviewResponse,
  PassBlockCommitInfo,
  PassEnergyLeaderboardPage,
  PassEnergySnapshot,
  PassEnergyRangePage,
  PassHistoryPage,
  PassSnapshot,
  PassStatsAtHeight,
  RpcActiveBalanceSnapshot,
  UsdbIndexerSummary,
  UsdbIndexerSyncStatus,
  UsdbRpcInfo,
} from '../lib/types'

interface ProtocolPageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

type EnergyScope = 'active' | 'active_dormant' | 'all'
type QueryTarget = 'pass' | 'energy'

export function ProtocolPage({ data, locale, t }: ProtocolPageProps) {
  const summary = data?.services.usdb_indexer.data as UsdbIndexerSummary | undefined

  const [rpcInfo, setRpcInfo] = useState<UsdbRpcInfo | null>(null)
  const [syncStatus, setSyncStatus] = useState<UsdbIndexerSyncStatus | null>(null)
  const [passStats, setPassStats] = useState<PassStatsAtHeight | null>(null)
  const [activeBalanceSnapshot, setActiveBalanceSnapshot] =
    useState<RpcActiveBalanceSnapshot | null>(null)
  const [homeError, setHomeError] = useState<string | null>(null)

  const [leaderboardScope, setLeaderboardScope] = useState<EnergyScope>('active')
  const [leaderboardPage, setLeaderboardPage] = useState(0)
  const [leaderboard, setLeaderboard] = useState<PassEnergyLeaderboardPage | null>(null)
  const [leaderboardError, setLeaderboardError] = useState<string | null>(null)

  const [queryTarget, setQueryTarget] = useState<QueryTarget>('pass')
  const [inscriptionId, setInscriptionId] = useState('')
  const [atHeight, setAtHeight] = useState('')

  const [passSnapshot, setPassSnapshot] = useState<PassSnapshot | null>(null)
  const [passCommit, setPassCommit] = useState<PassBlockCommitInfo | null>(null)
  const [passHistory, setPassHistory] = useState<PassHistoryPage | null>(null)
  const [passHistoryPage, setPassHistoryPage] = useState(0)
  const [passError, setPassError] = useState<string | null>(null)
  const [passLoading, setPassLoading] = useState(false)

  const [energySnapshot, setEnergySnapshot] = useState<PassEnergySnapshot | null>(null)
  const [energyRange, setEnergyRange] = useState<PassEnergyRangePage | null>(null)
  const [energyRangePage, setEnergyRangePage] = useState(0)
  const [energyError, setEnergyError] = useState<string | null>(null)
  const [energyLoading, setEnergyLoading] = useState(false)

  useEffect(() => {
    let cancelled = false

    void Promise.all([
      fetchUsdbRpcInfo(),
      fetchUsdbSyncStatus(),
      fetchUsdbPassStats(null),
      fetchUsdbLatestActiveBalanceSnapshot(),
    ])
      .then(([rpcInfoResult, syncStatusResult, passStatsResult, activeBalanceResult]) => {
        if (cancelled) return
        setRpcInfo(rpcInfoResult)
        setSyncStatus(syncStatusResult)
        setPassStats(passStatsResult)
        setActiveBalanceSnapshot(activeBalanceResult)
        setHomeError(null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setHomeError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [])

  useEffect(() => {
    let cancelled = false

    void fetchUsdbPassEnergyLeaderboard(leaderboardScope, leaderboardPage, 20)
      .then((page) => {
        if (cancelled) return
        setLeaderboard(page)
        setLeaderboardError(null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setLeaderboard(null)
        setLeaderboardError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [leaderboardPage, leaderboardScope])

  useEffect(() => {
    if (!passSnapshot) return
    let cancelled = false

    void fetchUsdbPassHistory(
      passSnapshot.inscription_id,
      passSnapshot.mint_block_height,
      passSnapshot.resolved_height,
      passHistoryPage,
      20,
      'desc',
    )
      .then((page) => {
        if (cancelled) return
        setPassHistory(page)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setPassError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [passHistoryPage, passSnapshot])

  useEffect(() => {
    if (!energySnapshot) return
    let cancelled = false

    void fetchUsdbPassEnergyRange(
      energySnapshot.inscription_id,
      energySnapshot.active_block_height,
      energySnapshot.record_block_height,
      energyRangePage,
      20,
      'desc',
    )
      .then((page) => {
        if (cancelled) return
        setEnergyRange(page)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setEnergyError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [energyRangePage, energySnapshot])

  const leaderboardTotalPages = useMemo(() => {
    if (!leaderboard) return 1
    return Math.max(1, Math.ceil(leaderboard.total / 20))
  }, [leaderboard])

  const passHistoryTotalPages = useMemo(() => {
    if (!passHistory) return 1
    return Math.max(1, Math.ceil(passHistory.total / 20))
  }, [passHistory])

  const energyRangeTotalPages = useMemo(() => {
    if (!energyRange) return 1
    return Math.max(1, Math.ceil(energyRange.total / 20))
  }, [energyRange])

  function parseHeightInput(rawValue: string) {
    const trimmed = rawValue.trim()
    if (trimmed === '') return null
    const parsed = Number.parseInt(trimmed, 10)
    if (!Number.isInteger(parsed) || parsed < 0) {
      throw new Error(t('protocol.lookup.invalidHeight'))
    }
    return parsed
  }

  async function handlePassQuery(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setPassLoading(true)
    setPassError(null)
    setPassSnapshot(null)
    setPassCommit(null)
    setPassHistory(null)

    try {
      const target = inscriptionId.trim()
      if (!target) throw new Error(t('protocol.lookup.inscriptionRequired'))

      const parsedHeight = parseHeightInput(atHeight)
      const snapshot = await fetchUsdbPassSnapshot(target, parsedHeight)
      if (!snapshot) {
        throw new Error(t('protocol.lookup.passNotFound'))
      }

      setPassSnapshot(snapshot)
      setPassHistoryPage(0)
      const commit = await fetchUsdbPassBlockCommit(snapshot.resolved_height)
      setPassCommit(commit)
    } catch (error) {
      setPassError(error instanceof Error ? error.message : String(error))
    } finally {
      setPassLoading(false)
    }
  }

  async function handleEnergyQuery(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setEnergyLoading(true)
    setEnergyError(null)
    setEnergySnapshot(null)
    setEnergyRange(null)

    try {
      const target = inscriptionId.trim()
      if (!target) throw new Error(t('protocol.lookup.inscriptionRequired'))

      const parsedHeight = parseHeightInput(atHeight)
      const snapshot = await fetchUsdbPassEnergy(target, parsedHeight, 'at_or_before')
      setEnergySnapshot(snapshot)
      setEnergyRangePage(0)
    } catch (error) {
      setEnergyError(error instanceof Error ? error.message : String(error))
    } finally {
      setEnergyLoading(false)
    }
  }

  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
          {t('pages.protocol.title')}
        </h2>
        <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {t('pages.protocol.subtitle')}
        </p>
      </section>

      <section className="console-card">
        <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
          {t('pages.protocol.snapshotTitle')}
        </h3>
        <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
          {t('pages.protocol.snapshotBody')}
        </p>

        <div className="mt-5 grid gap-4">
          <div className="grid gap-4 xl:grid-cols-2">
            <div className="console-subtle-card">
              <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('pages.protocol.indexStateTitle')}
              </h4>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('pages.protocol.indexStateBody')}
              </p>
              <div className="mt-4">
                <FieldValueList
                  items={[
                    {
                      label: t('fields.network'),
                      value: displayText(rpcInfo?.network ?? summary?.network, t),
                      helpText: t('help.fields.network'),
                    },
                    {
                      label: t('fields.syncedHeight'),
                      value: displayNumber(
                        locale,
                        syncStatus?.synced_block_height ?? summary?.synced_block_height ?? null,
                        t,
                      ),
                      helpText: t('help.fields.syncedHeight'),
                    },
                    {
                      label: t('fields.stableHeight'),
                      value: displayNumber(
                        locale,
                        syncStatus?.balance_history_stable_height ??
                          summary?.balance_history_stable_height ??
                          null,
                        t,
                      ),
                      helpText: t('help.fields.stableHeight'),
                    },
                    {
                      label: t('fields.consensus'),
                      value: displayText(summary?.consensus_ready, t),
                      helpText: t('help.fields.consensus'),
                    },
                    {
                      label: t('fields.statusMessage'),
                      value: displayText(syncStatus?.message ?? summary?.message, t),
                      helpText: t('help.fields.statusMessage'),
                    },
                  ]}
                />
              </div>
            </div>

            <div className="console-subtle-card">
              <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('pages.protocol.activityTitle')}
              </h4>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('pages.protocol.activityBody')}
              </p>
              <div className="mt-4">
                <FieldValueList
                  items={[
                    {
                      label: t('services.usdbIndexer.activePasses'),
                      value: displayNumber(locale, passStats?.active_count ?? null, t),
                    },
                    {
                      label: t('services.usdbIndexer.totalPasses'),
                      value: displayNumber(locale, passStats?.total_count ?? null, t),
                    },
                    {
                      label: t('services.usdbIndexer.invalidPasses'),
                      value: displayNumber(locale, passStats?.invalid_count ?? null, t),
                    },
                    {
                      label: t('services.usdbIndexer.activeAddresses'),
                      value: displayNumber(
                        locale,
                        activeBalanceSnapshot?.active_address_count ?? null,
                        t,
                      ),
                    },
                    {
                      label: t('services.usdbIndexer.activeBalance'),
                      value: displayBalanceSmart(
                        locale,
                        activeBalanceSnapshot?.total_balance ?? null,
                        t,
                      ),
                    },
                  ]}
                />
              </div>
            </div>
          </div>

          <div className="console-subtle-card">
            <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
              {t('pages.protocol.consistencyTitle')}
            </h4>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('pages.protocol.consistencyBody')}
            </p>
            <div className="mt-4">
              <FieldValueList
                items={[
                  {
                    label: t('fields.upstreamSnapshot'),
                    value: displayText(summary?.upstream_snapshot_id, t),
                    helpText: t('help.fields.upstreamSnapshot'),
                  },
                  {
                    label: t('fields.localStateCommit'),
                    value: displayText(summary?.local_state_commit, t),
                    helpText: t('help.fields.localStateCommit'),
                  },
                  {
                    label: t('fields.systemState'),
                    value: displayText(summary?.system_state_id, t),
                    helpText: t('help.fields.systemState'),
                  },
                ]}
              />
            </div>
          </div>
        </div>

        {homeError ? (
          <p className="mt-4 text-sm text-[color:var(--cp-danger)]">{homeError}</p>
        ) : null}
      </section>

      <section className="console-card">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('pages.protocol.leaderboardTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('pages.protocol.leaderboardBody')}
            </p>
          </div>
          <select
            className="console-select min-w-[180px]"
            value={leaderboardScope}
            onChange={(event) => {
              setLeaderboardScope(event.target.value as EnergyScope)
              setLeaderboardPage(0)
            }}
          >
            <option value="active">{t('services.usdbIndexer.scope.active')}</option>
            <option value="active_dormant">{t('services.usdbIndexer.scope.activeDormant')}</option>
            <option value="all">{t('services.usdbIndexer.scope.all')}</option>
          </select>
        </div>

        {leaderboardError ? (
          <p className="mt-4 text-sm text-[color:var(--cp-danger)]">{leaderboardError}</p>
        ) : null}

        <div className="mt-4 overflow-x-auto">
          <table className="console-table">
            <thead>
              <tr>
                <th>{t('services.usdbIndexer.rank')}</th>
                <th>{t('services.usdbIndexer.energy')}</th>
                <th>{t('services.usdbIndexer.inscriptionId')}</th>
                <th>{t('services.usdbIndexer.state')}</th>
              </tr>
            </thead>
            <tbody>
              {!leaderboard || leaderboard.items.length === 0 ? (
                <tr>
                  <td colSpan={4}>{t('services.usdbIndexer.noRows')}</td>
                </tr>
              ) : (
                leaderboard.items.map((item, index) => (
                  <tr
                    key={`${item.inscription_id}:${item.record_block_height}`}
                    className="cursor-pointer"
                    onClick={() => {
                      setInscriptionId(item.inscription_id)
                      setQueryTarget('energy')
                    }}
                  >
                    <td>{displayNumber(locale, leaderboardPage * 20 + index + 1, t)}</td>
                    <td>{displayNumber(locale, item.energy, t)}</td>
                    <td className="break-all">{item.inscription_id}</td>
                    <td>{item.state}</td>
                  </tr>
                ))
              )}
            </tbody>
          </table>
        </div>

        <div className="mt-4 flex items-center justify-between gap-3">
          <button
            type="button"
            className="console-secondary-button"
            disabled={leaderboardPage === 0}
            onClick={() => setLeaderboardPage((current) => Math.max(0, current - 1))}
          >
            {t('actions.previousPage')}
          </button>
          <span className="text-sm text-[color:var(--cp-muted)]">
            {t('services.usdbIndexer.pageIndicator', undefined, {
              current: leaderboardPage + 1,
              total: leaderboardTotalPages,
            })}
          </span>
          <button
            type="button"
            className="console-secondary-button"
            disabled={leaderboardPage + 1 >= leaderboardTotalPages}
            onClick={() => setLeaderboardPage((current) => current + 1)}
          >
            {t('actions.nextPage')}
          </button>
        </div>
      </section>

      <section className="console-card">
        <div className="mb-5">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('pages.protocol.workspaceTitle')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('pages.protocol.workspaceBody')}
          </p>
        </div>

        <div className="grid gap-4">
          <div className="console-subtle-card">
            <p className="text-sm font-semibold text-[color:var(--cp-text)]">
              {t('pages.protocol.queryTargetTitle')}
            </p>
            <div className="mt-3 grid gap-3 sm:grid-cols-2">
              <button
                type="button"
                className={
                  queryTarget === 'pass'
                    ? 'console-action-button w-full'
                    : 'console-secondary-button w-full'
                }
                onClick={() => setQueryTarget('pass')}
              >
                {t('services.usdbIndexer.queryTargetPass')}
              </button>
              <button
                type="button"
                className={
                  queryTarget === 'energy'
                    ? 'console-action-button w-full'
                    : 'console-secondary-button w-full'
                }
                onClick={() => setQueryTarget('energy')}
              >
                {t('services.usdbIndexer.queryTargetEnergy')}
              </button>
            </div>
          </div>

          <form
            className="grid gap-4"
            onSubmit={queryTarget === 'pass' ? handlePassQuery : handleEnergyQuery}
          >
            <div className="console-subtle-card">
              <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {queryTarget === 'pass'
                  ? t('pages.protocol.passLookupTitle')
                  : t('pages.protocol.energyLookupTitle')}
              </h4>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {queryTarget === 'pass'
                  ? t('pages.protocol.passLookupBody')
                  : t('pages.protocol.energyLookupBody')}
              </p>
            </div>

            <div className="grid gap-4 xl:grid-cols-2">
              <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                <span>{t('services.usdbIndexer.inscriptionId')}</span>
                <input
                  className="console-input"
                  value={inscriptionId}
                  onChange={(event) => setInscriptionId(event.target.value)}
                  placeholder={t('services.usdbIndexer.inscriptionPlaceholder')}
                />
              </label>

              <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                <span>{t('services.usdbIndexer.atHeight')}</span>
                <input
                  className="console-input"
                  inputMode="numeric"
                  value={atHeight}
                  onChange={(event) => setAtHeight(event.target.value)}
                  placeholder={t('services.usdbIndexer.atHeightPlaceholder')}
                />
              </label>
            </div>

            <div className="flex flex-wrap items-center gap-3">
              <button
                type="submit"
                className="console-action-button"
                disabled={queryTarget === 'pass' ? passLoading : energyLoading}
              >
                {queryTarget === 'pass'
                  ? passLoading
                    ? t('actions.reloading')
                    : t('services.usdbIndexer.runPassQuery')
                  : energyLoading
                    ? t('actions.reloading')
                    : t('services.usdbIndexer.runEnergyQuery')}
              </button>
              {queryTarget === 'pass' && passError ? (
                <span className="text-sm text-[color:var(--cp-danger)]">{passError}</span>
              ) : null}
              {queryTarget === 'energy' && energyError ? (
                <span className="text-sm text-[color:var(--cp-danger)]">{energyError}</span>
              ) : null}
            </div>
          </form>

          {queryTarget === 'pass' ? (
            <div className="grid gap-4">
              {passSnapshot ? (
                <div className="console-subtle-card">
                  <FieldValueList
                    items={[
                      {
                        label: t('services.usdbIndexer.state'),
                        value: displayText(passSnapshot.state, t),
                      },
                      {
                        label: t('services.usdbIndexer.owner'),
                        value: displayText(passSnapshot.owner, t),
                      },
                      {
                        label: t('services.usdbIndexer.resolvedHeight'),
                        value: displayNumber(locale, passSnapshot.resolved_height, t),
                      },
                      {
                        label: t('services.usdbIndexer.ethMain'),
                        value: displayText(passSnapshot.eth_main, t),
                      },
                    ]}
                  />
                </div>
              ) : null}

              {passCommit ? (
                <div className="console-subtle-card">
                  <FieldValueList
                    items={[
                      {
                        label: t('services.usdbIndexer.commitHeight'),
                        value: displayNumber(locale, passCommit.block_height, t),
                      },
                      {
                        label: t('services.usdbIndexer.mutationRoot'),
                        value: displayText(passCommit.mutation_root, t),
                      },
                      {
                        label: t('fields.latestBlockCommit'),
                        value: displayText(passCommit.block_commit, t),
                      },
                    ]}
                  />
                </div>
              ) : null}

              <div className="overflow-x-auto">
                <table className="console-table">
                  <thead>
                    <tr>
                      <th>{t('services.usdbIndexer.eventHeight')}</th>
                      <th>{t('services.usdbIndexer.eventType')}</th>
                      <th>{t('services.usdbIndexer.state')}</th>
                      <th>{t('services.usdbIndexer.owner')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {!passHistory || passHistory.items.length === 0 ? (
                      <tr>
                        <td colSpan={4}>{t('services.usdbIndexer.noHistory')}</td>
                      </tr>
                    ) : (
                      passHistory.items.map((event) => (
                        <tr key={`${event.event_id}:${event.block_height}`}>
                          <td>{displayNumber(locale, event.block_height, t)}</td>
                          <td>{event.event_type}</td>
                          <td>{event.state}</td>
                          <td className="break-all">{event.owner}</td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>

              <div className="flex items-center justify-between gap-3">
                <button
                  type="button"
                  className="console-secondary-button"
                  disabled={passHistoryPage === 0}
                  onClick={() => setPassHistoryPage((current) => Math.max(0, current - 1))}
                >
                  {t('actions.previousPage')}
                </button>
                <span className="text-sm text-[color:var(--cp-muted)]">
                  {t('services.usdbIndexer.pageIndicator', undefined, {
                    current: passHistoryPage + 1,
                    total: passHistoryTotalPages,
                  })}
                </span>
                <button
                  type="button"
                  className="console-secondary-button"
                  disabled={passHistoryPage + 1 >= passHistoryTotalPages}
                  onClick={() => setPassHistoryPage((current) => current + 1)}
                >
                  {t('actions.nextPage')}
                </button>
              </div>
            </div>
          ) : (
            <div className="grid gap-4">
              {energySnapshot ? (
                <div className="console-subtle-card">
                  <FieldValueList
                    items={[
                      {
                        label: t('services.usdbIndexer.energy'),
                        value: displayNumber(locale, energySnapshot.energy, t),
                      },
                      {
                        label: t('services.usdbIndexer.recordHeight'),
                        value: displayNumber(locale, energySnapshot.record_block_height, t),
                      },
                      {
                        label: t('services.usdbIndexer.owner'),
                        value: displayText(energySnapshot.owner_address, t),
                      },
                      {
                        label: t('services.usdbIndexer.ownerBalance'),
                        value: displayBalanceSmart(locale, energySnapshot.owner_balance, t),
                      },
                      {
                        label: t('services.usdbIndexer.ownerDelta'),
                        value: displayBalanceDeltaSmart(locale, energySnapshot.owner_delta, t),
                      },
                    ]}
                  />
                </div>
              ) : null}

              <div className="overflow-x-auto">
                <table className="console-table">
                  <thead>
                    <tr>
                      <th>{t('services.usdbIndexer.recordHeight')}</th>
                      <th>{t('services.usdbIndexer.state')}</th>
                      <th>{t('services.usdbIndexer.ownerBalance')}</th>
                      <th>{t('services.usdbIndexer.ownerDelta')}</th>
                      <th>{t('services.usdbIndexer.energy')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {!energyRange || energyRange.items.length === 0 ? (
                      <tr>
                        <td colSpan={5}>{t('services.usdbIndexer.noRows')}</td>
                      </tr>
                    ) : (
                      energyRange.items.map((item) => (
                        <tr key={`${item.record_block_height}:${item.owner_address}`}>
                          <td>{displayNumber(locale, item.record_block_height, t)}</td>
                          <td>{item.state}</td>
                          <td>{displayBalanceSmart(locale, item.owner_balance, t)}</td>
                          <td>{displayBalanceDeltaSmart(locale, item.owner_delta, t)}</td>
                          <td>{displayNumber(locale, item.energy, t)}</td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>

              <div className="flex items-center justify-between gap-3">
                <button
                  type="button"
                  className="console-secondary-button"
                  disabled={energyRangePage === 0}
                  onClick={() => setEnergyRangePage((current) => Math.max(0, current - 1))}
                >
                  {t('actions.previousPage')}
                </button>
                <span className="text-sm text-[color:var(--cp-muted)]">
                  {t('services.usdbIndexer.pageIndicator', undefined, {
                    current: energyRangePage + 1,
                    total: energyRangeTotalPages,
                  })}
                </span>
                <button
                  type="button"
                  className="console-secondary-button"
                  disabled={energyRangePage + 1 >= energyRangeTotalPages}
                  onClick={() => setEnergyRangePage((current) => current + 1)}
                >
                  {t('actions.nextPage')}
                </button>
              </div>
            </div>
          )}
        </div>
      </section>
    </div>
  )
}
