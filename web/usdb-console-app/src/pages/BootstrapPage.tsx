import { ArtifactCard } from '../components/ArtifactCard'
import { BootstrapSteps } from '../components/BootstrapSteps'
import { artifactTone, type Tone } from '../lib/console'
import type { OverviewResponse } from '../lib/types'

interface BootstrapPageProps {
  data?: OverviewResponse
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

function overallBootstrapTone(state?: string): Tone {
  if (state === 'completed') return 'success'
  if (state === 'error') return 'danger'
  return 'warning'
}

export function BootstrapPage({ data, t }: BootstrapPageProps) {
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
          status={data?.bootstrap.bootstrap_manifest.exists ? t('artifact.present') : t('artifact.missing')}
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
          title={t('artifacts.sourcedaoState')}
          summary={
            data?.bootstrap.sourcedao_bootstrap_state ?? {
              path: '-',
              exists: false,
              error: null,
              data: null,
            }
          }
          status={data?.bootstrap.sourcedao_bootstrap_state.exists ? t('artifact.present') : t('artifact.missing')}
          tone={artifactTone(Boolean(data?.bootstrap.sourcedao_bootstrap_state.exists))}
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
          status={data?.bootstrap.sourcedao_bootstrap_marker.exists ? t('artifact.present') : t('artifact.missing')}
          tone={artifactTone(Boolean(data?.bootstrap.sourcedao_bootstrap_marker.exists))}
        />
      </section>

      <BootstrapSteps
        overallLabel={data ? t(`states.${data.bootstrap.overall_state}`) : '-'}
        overallTone={overallBootstrapTone(data?.bootstrap.overall_state)}
        steps={data?.bootstrap.steps ?? []}
        t={t}
      />
    </div>
  )
}
