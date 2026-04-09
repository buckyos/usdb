export function shortText(value: unknown, head = 14, tail = 12) {
  const text = String(value ?? '')
  if (!text) return '-'
  if (text.length <= head + tail + 3) return text
  return `${text.slice(0, head)}...${text.slice(-tail)}`
}

type Translate = (key: string, fallback?: string, variables?: Record<string, string | number>) => string

export function displayText(
  value: unknown,
  t: Translate,
  emptyKey = 'common.notYetAvailable',
) {
  if (value == null) return t(emptyKey)
  const text = String(value).trim()
  return text ? text : t(emptyKey)
}

export function displayNumber(
  locale: string,
  value: number | null | undefined,
  t: Translate,
  emptyKey = 'common.notYetAvailable',
) {
  if (value === null || value === undefined || Number.isNaN(Number(value))) {
    return t(emptyKey)
  }
  return new Intl.NumberFormat(locale).format(Number(value))
}

export function displayBoolean(
  value: boolean | null | undefined,
  t: Translate,
  emptyKey = 'common.notYetAvailable',
) {
  if (value == null) return t(emptyKey)
  return value ? t('common.true') : t('common.false')
}

export function displayList(
  values: string[] | null | undefined,
  t: Translate,
  options?: {
    emptyKey?: string
    missingKey?: string
  },
) {
  if (values == null) return t(options?.missingKey ?? 'common.notYetAvailable')
  if (values.length === 0) return t(options?.emptyKey ?? 'common.none')
  return values.join(', ')
}

export function displayShortText(
  value: unknown,
  t: Translate,
  options?: {
    head?: number
    tail?: number
    emptyKey?: string
  },
) {
  if (value == null || String(value).trim() === '') {
    return t(options?.emptyKey ?? 'common.notYetAvailable')
  }
  return shortText(value, options?.head, options?.tail)
}
