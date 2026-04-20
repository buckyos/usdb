import { useState } from 'react'
import { ArtifactCard } from '../components/ArtifactCard'
import { BootstrapSteps } from '../components/BootstrapSteps'
import { FieldValueList } from '../components/FieldValueList'
import { InlineHelpTooltip } from '../components/InlineHelpTooltip'
import { JsonArtifactViewer } from '../components/JsonArtifactViewer'
import { artifactTone, type Tone } from '../lib/console'
import type {
  ArtifactSummary,
  OverviewResponse,
  SourceDaoBootstrapModule,
  SourceDaoBootstrapState,
} from '../lib/types'

interface BootstrapPageProps {
  data?: OverviewResponse
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

const SOURCE_DAO_MODULE_ORDER = [
  'committee',
  'dev_token',
  'normal_token',
  'token_lockup',
  'project',
  'acquired',
] as const

function overallBootstrapTone(state?: string): Tone {
  if (state === 'completed') return 'success'
  if (state === 'error') return 'danger'
  if (state === 'skipped') return 'neutral'
  return 'warning'
}

function sourcedaoTone(state?: string | null): Tone {
  if (state === 'completed') return 'success'
  if (state === 'error') return 'danger'
  if (state === 'running') return 'warning'
  return 'warning'
}

function presentValue(value: unknown) {
  if (value === null || value === undefined || value === '') return '-'
  return String(value)
}

function translateStateValue(
  value: string | null | undefined,
  t: BootstrapPageProps['t'],
  fallback = '-',
) {
  if (!value) return fallback
  return t(`states.${value}`, value)
}

function formatTimestamp(value?: string | null) {
  if (!value) return '-'
  const parsed = new Date(value)
  if (Number.isNaN(parsed.getTime())) return value
  return parsed.toLocaleString(undefined, { hour12: false })
}

function normalizeStatusToken(value?: string | null) {
  return (value ?? '').replace(/[^a-z0-9]/gi, '').toLowerCase()
}

function moduleStateLabel(
  moduleName: string,
  module: SourceDaoBootstrapModule | null,
  finalWiringValue: string | null | undefined,
  currentStep: string | null | undefined,
  t: BootstrapPageProps['t'],
) {
  if (module) return t('states.completed')
  if (normalizeStatusToken(currentStep) === normalizeStatusToken(moduleName)) {
    return t('states.running')
  }
  if (finalWiringValue) return t('states.completed')
  return t('states.pending')
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
  module: SourceDaoBootstrapModule | null,
  stateLabel: string,
  t: BootstrapPageProps['t'],
) {
  return [
    {
      label: t('fields.moduleState'),
      value: stateLabel,
      helpText: t('help.fields.moduleState'),
    },
    {
      label: t('fields.address'),
      value: presentValue(module?.address),
      helpText: t('help.fields.address'),
    },
    {
      label: t('fields.source'),
      value: presentValue(module?.source),
      helpText: t('help.fields.source'),
    },
    {
      label: t('fields.implementationAddress'),
      value: presentValue(module?.implementation_address),
      helpText: t('help.fields.implementationAddress'),
    },
    {
      label: t('fields.proxyTxHash'),
      value: presentValue(module?.proxy_tx_hash),
      helpText: t('help.fields.proxyTxHash'),
    },
    {
      label: t('fields.implementationTxHash'),
      value: presentValue(module?.implementation_tx_hash),
      helpText: t('help.fields.implementationTxHash'),
    },
    {
      label: t('fields.wiringTxHash'),
      value: presentValue(module?.wiring_tx_hash),
      helpText: t('help.fields.wiringTxHash'),
    },
  ]
}

function artifactFallback(): ArtifactSummary {
  return {
    path: '-',
    exists: false,
    error: null,
    data: null,
  }
}

function renderHeaderWithHelp(
  label: string,
  helpText?: string,
) {
  return (
    <span className="inline-flex items-center gap-2">
      <span>{label}</span>
      <InlineHelpTooltip text={helpText} />
    </span>
  )
}

export function BootstrapPage({ data, t }: BootstrapPageProps) {
  const [selectedArtifact, setSelectedArtifact] = useState<{
    title: string
    summary: ArtifactSummary
  } | null>(null)
  const sourcedaoState = parseSourceDaoState(data?.bootstrap.sourcedao_bootstrap_state.data)
  const sourcedaoStatus = sourcedaoState?.status ?? null
  const sourcedaoOperations = sourcedaoState?.operations ?? []
  const sourcedaoModulesByKey = sourcedaoState?.modules ?? {}
  const sourcedaoFinalWiring = Object.entries(sourcedaoState?.final_wiring ?? {})
  const sourcedaoWarnings = sourcedaoState?.warnings ?? []
  const sourcedaoCurrentStep = sourcedaoState?.current_step ?? null
  const sourcedaoLastError = sourcedaoState?.last_error ?? null
  const sourcedaoRuntimeMessage = sourcedaoState?.message ?? null
  const bootstrapArtifacts = [
    {
      title: t('artifacts.bootstrapManifest'),
      helpText: t('help.artifacts.bootstrapManifest'),
      summary: data?.bootstrap.bootstrap_manifest ?? artifactFallback(),
    },
    {
      title: t('artifacts.snapshotMarker'),
      helpText: t('help.artifacts.snapshotMarker'),
      summary: data?.bootstrap.snapshot_marker ?? artifactFallback(),
    },
    {
      title: t('artifacts.ethwInitMarker'),
      helpText: t('help.artifacts.ethwInitMarker'),
      summary: data?.bootstrap.ethw_init_marker ?? artifactFallback(),
    },
    {
      title: t('artifacts.ethwGenesis'),
      helpText: t('help.artifacts.ethwGenesis'),
      summary: data?.bootstrap.ethw_genesis ?? artifactFallback(),
    },
    {
      title: t('artifacts.sourcedaoState'),
      helpText: t('help.artifacts.sourcedaoState'),
      summary: data?.bootstrap.sourcedao_bootstrap_state ?? artifactFallback(),
    },
    {
      title: t('artifacts.sourcedaoMarker'),
      helpText: t('help.artifacts.sourcedaoMarker'),
      summary: data?.bootstrap.sourcedao_bootstrap_marker ?? artifactFallback(),
    },
  ]
  const bootstrapArtifactsByPath = new Map(
    bootstrapArtifacts.map((artifact) => [artifact.summary.path, artifact] as const),
  )
  const sourcedaoModuleNames = Array.from(
    new Set([
      ...SOURCE_DAO_MODULE_ORDER,
      ...Object.keys(sourcedaoModulesByKey),
      ...Object.keys(sourcedaoState?.final_wiring ?? {}),
    ]),
  )
  const sourcedaoModuleStatusByKey = Object.fromEntries(
    sourcedaoModuleNames.map((moduleName) => [
      moduleName,
      moduleStateLabel(
        moduleName,
        sourcedaoModulesByKey[moduleName] ?? null,
        sourcedaoState?.final_wiring?.[moduleName] ?? null,
        sourcedaoCurrentStep,
        t,
      ),
    ]),
  )

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

      <section className="mt-1 grid gap-4 xl:grid-cols-3">
        {bootstrapArtifacts.map((artifact) => (
          <ArtifactCard
            key={artifact.summary.path}
            title={artifact.title}
            helpText={artifact.helpText}
            summary={artifact.summary}
            status={artifact.summary.exists ? t('artifact.present') : t('artifact.missing')}
            tone={artifactTone(Boolean(artifact.summary.exists))}
            canOpenArtifact={(path) => bootstrapArtifactsByPath.has(path)}
            onOpenArtifact={(path) => {
              const linkedArtifact = bootstrapArtifactsByPath.get(path)
              if (linkedArtifact) {
                setSelectedArtifact({
                  title: linkedArtifact.title,
                  summary: linkedArtifact.summary,
                })
              }
            }}
          />
        ))}
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
                        value: translateStateValue(sourcedaoState.status, t),
                        helpText: t('help.fields.status'),
                      },
                      {
                        label: t('fields.currentStep'),
                        value: presentValue(sourcedaoCurrentStep),
                        helpText: t('help.fields.currentStep'),
                      },
                      {
                        label: t('fields.scope'),
                        value: presentValue(sourcedaoState.scope),
                        helpText: t('help.fields.scope'),
                      },
                      {
                        label: t('fields.runtimeMessage'),
                        value: presentValue(sourcedaoRuntimeMessage),
                        helpText: t('help.fields.runtimeMessage'),
                      },
                      {
                        label: t('fields.chainId'),
                        value: presentValue(sourcedaoState.chain_id),
                        helpText: t('help.fields.chainId'),
                      },
                      {
                        label: t('fields.generatedAt'),
                        value: formatTimestamp(sourcedaoState.generated_at),
                        helpText: t('help.fields.generatedAt'),
                      },
                      {
                        label: t('fields.completedAt'),
                        value: formatTimestamp(sourcedaoState.completed_at),
                        helpText: t('help.fields.completedAt'),
                      },
                      {
                        label: t('fields.bootstrapAdmin'),
                        value: presentValue(sourcedaoState.bootstrap_admin),
                        helpText: t('help.fields.bootstrapAdmin'),
                      },
                      {
                        label: t('fields.daoAddress'),
                        value: presentValue(sourcedaoState.dao_address),
                        helpText: t('help.fields.daoAddress'),
                      },
                      {
                        label: t('fields.dividendAddress'),
                        value: presentValue(sourcedaoState.dividend_address),
                        helpText: t('help.fields.dividendAddress'),
                      },
                      {
                        label: t('fields.configPath'),
                        value: presentValue(sourcedaoState.config_path),
                        helpText: t('help.fields.configPath'),
                      },
                      {
                        label: t('fields.artifactsDir'),
                        value: presentValue(sourcedaoState.artifacts_dir),
                        helpText: t('help.fields.artifactsDir'),
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
                      helpText: t('help.fields.finalWiringAddress'),
                    }))}
                  />
                </div>
              </article>
            </div>

            {sourcedaoLastError ? (
              <article className="console-subtle-card">
                <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">
                  {t('pages.bootstrap.lastErrorTitle')}
                </h3>
                <p className="mt-2 text-sm leading-6 text-[color:var(--cp-muted)]">
                  {t('pages.bootstrap.lastErrorBody')}
                </p>
                <pre className="mt-4 overflow-x-auto whitespace-pre-wrap break-words rounded-[20px] border border-[color:var(--cp-danger-border)] bg-[color:var(--cp-danger-surface)]/65 px-4 py-3 text-xs leading-6 text-[color:var(--cp-danger)]">
                  {sourcedaoLastError}
                </pre>
              </article>
            ) : null}

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
                      <th>{renderHeaderWithHelp(t('fields.operation'), t('help.fields.operation'))}</th>
                      <th>{renderHeaderWithHelp(t('fields.status'), t('help.fields.status'))}</th>
                      <th>{renderHeaderWithHelp(t('fields.txHash'), t('help.fields.txHash'))}</th>
                      <th>{renderHeaderWithHelp(t('fields.details'), t('help.fields.details'))}</th>
                    </tr>
                  </thead>
                  <tbody>
                    {sourcedaoOperations.length === 0 ? (
                      <tr>
                        <td className="py-3 text-sm text-[color:var(--cp-muted)]" colSpan={4}>
                          {t('pages.bootstrap.noOperations')}
                        </td>
                      </tr>
                    ) : (
                      sourcedaoOperations.map((operation) => (
                        <tr key={`${operation.name}:${operation.tx_hash ?? 'none'}`}>
                          <td className="break-all">{operation.name}</td>
                          <td>{translateStateValue(operation.status, t, presentValue(operation.status))}</td>
                          <td className="break-all">{presentValue(operation.tx_hash)}</td>
                          <td className="break-all">{presentValue(operation.details)}</td>
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
              {sourcedaoModuleNames.length === 0 ? (
                <p className="mt-4 text-sm text-[color:var(--cp-muted)]">
                  {t('pages.bootstrap.noModules')}
                </p>
              ) : (
                <div className="mt-4 grid gap-4 xl:grid-cols-2">
                  {sourcedaoModuleNames.map((moduleName) => {
                    const module = sourcedaoModulesByKey[moduleName] ?? null
                    return (
                    <section key={moduleName} className="console-card">
                      <div className="flex items-start justify-between gap-3">
                        <h4 className="text-sm font-semibold text-[color:var(--cp-text)]">
                          {humanizeKey(moduleName)}
                        </h4>
                        <span
                          className="status-pill"
                          data-tone={sourcedaoTone(
                            module
                              ? 'completed'
                              : normalizeStatusToken(sourcedaoCurrentStep) ===
                                  normalizeStatusToken(moduleName)
                                ? 'running'
                                : 'pending',
                          )}
                        >
                          {sourcedaoModuleStatusByKey[moduleName]}
                        </span>
                      </div>
                      <div className="mt-3">
                        <FieldValueList
                          items={moduleFieldItems(
                            module,
                            sourcedaoModuleStatusByKey[moduleName],
                            t,
                          )}
                        />
                      </div>
                    </section>
                    )
                  })}
                </div>
              )}
            </article>
          </div>
        )}
      </section>
      {selectedArtifact?.summary.data ? (
        <JsonArtifactViewer
          title={selectedArtifact.title}
          path={selectedArtifact.summary.path}
          data={selectedArtifact.summary.data}
          closeLabel={t('actions.closeViewer')}
          onClose={() => setSelectedArtifact(null)}
        />
      ) : null}
    </div>
  )
}
