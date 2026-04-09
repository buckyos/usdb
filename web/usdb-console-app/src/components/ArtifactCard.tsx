import type { ArtifactSummary } from '../lib/types'

type Tone = 'neutral' | 'success' | 'warning' | 'danger'

interface ArtifactCardProps {
  title: string
  summary: ArtifactSummary
  status: string
  tone: Tone
}

export function ArtifactCard({ title, summary, status, tone }: ArtifactCardProps) {
  const entries = Object.entries(summary.data ?? {}).slice(0, 8)

  return (
    <article className="console-card">
      <div className="mb-4 flex items-start justify-between gap-3">
        <h3 className="text-base font-semibold text-[color:var(--cp-text)]">{title}</h3>
        <span className="status-pill" data-tone={tone}>
          {status}
        </span>
      </div>
      <p className="text-sm text-[color:var(--cp-muted)] break-all">{summary.path}</p>
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
              {typeof value === 'object' ? JSON.stringify(value) : String(value)}
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

