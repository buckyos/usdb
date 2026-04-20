import { InlineHelpTooltip } from './InlineHelpTooltip'
import type { ArtifactSummary } from '../lib/types'

type Tone = 'neutral' | 'success' | 'warning' | 'danger'

interface ArtifactCardProps {
  title: string
  helpText?: string
  summary: ArtifactSummary
  status: string
  tone: Tone
  canOpenArtifact?: (path: string) => boolean
  onOpenArtifact?: (path: string) => void
}

export function ArtifactCard({
  title,
  helpText,
  summary,
  status,
  tone,
  canOpenArtifact,
  onOpenArtifact,
}: ArtifactCardProps) {
  const entries = Object.entries(summary.data ?? {}).slice(0, 8)
  const canOpenSelf = Boolean(summary.exists && summary.data && onOpenArtifact)

  function renderValue(value: unknown) {
    if (typeof value === 'string' && value.endsWith('.json') && canOpenArtifact?.(value) && onOpenArtifact) {
      return (
        <button
          className="console-link-button"
          onClick={() => onOpenArtifact(value)}
          type="button"
        >
          {value}
        </button>
      )
    }

    return typeof value === 'object' ? JSON.stringify(value) : String(value)
  }

  return (
    <article className="console-card">
      <div className="mb-4 flex items-start justify-between gap-3">
        <h3 className="inline-flex items-center gap-2 text-base font-semibold text-[color:var(--cp-text)]">
          <span>{title}</span>
          <InlineHelpTooltip text={helpText} />
        </h3>
        <span className="status-pill" data-tone={tone}>
          {status}
        </span>
      </div>
      <p className="text-sm text-[color:var(--cp-muted)] break-all">
        {canOpenSelf ? (
          <button
            className="console-link-button"
            onClick={() => onOpenArtifact?.(summary.path)}
            type="button"
          >
            {summary.path}
          </button>
        ) : (
          summary.path
        )}
      </p>
      <div className="mt-4 grid gap-3">
        {entries.map(([key, value]) => (
          <div
            key={key}
            className="border-t border-[color:var(--cp-border)] pt-3"
          >
            <span className="mb-1 block text-[11px] font-semibold uppercase tracking-[0.12em] text-[color:var(--cp-muted)]">
              {key}
            </span>
            <strong className="block break-all text-sm text-[color:var(--cp-text)]">
              {renderValue(value)}
            </strong>
          </div>
        ))}
      </div>
      {summary.error ? (
        <p className="mt-4 text-sm text-[color:var(--cp-danger)] break-all">{summary.error}</p>
      ) : null}
    </article>
  )
}
