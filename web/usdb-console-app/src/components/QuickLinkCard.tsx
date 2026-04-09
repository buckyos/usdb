import type { ReactNode } from 'react'
import { Link } from 'react-router-dom'

interface QuickLinkCardProps {
  title: string
  body: string
  to?: string
  href?: string
  children?: ReactNode
}

export function QuickLinkCard({ title, body, to, href, children }: QuickLinkCardProps) {
  const className = 'console-subtle-card block no-underline'

  const content = (
    <>
      <strong className="block text-base font-semibold text-[color:var(--cp-text)]">
        {title}
      </strong>
      <span className="mt-2 block text-sm leading-6 text-[color:var(--cp-muted)]">
        {body}
      </span>
      {children ? <div className="mt-4">{children}</div> : null}
    </>
  )

  if (to) {
    return (
      <Link to={to} className={className}>
        {content}
      </Link>
    )
  }

  return (
    <a href={href} className={className}>
      {content}
    </a>
  )
}
