import { useEffect, useMemo, useState, type FormEvent } from 'react'
import {
  fetchBalanceHistoryBatchBalances,
  fetchBalanceHistorySingleBalance,
  fetchBalanceHistorySyncStatus,
} from '../lib/api'
import { displayNumber, displayText } from '../lib/format'
import type { AddressBalanceRow, BalanceHistorySummary, BalanceHistorySyncStatus, OverviewResponse } from '../lib/types'
import { FieldValueList } from '../components/FieldValueList'

interface BalanceHistoryExplorerPageProps {
  data?: OverviewResponse
  locale: string
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
  embedded?: boolean
}

type QueryMode = 'latest' | 'height' | 'range'
type QueryTarget = 'single' | 'batch'

function buildSelector(
  mode: QueryMode,
  heightInput: string,
  rangeStartInput: string,
  rangeEndInput: string,
) {
  if (mode === 'height') {
    const height = Number(heightInput)
    if (!Number.isInteger(height) || height < 0) {
      throw new Error('A non-negative height is required.')
    }
    return {
      block_height: height,
      block_range: null,
    }
  }

  if (mode === 'range') {
    const start = Number(rangeStartInput)
    const end = Number(rangeEndInput)
    if (!Number.isInteger(start) || !Number.isInteger(end) || start < 0 || end <= start) {
      throw new Error('A valid range is required and end must be greater than start.')
    }
    return {
      block_height: null,
      block_range: {
        start,
        end,
      },
    }
  }

  return {
    block_height: null,
    block_range: null,
  }
}

function summarizeRows(rows: AddressBalanceRow[]) {
  if (rows.length === 0) {
    return null
  }

  const latest = rows[rows.length - 1]
  const net = rows.reduce((acc, row) => acc + row.delta, 0)
  return {
    recordCount: rows.length,
    latestHeight: latest.block_height,
    latestBalance: latest.balance,
    netDelta: net,
  }
}

function summarizeBatch(scriptHashes: string[], rowsList: AddressBalanceRow[][]) {
  return scriptHashes.map((scriptHash, index) => {
    const rows = rowsList[index] ?? []
    const latest = rows[rows.length - 1]
    const net = rows.reduce((acc, row) => acc + row.delta, 0)
    return {
      scriptHash,
      recordCount: rows.length,
      latestHeight: latest?.block_height ?? null,
      latestBalance: latest?.balance ?? null,
      netDelta: net,
    }
  })
}

export function BalanceHistoryExplorerPage({
  data,
  locale,
  t,
  embedded = false,
}: BalanceHistoryExplorerPageProps) {
  const summary = data?.services.balance_history.data as BalanceHistorySummary | undefined
  const [syncStatus, setSyncStatus] = useState<BalanceHistorySyncStatus | null>(null)
  const [syncError, setSyncError] = useState<string | null>(null)
  const [queryTarget, setQueryTarget] = useState<QueryTarget>('single')

  const [singleScriptHash, setSingleScriptHash] = useState('')
  const [singleRows, setSingleRows] = useState<AddressBalanceRow[]>([])
  const [singleError, setSingleError] = useState<string | null>(null)
  const [singleLoading, setSingleLoading] = useState(false)

  const [batchScriptHashes, setBatchScriptHashes] = useState('')
  const [batchRows, setBatchRows] = useState<AddressBalanceRow[][]>([])
  const [batchError, setBatchError] = useState<string | null>(null)
  const [batchLoading, setBatchLoading] = useState(false)

  const [queryMode, setQueryMode] = useState<QueryMode>('latest')
  const [queryHeight, setQueryHeight] = useState('')
  const [queryRangeStart, setQueryRangeStart] = useState('')
  const [queryRangeEnd, setQueryRangeEnd] = useState('')

  useEffect(() => {
    let cancelled = false
    void fetchBalanceHistorySyncStatus()
      .then((status) => {
        if (cancelled) return
        setSyncStatus(status)
        setSyncError(null)
      })
      .catch((error: Error) => {
        if (cancelled) return
        setSyncStatus(null)
        setSyncError(error.message)
      })

    return () => {
      cancelled = true
    }
  }, [])

  const singleSummary = useMemo(() => summarizeRows(singleRows), [singleRows])
  const batchItems = useMemo(
    () =>
      summarizeBatch(
        batchScriptHashes
          .split('\n')
          .map((item) => item.trim())
          .filter(Boolean),
        batchRows,
      ),
    [batchRows, batchScriptHashes],
  )

  async function handleSingleQuery(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setSingleLoading(true)
    setSingleError(null)

    try {
      if (!singleScriptHash.trim()) {
        throw new Error('A script hash is required.')
      }

      const selector = buildSelector(queryMode, queryHeight, queryRangeStart, queryRangeEnd)
      const rows = await fetchBalanceHistorySingleBalance({
        script_hash: singleScriptHash.trim(),
        ...selector,
      })
      setSingleRows(rows)
    } catch (error) {
      setSingleRows([])
      setSingleError(error instanceof Error ? error.message : String(error))
    } finally {
      setSingleLoading(false)
    }
  }

  async function handleBatchQuery(event: FormEvent<HTMLFormElement>) {
    event.preventDefault()
    setBatchLoading(true)
    setBatchError(null)

    try {
      const scriptHashes = batchScriptHashes
        .split('\n')
        .map((item) => item.trim())
        .filter(Boolean)
      if (scriptHashes.length === 0) {
        throw new Error('At least one script hash is required.')
      }

      const selector = buildSelector(queryMode, queryHeight, queryRangeStart, queryRangeEnd)
      const rows = await fetchBalanceHistoryBatchBalances({
        script_hashes: scriptHashes,
        ...selector,
      })
      setBatchRows(rows)
    } catch (error) {
      setBatchRows([])
      setBatchError(error instanceof Error ? error.message : String(error))
    } finally {
      setBatchLoading(false)
    }
  }

  return (
    <div className="grid gap-5">
      {!embedded ? (
        <section className="console-page-intro">
          <div className="flex flex-wrap items-start justify-between gap-4">
            <div>
              <p className="shell-kicker m-0">{t('services.balanceHistory.kicker')}</p>
              <h2 className="mt-2 text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
                {t('services.balanceHistory.title')}
              </h2>
              <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
                {t('services.balanceHistory.subtitle')}
              </p>
            </div>
          </div>
        </section>
      ) : null}

      <article className="console-card">
        <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
          {t('services.balanceHistory.runtimeTitle')}
        </h3>
        <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
          {t('services.balanceHistory.runtimeBody')}
        </p>
        <div className="mt-4">
          <FieldValueList
            items={[
              {
                label: t('fields.network'),
                value: displayText(summary?.network, t),
                helpText: t('help.fields.network'),
              },
              {
                label: t('fields.stableHeight'),
                value: displayNumber(locale, summary?.stable_height ?? null, t),
                helpText: t('help.fields.stableHeight'),
              },
              {
                label: t('fields.phase'),
                value: displayText(syncStatus?.phase ?? summary?.phase, t),
                helpText: t('help.fields.phase'),
              },
              {
                label: t('fields.statusMessage'),
                value: displayText(syncStatus?.message ?? summary?.message, t),
                helpText: t('help.fields.statusMessage'),
              },
            ]}
          />
        </div>
        {syncError ? (
          <p className="mt-4 text-sm text-[color:var(--cp-danger)]">{syncError}</p>
        ) : null}
      </article>

      <article className="console-card">
        <div className="mb-5">
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
            {t('services.balanceHistory.queryWorkspaceTitle')}
          </h3>
          <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
            {t('services.balanceHistory.queryWorkspaceBody')}
          </p>
        </div>

        <div className="grid gap-5 xl:grid-cols-[minmax(320px,420px)_minmax(0,1fr)]">
          <section className="grid gap-4">
            <div className="console-subtle-card">
              <p className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('services.balanceHistory.queryTarget')}
              </p>
              <div className="mt-3 grid gap-3 sm:grid-cols-2">
                <button
                  type="button"
                  className={
                    queryTarget === 'single'
                      ? 'console-action-button w-full'
                      : 'console-secondary-button w-full'
                  }
                  onClick={() => setQueryTarget('single')}
                >
                  {t('services.balanceHistory.queryTargetSingle')}
                </button>
                <button
                  type="button"
                  className={
                    queryTarget === 'batch'
                      ? 'console-action-button w-full'
                      : 'console-secondary-button w-full'
                  }
                  onClick={() => setQueryTarget('batch')}
                >
                  {t('services.balanceHistory.queryTargetBatch')}
                </button>
              </div>
            </div>

            <form
              className="grid gap-4"
              onSubmit={queryTarget === 'single' ? handleSingleQuery : handleBatchQuery}
            >
              {queryTarget === 'single' ? (
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('services.balanceHistory.scriptHash')}</span>
                  <input
                    className="console-input"
                    value={singleScriptHash}
                    onChange={(event) => setSingleScriptHash(event.target.value)}
                    placeholder={t('services.balanceHistory.scriptHashPlaceholder')}
                  />
                </label>
              ) : (
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('services.balanceHistory.scriptHashes')}</span>
                  <textarea
                    className="console-textarea"
                    value={batchScriptHashes}
                    onChange={(event) => setBatchScriptHashes(event.target.value)}
                    placeholder={t('services.balanceHistory.scriptHashesPlaceholder')}
                    rows={6}
                  />
                </label>
              )}

              <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                <span>{t('services.balanceHistory.queryMode')}</span>
                <select
                  className="console-select"
                  value={queryMode}
                  onChange={(event) => setQueryMode(event.target.value as QueryMode)}
                >
                  <option value="latest">{t('services.balanceHistory.mode.latest')}</option>
                  <option value="height">{t('services.balanceHistory.mode.height')}</option>
                  <option value="range">{t('services.balanceHistory.mode.range')}</option>
                </select>
              </label>

              {queryMode === 'height' ? (
                <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                  <span>{t('services.balanceHistory.height')}</span>
                  <input
                    className="console-input"
                    inputMode="numeric"
                    value={queryHeight}
                    onChange={(event) => setQueryHeight(event.target.value)}
                  />
                </label>
              ) : null}

              {queryMode === 'range' ? (
                <div className="grid gap-4 sm:grid-cols-2">
                  <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                    <span>{t('services.balanceHistory.rangeStart')}</span>
                    <input
                      className="console-input"
                      inputMode="numeric"
                      value={queryRangeStart}
                      onChange={(event) => setQueryRangeStart(event.target.value)}
                    />
                  </label>
                  <label className="grid gap-2 text-sm font-medium text-[color:var(--cp-text)]">
                    <span>{t('services.balanceHistory.rangeEnd')}</span>
                    <input
                      className="console-input"
                      inputMode="numeric"
                      value={queryRangeEnd}
                      onChange={(event) => setQueryRangeEnd(event.target.value)}
                    />
                  </label>
                </div>
              ) : null}

              <div className="console-subtle-card">
                <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                  {t('services.balanceHistory.queryModesTitle')}
                </h4>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('services.balanceHistory.queryModesBody')}
                </p>
                <ul className="mt-3 grid gap-3 text-sm text-[color:var(--cp-muted)]">
                  <li>{t('services.balanceHistory.queryModeLatest')}</li>
                  <li>{t('services.balanceHistory.queryModeHeight')}</li>
                  <li>{t('services.balanceHistory.queryModeRange')}</li>
                </ul>
              </div>

              <div className="flex flex-wrap items-center gap-3">
                <button
                  type="submit"
                  className="console-action-button"
                  disabled={queryTarget === 'single' ? singleLoading : batchLoading}
                >
                  {queryTarget === 'single'
                    ? singleLoading
                      ? t('actions.reloading')
                      : t('services.balanceHistory.runQuery')
                    : batchLoading
                      ? t('actions.reloading')
                      : t('services.balanceHistory.runBatchQuery')}
                </button>
                {queryTarget === 'single' && singleError ? (
                  <span className="text-sm text-[color:var(--cp-danger)]">{singleError}</span>
                ) : null}
                {queryTarget === 'batch' && batchError ? (
                  <span className="text-sm text-[color:var(--cp-danger)]">{batchError}</span>
                ) : null}
              </div>
            </form>
          </section>

          <section className="grid min-w-0 gap-4">
            <div>
              <h4 className="text-base font-semibold text-[color:var(--cp-text)]">
                {queryTarget === 'single'
                  ? t('services.balanceHistory.singleTitle')
                  : t('services.balanceHistory.batchTitle')}
              </h4>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {queryTarget === 'single'
                  ? t('services.balanceHistory.singleBody')
                  : t('services.balanceHistory.batchBody')}
              </p>
            </div>

            {queryTarget === 'single' ? (
              <div className="grid gap-3">
                {singleSummary ? (
                  <div className="console-subtle-card">
                    <p className="text-sm leading-6 text-[color:var(--cp-muted)]">
                      {t('services.balanceHistory.singleSummary', undefined, {
                        count: singleSummary.recordCount,
                        latestHeight: singleSummary.latestHeight,
                        latestBalance: singleSummary.latestBalance,
                        netDelta: singleSummary.netDelta,
                      })}
                    </p>
                  </div>
                ) : null}

                <div className="overflow-x-auto">
                  <table className="console-table">
                    <thead>
                      <tr>
                        <th>{t('fields.blocks')}</th>
                        <th>{t('services.balanceHistory.delta')}</th>
                        <th>{t('services.balanceHistory.balance')}</th>
                      </tr>
                    </thead>
                    <tbody>
                      {singleRows.length === 0 ? (
                        <tr>
                          <td colSpan={3}>{t('services.balanceHistory.noRows')}</td>
                        </tr>
                      ) : (
                        singleRows.map((row) => (
                          <tr key={`${row.block_height}:${row.delta}:${row.balance}`}>
                            <td>{displayNumber(locale, row.block_height, t)}</td>
                            <td>{displayNumber(locale, row.delta, t)}</td>
                            <td>{displayNumber(locale, row.balance, t)}</td>
                          </tr>
                        ))
                      )}
                    </tbody>
                  </table>
                </div>
              </div>
            ) : (
              <div className="overflow-x-auto">
                <table className="console-table">
                  <thead>
                    <tr>
                      <th>{t('services.balanceHistory.scriptHash')}</th>
                      <th>{t('services.balanceHistory.records')}</th>
                      <th>{t('services.balanceHistory.latestHeight')}</th>
                      <th>{t('services.balanceHistory.latestBalance')}</th>
                      <th>{t('services.balanceHistory.netDelta')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {batchItems.length === 0 ? (
                      <tr>
                        <td colSpan={5}>{t('services.balanceHistory.noRows')}</td>
                      </tr>
                    ) : (
                      batchItems.map((item) => (
                        <tr key={item.scriptHash}>
                          <td className="break-all">{item.scriptHash}</td>
                          <td>{displayNumber(locale, item.recordCount, t)}</td>
                          <td>{displayNumber(locale, item.latestHeight, t)}</td>
                          <td>{displayNumber(locale, item.latestBalance, t)}</td>
                          <td>{displayNumber(locale, item.netDelta, t)}</td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>
            )}
          </section>
        </div>
      </article>
    </div>
  )
}
