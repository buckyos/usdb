export function shortText(value: unknown, head = 14, tail = 12) {
  const text = String(value ?? '')
  if (!text) return '-'
  if (text.length <= head + tail + 3) return text
  return `${text.slice(0, head)}...${text.slice(-tail)}`
}

