import type { ReactNode } from 'react'
import type { Tone } from '../lib/console'

export interface ServiceSummaryItem {
  label: string
  value: string
  monospace?: boolean
}

interface ServiceSummaryCardProps {
  title: string
  status: string
  tone: Tone
  items?: ServiceSummaryItem[]
  summary?: string
  children?: ReactNode
}

export function ServiceSummaryCard({
  title,
  status,
  tone,
  items,
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
      {items && items.length > 0 ? (
        <dl className="space-y-2 text-sm leading-6">
          {items.map((item) => (
            <div
              key={`${title}-${item.label}`}
              className="flex flex-col gap-1 sm:flex-row sm:gap-2"
            >
              <dt className="shrink-0 font-medium text-[color:var(--cp-muted)]">
                {item.label}:
              </dt>
              <dd
                className={`min-w-0 break-all text-[color:var(--cp-text)] ${
                  item.monospace ? 'font-mono text-[13px]' : ''
                }`}
              >
                {item.value}
              </dd>
            </div>
          ))}
        </dl>
      ) : summary ? (
        <p className="text-sm leading-6 text-[color:var(--cp-muted)]">{summary}</p>
      ) : null}
      {children ? <div className="mt-4">{children}</div> : null}
    </article>
  )
}
