interface PlaceholderPageProps {
  title: string
  subtitle: string
  body: string
}

export function PlaceholderPage({ title, subtitle, body }: PlaceholderPageProps) {
  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
          {title}
        </h2>
        <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {subtitle}
        </p>
      </section>

      <section className="console-card">
        <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
          {title}
        </h3>
        <p className="mt-3 max-w-3xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {body}
        </p>
      </section>
    </div>
  )
}
