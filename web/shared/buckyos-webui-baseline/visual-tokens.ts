export const panelToneClasses = {
  accent:
    'bg-[color:color-mix(in_srgb,var(--cp-accent-soft)_16%,var(--cp-surface))] text-[color:var(--cp-accent)]',
  success:
    'bg-[color:color-mix(in_srgb,var(--cp-success)_14%,var(--cp-surface))] text-[color:var(--cp-success)]',
  warning:
    'bg-[color:color-mix(in_srgb,var(--cp-warning)_14%,var(--cp-surface))] text-[color:var(--cp-warning)]',
  neutral:
    'bg-[color:color-mix(in_srgb,var(--cp-surface-2)_88%,transparent)] text-[color:var(--cp-muted)]',
} as const

export function statusTone(status: string) {
  switch (status) {
    case 'completed':
    case 'consensus_ready':
      return 'success'
    case 'error':
    case 'offline':
      return 'warning'
    default:
      return 'neutral'
  }
}

