import type { ReactNode } from 'react'
import type { Tone } from '../lib/console'

interface ServiceSummaryCardProps {
  title: string
  status: string
  tone: Tone
  summary: string
  children?: ReactNode
}

export function ServiceSummaryCard({
  title,
  status,
  tone,
  summary,
  children,
}: ServiceSummaryCardProps) {
  return (
    <article className="console-subtle-card">
      <div className="mb-3 flex items-start justify-between gap-3">
        <h3 className="text-sm font-semibold text-[color:var(--cp-text)]">{title}</h3>
        <span className="status-pill" data-tone={tone}>
          {status}
        </span>
      </div>
      <p className="text-sm leading-6 text-[color:var(--cp-muted)]">{summary}</p>
      {children ? <div className="mt-4">{children}</div> : null}
    </article>
  )
}
