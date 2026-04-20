import { ArtifactCard } from '../components/ArtifactCard'
import { BootstrapSteps } from '../components/BootstrapSteps'
import { FieldValueList } from '../components/FieldValueList'
import { artifactTone, type Tone } from '../lib/console'
import type {
  OverviewResponse,
  SourceDaoBootstrapModule,
  SourceDaoBootstrapState,
} from '../lib/types'

interface BootstrapPageProps {
  data?: OverviewResponse
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

function overallBootstrapTone(state?: string): Tone {
  if (state === 'completed') return 'success'
  if (state === 'error') return 'danger'
  if (state === 'skipped') return 'neutral'
  return 'warning'
}

function sourcedaoTone(state?: string | null): Tone {
  if (state === 'completed') return 'success'
  if (state === 'error') return 'danger'
  return 'warning'
}

function presentValue(value: unknown) {
  if (value === null || value === undefined || value === '') return '-'
  return String(value)
}

function formatTimestamp(value?: string | null) {
  if (!value) return '-'
  const parsed = new Date(value)
  if (Number.isNaN(parsed.getTime())) return value
  return parsed.toLocaleString(undefined, { hour12: false })
}

function humanizeKey(key: string) {
  return key
    .split('_')
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(' ')
}

function parseSourceDaoState(data?: Record<string, unknown> | null): SourceDaoBootstrapState | null {
  if (!data) return null
  return data as SourceDaoBootstrapState
}

function moduleFieldItems(
  module: SourceDaoBootstrapModule,
  t: BootstrapPageProps['t'],
) {
  return [
    {
      label: t('fields.address'),
      value: presentValue(module.address),
    },
    {
      label: t('fields.source'),
      value: presentValue(module.source),
    },
    {
      label: t('fields.implementationAddress'),
      value: presentValue(module.implementation_address),
    },
    {
      label: t('fields.proxyTxHash'),
      value: presentValue(module.proxy_tx_hash),
    },
    {
      label: t('fields.implementationTxHash'),
      value: presentValue(module.implementation_tx_hash),
    },
    {
      label: t('fields.wiringTxHash'),
      value: presentValue(module.wiring_tx_hash),
    },
  ]
}

export function BootstrapPage({ data, t }: BootstrapPageProps) {
  const sourcedaoState = parseSourceDaoState(data?.bootstrap.sourcedao_bootstrap_state.data)
  const sourcedaoStatus = sourcedaoState?.status ?? null
  const sourcedaoOperations = sourcedaoState?.operations ?? []
  const sourcedaoModules = Object.entries(sourcedaoState?.modules ?? {})
  const sourcedaoFinalWiring = Object.entries(sourcedaoState?.final_wiring ?? {})
  const sourcedaoWarnings = sourcedaoState?.warnings ?? []

  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
          {t('pages.bootstrap.title')}
        </h2>
        <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {t('pages.bootstrap.subtitle')}
        </p>
      </section>

      <section className="mt-1 grid gap-4 xl:grid-cols-4">
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
          status={
            data?.bootstrap.bootstrap_manifest.exists ? t('artifact.present') : t('artifact.missing')
          }
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
          title={t('artifacts.sourcedaoMarker')}
          summary={
            data?.bootstrap.sourcedao_bootstrap_marker ?? {
              path: '-',
              exists: false,
              error: null,
              data: null,
            }
          }
          status={
            data?.bootstrap.sourcedao_bootstrap_marker.exists
              ? t('artifact.present')
              : t('artifact.missing')
          }
          tone={artifactTone(Boolean(data?.bootstrap.sourcedao_bootstrap_marker.exists))}
        />
      </section>

      <BootstrapSteps
        overallLabel={data ? t(`states.${data.bootstrap.overall_state}`) : '-'}
        overallTone={overallBootstrapTone(data?.bootstrap.overall_state)}
        steps={data?.bootstrap.steps ?? []}
        t={t}
      />

      <section className="console-card">
        <div className="mb-4 flex items-start justify-between gap-3">
          <div>
            <h2 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('pages.bootstrap.sourcedaoTitle')}
            </h2>
            <p className="mt-2 max-w-4xl text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('pages.bootstrap.sourcedaoBody')}
            </p>
          </div>
          <span className="status-pill" data-tone={sourcedaoTone(sourcedaoStatus)}>
            {t(`states.${sourcedaoStatus ?? 'pending'}`)}
          </span>
        </div>

        {!sourcedaoState ? (
          <div className="console-subtle-card text-sm text-[color:var(--cp-muted)]">
            {t('pages.bootstrap.sourcedaoUnavailable')}
          </div>
        ) : (
          <div className="grid gap-5">
            <div className="grid gap-4 xl:grid-cols-2">
              <article className="console-subtle-card">
                <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">
                  {t('pages.bootstrap.sourcedaoSummaryTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('pages.bootstrap.sourcedaoSummaryBody')}
                </p>
                <div className="mt-4">
                  <FieldValueList
                    items={[
                      {
                        label: t('fields.status'),
                        value: presentValue(sourcedaoState.status),
                      },
                      {
                        label: t('fields.scope'),
                        value: presentValue(sourcedaoState.scope),
                      },
                      {
                        label: t('fields.chainId'),
                        value: presentValue(sourcedaoState.chain_id),
                      },
                      {
                        label: t('fields.completedAt'),
                        value: formatTimestamp(sourcedaoState.completed_at),
                      },
                      {
                        label: t('fields.bootstrapAdmin'),
                        value: presentValue(sourcedaoState.bootstrap_admin),
                      },
                      {
                        label: t('fields.daoAddress'),
                        value: presentValue(sourcedaoState.dao_address),
                      },
                      {
                        label: t('fields.dividendAddress'),
                        value: presentValue(sourcedaoState.dividend_address),
                      },
                      {
                        label: t('fields.configPath'),
                        value: presentValue(sourcedaoState.config_path),
                      },
                      {
                        label: t('fields.artifactsDir'),
                        value: presentValue(sourcedaoState.artifacts_dir),
                      },
                    ]}
                  />
                </div>
              </article>

              <article className="console-subtle-card">
                <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">
                  {t('pages.bootstrap.finalWiringTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('pages.bootstrap.finalWiringBody')}
                </p>
                <div className="mt-4">
                  <FieldValueList
                    items={sourcedaoFinalWiring.map(([key, value]) => ({
                      label: humanizeKey(key),
                      value: presentValue(value),
                    }))}
                  />
                </div>
              </article>
            </div>

            {sourcedaoWarnings.length > 0 ? (
              <article className="console-subtle-card">
                <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">
                  {t('pages.bootstrap.warningsTitle')}
                </h3>
                <ul className="mt-3 grid gap-2 text-sm leading-6 text-[color:var(--cp-warning)]">
                  {sourcedaoWarnings.map((warning) => (
                    <li key={warning} className="break-all">
                      {warning}
                    </li>
                  ))}
                </ul>
              </article>
            ) : null}

            <article className="console-subtle-card">
              <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('pages.bootstrap.operationsTitle')}
              </h3>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('pages.bootstrap.operationsBody')}
              </p>
              <div className="mt-4 overflow-x-auto">
                <table className="console-table">
                  <thead>
                    <tr>
                      <th>{t('fields.operation')}</th>
                      <th>{t('fields.status')}</th>
                      <th>{t('fields.txHash')}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sourcedaoOperations.length === 0 ? (
                      <tr>
                        <td className="py-3 text-sm text-[color:var(--cp-muted)]" colSpan={3}>
                          {t('pages.bootstrap.noOperations')}
                        </td>
                      </tr>
                    ) : (
                      sourcedaoOperations.map((operation) => (
                        <tr key={`${operation.name}:${operation.tx_hash ?? 'none'}`}>
                          <td className="break-all">{operation.name}</td>
                          <td>{presentValue(operation.status)}</td>
                          <td className="break-all">{presentValue(operation.tx_hash)}</td>
                        </tr>
                      ))
                    )}
                  </tbody>
                </table>
              </div>
            </article>

            <article className="console-subtle-card">
              <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">
                {t('pages.bootstrap.modulesTitle')}
              </h3>
              <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                {t('pages.bootstrap.modulesBody')}
              </p>
              {sourcedaoModules.length === 0 ? (
                <p className="mt-4 text-sm text-[color:var(--cp-muted)]">
                  {t('pages.bootstrap.noModules')}
                </p>
              ) : (
                <div className="mt-4 grid gap-4 xl:grid-cols-2">
                  {sourcedaoModules.map(([moduleName, module]) => (
                    <section key={moduleName} className="console-card">
                      <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                        {humanizeKey(moduleName)}
                      </h4>
                      <div className="mt-3">
                        <FieldValueList items={moduleFieldItems(module, t)} />
                      </div>
                    </section>
                  ))}
                </div>
              )}
            </article>
          </div>
        )}
      </section>
    </div>
  )
}
