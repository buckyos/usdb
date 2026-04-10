import { Info } from 'lucide-react'

interface InlineHelpTooltipProps {
  text?: string | null
}

export function InlineHelpTooltip({ text }: InlineHelpTooltipProps) {
  if (!text) return null

  return (
    <span className="console-help-tooltip">
      <span
        className="console-help-tooltip__trigger"
        aria-label={text}
        tabIndex={0}
      >
        <Info size={14} strokeWidth={2} />
      </span>
      <span className="console-help-tooltip__content" role="tooltip">
        {text}
      </span>
    </span>
  )
}
