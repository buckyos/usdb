import { InlineHelpTooltip } from './InlineHelpTooltip'

interface MetricCardProps {
  label: string
  value: string
  helpText?: string
}

export function MetricCard({ label, value, helpText }: MetricCardProps) {
  return (
    <article className="console-card">
      <h2 className="flex items-center gap-2 text-sm font-semibold text-[color:var(--cp-muted)]">
        <span>{label}</span>
        <InlineHelpTooltip text={helpText} />
      </h2>
      <p className="mt-4 text-[clamp(1.5rem,3vw,2.35rem)] font-semibold tracking-[-0.04em] text-[color:var(--cp-text)]">
        {value}
      </p>
    </article>
  )
}
