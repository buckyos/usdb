interface MetricCardProps {
  label: string
  value: string
}

export function MetricCard({ label, value }: MetricCardProps) {
  return (
    <article className="console-card">
      <h2 className="text-sm font-semibold text-[color:var(--cp-muted)]">{label}</h2>
      <p className="mt-4 text-[clamp(1.5rem,3vw,2.35rem)] font-semibold tracking-[-0.04em] text-[color:var(--cp-text)]">
        {value}
      </p>
    </article>
  )
}

