import type { ReactNode } from 'react'

type Tone = 'neutral' | 'success' | 'warning' | 'danger'

interface ServiceCardProps {
  title: string
  status: string
  tone: Tone
  rpcUrl: string
  error?: string | null
  children: ReactNode
}

export function ServiceCard({
  title,
  status,
  tone,
  rpcUrl,
  error,
  children,
}: ServiceCardProps) {
  return (
    <article className="console-card">
      <div className="mb-4 flex items-start justify-between gap-3">
        <div>
          <h3 className="text-base font-semibold text-[color:var(--cp-text)]">{title}</h3>
          <p className="mt-2 text-sm text-[color:var(--cp-muted)] break-all">{rpcUrl}</p>
        </div>
        <span className="status-pill" data-tone={tone}>
          {status}
        </span>
      </div>

      <div className="grid grid-cols-2 gap-3">{children}</div>

      {error ? (
        <p className="mt-4 text-sm text-[color:var(--cp-danger)] break-all">{error}</p>
      ) : null}
    </article>
  )
}

