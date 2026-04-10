import { InlineHelpTooltip } from './InlineHelpTooltip'

interface FieldValueItem {
  label: string
  value: string
  helpText?: string
}

interface FieldValueListProps {
  items: FieldValueItem[]
}

export function FieldValueList({ items }: FieldValueListProps) {
  return (
    <div className="grid gap-3">
      {items.map((item) => (
        <div
          key={`${item.label}:${item.value}`}
          className="border-t border-[color:var(--cp-border)] pt-3 sm:flex sm:gap-2"
        >
          <span className="shrink-0 inline-flex items-center gap-2 text-sm font-medium text-[color:var(--cp-muted)]">
            <span>{item.label}:</span>
            <InlineHelpTooltip text={item.helpText} />
          </span>
          <strong className="block break-all text-sm text-[color:var(--cp-text)]">
            {item.value}
          </strong>
        </div>
      ))}
    </div>
  )
}
