import type { BootstrapStepSummary } from '../lib/types'

type Tone = 'neutral' | 'success' | 'warning' | 'danger'

interface BootstrapStepsProps {
  overallLabel: string
  overallTone: Tone
  steps: BootstrapStepSummary[]
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

export function BootstrapSteps({
  overallLabel,
  overallTone,
  steps,
  t,
}: BootstrapStepsProps) {
  return (
    <section className="console-card">
      <div className="mb-4 flex items-start justify-between gap-3">
        <h2 className="text-base font-semibold text-[color:var(--cp-text)]">
          {t('sections.bootstrap')}
        </h2>
        <span className="status-pill" data-tone={overallTone}>
          {overallLabel}
        </span>
      </div>

      <div className="grid gap-4 lg:grid-cols-2">
        {steps.map((step) => {
          const tone =
            step.state === 'completed'
              ? 'success'
              : step.state === 'skipped'
                ? 'neutral'
              : step.state === 'error'
                ? 'danger'
                : 'warning'

          const detailKey =
            step.state === 'completed'
              ? 'bootstrap.completed'
              : step.state === 'skipped'
                ? 'bootstrap.skipped'
              : step.state === 'error'
                ? 'bootstrap.error'
                : 'bootstrap.pending'

          return (
            <article key={step.step} className="console-subtle-card">
              <div className="mb-3 flex items-start justify-between gap-3">
                <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">
                  {t(`bootstrap.${step.step}`)}
                </h3>
                <span className="status-pill" data-tone={tone}>
                  {t(`states.${step.state}`)}
                </span>
              </div>
              <p className="text-sm leading-6 text-[color:var(--cp-muted)]">
                {t(detailKey, detailKey, {
                  label: t(`bootstrap.${step.step}`),
                  path: step.artifact_path ?? t('common.none'),
                  error: step.error ?? t('common.none'),
                })}
              </p>
            </article>
          )
        })}
      </div>
    </section>
  )
}
