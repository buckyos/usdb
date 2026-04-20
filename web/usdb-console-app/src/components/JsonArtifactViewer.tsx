interface JsonArtifactViewerProps {
  title: string
  path: string
  data: Record<string, unknown> | null | undefined
  closeLabel: string
  onClose: () => void
}

export function JsonArtifactViewer({
  title,
  path,
  data,
  closeLabel,
  onClose,
}: JsonArtifactViewerProps) {
  return (
    <div className="console-modal-overlay" onClick={onClose} role="presentation">
      <section
        className="console-modal"
        aria-modal="true"
        role="dialog"
        aria-label={title}
        onClick={(event) => event.stopPropagation()}
      >
        <div className="console-modal__header">
          <div className="min-w-0">
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">{title}</h3>
            <p className="mt-2 break-all text-sm text-[color:var(--cp-muted)]">{path}</p>
          </div>
          <button className="console-secondary-button" onClick={onClose} type="button">
            {closeLabel}
          </button>
        </div>
        <pre className="console-modal__code">
          {JSON.stringify(data ?? {}, null, 2)}
        </pre>
      </section>
    </div>
  )
}
