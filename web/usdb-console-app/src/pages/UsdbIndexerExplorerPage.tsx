import { useEffect, useMemo, useState, type FormEvent } from 'react'
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
import { FieldValueList } from '../components/FieldValueList'
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
  PassEnergyRangePage,
  PassEnergySnapshot,
  PassHistoryPage,
  PassSnapshot,
  PassStatsAtHeight,
  RpcActiveBalanceSnapshot,
  UsdbIndexerSummary,
  UsdbIndexerSyncStatus,
  UsdbRpcInfo,
} from '../lib/types'

interface UsdbIndexerExplorerPageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
  embedded?: boolean
}

type EnergyScope = 'active' | 'active_dormant' | 'all'

export function UsdbIndexerExplorerPage({
  data,
  locale,
  t,
  embedded = false,
}: UsdbIndexerExplorerPageProps) {
  const summary = data?.services.usdb_indexer.data as UsdbIndexerSummary | undefined

  const [rpcInfo, setRpcInfo] = useState<UsdbRpcInfo | null>(null)
  const [syncStatus, setSyncStatus] = useState<UsdbIndexerSyncStatus | null>(null)
  const [passStats, setPassStats] = useState<PassStatsAtHeight | null>(null)
  const [activeBalanceSnapshot, setActiveBalanceSnapshot] =
    useState<RpcActiveBalanceSnapshot | null>(null)
  const [homeError, setHomeError] = useState<string | null>(null)

  const [passInscriptionId, setPassInscriptionId] = useState('')
  const [passAtHeight, setPassAtHeight] = useState('')
  const [passSnapshot, setPassSnapshot] = useState<PassSnapshot | null>(null)
  const [passCommit, setPassCommit] = useState<PassBlockCommitInfo | null>(null)
  const [passHistory, setPassHistory] = useState<PassHistoryPage | null>(null)
  const [passHistoryPage, setPassHistoryPage] = useState(0)
  const [passError, setPassError] = useState<string | null>(null)
  const [passLoading, setPassLoading] = useState(false)

  const [leaderboardScope, setLeaderboardScope] = useState<EnergyScope>('active')
  const [leaderboardPage, setLeaderboardPage] = useState(0)
  const [leaderboard, setLeaderboard] = useState<PassEnergyLeaderboardPage | null>(null)
  const [leaderboardError, setLeaderboardError] = useState<string | null>(null)

  const [energyInscriptionId, setEnergyInscriptionId] = useState('')
  const [energyBlockHeight, setEnergyBlockHeight] = useState('')
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

    void fetchUsdbPassEnergyLeaderboard(leaderboardScope, leaderboardPage, 25)
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
      25,
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

  const passTotalPages = useMemo(() => {
    if (!passHistory) return 1
    return Math.max(1, Math.ceil(passHistory.total / 20))
  }, [passHistory])

  const leaderboardTotalPages = useMemo(() => {
    if (!leaderboard) return 1
    return Math.max(1, Math.ceil(leaderboard.total / 25))
  }, [leaderboard])

  const energyRangeTotalPages = useMemo(() => {
    if (!energyRange) return 1
    return Math.max(1, Math.ceil(energyRange.total / 25))
  }, [energyRange])

  async function handlePassQuery(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setPassLoading(true)
    setPassError(null)
    setPassHistory(null)
    setPassCommit(null)

    try {
      if (!passInscriptionId.trim()) {
        throw new Error('An inscription ID is required.')
      }

      const parsedHeight =
        passAtHeight.trim() === '' ? null : Number.parseInt(passAtHeight.trim(), 10)
      if (passAtHeight.trim() !== '' && (!Number.isInteger(parsedHeight) || parsedHeight! < 0)) {
        throw new Error('Height must be a non-negative integer.')
      }

      const snapshot = await fetchUsdbPassSnapshot(passInscriptionId.trim(), parsedHeight)
      if (!snapshot) {
        throw new Error('The requested pass is not visible at that height.')
      }

      setPassSnapshot(snapshot)
      setPassHistoryPage(0)

      const commit = await fetchUsdbPassBlockCommit(snapshot.resolved_height)
      setPassCommit(commit)
    } catch (error) {
      setPassSnapshot(null)
      setPassHistory(null)
      setPassCommit(null)
      setPassError(error instanceof Error ? error.message : String(error))
    } finally {
      setPassLoading(false)
    }
  }

  async function handleEnergyQuery(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setEnergyLoading(true)
    setEnergyError(null)
    setEnergyRange(null)

    try {
      if (!energyInscriptionId.trim()) {
        throw new Error('An inscription ID is required.')
      }

      const parsedHeight =
        energyBlockHeight.trim() === '' ? null : Number.parseInt(energyBlockHeight.trim(), 10)
      if (energyBlockHeight.trim() !== '' && (!Number.isInteger(parsedHeight) || parsedHeight! < 0)) {
        throw new Error('Height must be a non-negative integer.')
      }

      const snapshot = await fetchUsdbPassEnergy(
        energyInscriptionId.trim(),
        parsedHeight,
        'at_or_before',
      )

      setEnergySnapshot(snapshot)
      setEnergyRangePage(0)
    } catch (error) {
      setEnergySnapshot(null)
      setEnergyRange(null)
      setEnergyError(error instanceof Error ? error.message : String(error))
    } finally {
      setEnergyLoading(false)
    }
  }

  return (
    <div className="grid gap-5">
      {!embedded ? (
        <section className="console-page-intro">
          <div className="flex flex-wrap items-start justify-between gap-4">
            <div>
              <p className="shell-kicker m-0">{t('services.usdbIndexer.kicker')}</p>
              <h2 className="mt-2 text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
                {t('services.usdbIndexer.title')}
              </h2>
              <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
                {t('services.usdbIndexer.subtitle')}
              </p>
            </div>
          </div>
        </section>
      ) : null}

      <section className="grid gap-4 xl:grid-cols-2">
        <article className="console-card">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('services.usdbIndexer.runtimeTitle')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('services.usdbIndexer.runtimeBody')}
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
                  label: t('fields.statusMessage'),
                  value: displayText(syncStatus?.message ?? summary?.message, t),
                  helpText: t('help.fields.statusMessage'),
                },
              ]}
            />
          </div>
          {homeError ? (
            <p className="mt-4 text-sm text-[color:var(--cp-danger)]">{homeError}</p>
          ) : null}
        </article>

        <article className="console-card">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('services.usdbIndexer.passStatsTitle')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('services.usdbIndexer.passStatsBody')}
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
                  label: t('services.usdbIndexer.activeBalance'),
                  value: displayBalanceSmart(locale, activeBalanceSnapshot?.total_balance ?? null, t),
                },
              ]}
            />
          </div>
        </article>
      </section>

      <section className="grid gap-4 xl:grid-cols-2">
        <article className="console-card">
          <div className="mb-4">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('services.usdbIndexer.passTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('services.usdbIndexer.passBody')}
            </p>
          </div>

          <form className="grid gap-4" onSubmit={handlePassQuery}>
            <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
              <span>{t('services.usdbIndexer.inscriptionId')}</span>
              <input
                className="console-input"
                value={passInscriptionId}
                onChange={(event) => setPassInscriptionId(event.target.value)}
                placeholder={t('services.usdbIndexer.inscriptionPlaceholder')}
              />
            </label>

            <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
              <span>{t('services.usdbIndexer.atHeight')}</span>
              <input
                className="console-input"
                inputMode="numeric"
                value={passAtHeight}
                onChange={(event) => setPassAtHeight(event.target.value)}
                placeholder={t('services.usdbIndexer.atHeightPlaceholder')}
              />
            </label>

            <div className="flex items-center gap-3">
              <button type="submit" className="console-action-button" disabled={passLoading}>
                {passLoading ? t('actions.reloading') : t('services.usdbIndexer.runPassQuery')}
              </button>
              {passError ? (
                <span className="text-sm text-[color:var(--cp-danger)]">{passError}</span>
              ) : null}
            </div>
          </form>

          {passSnapshot ? (
            <div className="mt-5 grid gap-4">
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
                    total: passTotalPages,
                  })}
                </span>
                <button
                  type="button"
                  className="console-secondary-button"
                  disabled={passHistoryPage + 1 >= passTotalPages}
                  onClick={() => setPassHistoryPage((current) => current + 1)}
                >
                  {t('actions.nextPage')}
                </button>
              </div>
            </div>
          ) : null}
        </article>

        <article className="console-card">
          <div className="mb-4">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('services.usdbIndexer.energyTitle')}
            </h3>
            <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('services.usdbIndexer.energyBody')}
            </p>
          </div>

          <div className="grid gap-4">
            <div className="console-subtle-card">
              <div className="flex flex-wrap items-center justify-between gap-3">
                <div>
                  <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                    {t('services.usdbIndexer.leaderboardTitle')}
                  </h4>
                  <p className="mt-1 text-sm text-[color:var(--cp-muted)]">
                    {t('services.usdbIndexer.leaderboardBody')}
                  </p>
                </div>

                <select
                  className="console-select"
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
                <p className="mt-3 text-sm text-[color:var(--cp-danger)]">{leaderboardError}</p>
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
                            setEnergyInscriptionId(item.inscription_id)
                          }}
                        >
                          <td>{displayNumber(locale, leaderboardPage * 25 + index + 1, t)}</td>
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
            </div>

            <form className="grid gap-4" onSubmit={handleEnergyQuery}>
              <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                <span>{t('services.usdbIndexer.inscriptionId')}</span>
                <input
                  className="console-input"
                  value={energyInscriptionId}
                  onChange={(event) => setEnergyInscriptionId(event.target.value)}
                  placeholder={t('services.usdbIndexer.inscriptionPlaceholder')}
                />
              </label>

              <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                <span>{t('services.usdbIndexer.energyHeight')}</span>
                <input
                  className="console-input"
                  inputMode="numeric"
                  value={energyBlockHeight}
                  onChange={(event) => setEnergyBlockHeight(event.target.value)}
                  placeholder={t('services.usdbIndexer.atHeightPlaceholder')}
                />
              </label>

              <div className="flex items-center gap-3">
                <button type="submit" className="console-action-button" disabled={energyLoading}>
                  {energyLoading ? t('actions.reloading') : t('services.usdbIndexer.runEnergyQuery')}
                </button>
                {energyError ? (
                  <span className="text-sm text-[color:var(--cp-danger)]">{energyError}</span>
                ) : null}
              </div>
            </form>

            {energySnapshot ? (
              <div className="grid gap-4">
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
                    ]}
                  />
                </div>

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
            ) : null}
          </div>
        </article>
      </section>
    </div>
  )
}
