import { Globe, RefreshCcw } from 'lucide-react'
import type { ReactNode } from 'react'
import { NavLink } from 'react-router-dom'
import type { SupportedLocale } from '../i18n/provider'

interface ConsoleShellProps {
  locale: SupportedLocale
  setLocale: (locale: SupportedLocale) => void
  isLoading: boolean
  onRefresh: () => void
  t: (key: string) => string
  children: ReactNode
}

const navItems = [
  { to: '/overview', labelKey: 'nav.overview' },
  { to: '/apps', labelKey: 'nav.apps' },
  { to: '/services', labelKey: 'nav.services' },
  { to: '/bootstrap', labelKey: 'nav.bootstrap' },
  { to: '/protocol', labelKey: 'nav.protocol' },
  { to: '/me', labelKey: 'nav.me' },
]

export function ConsoleShell({
  locale,
  setLocale,
  isLoading,
  onRefresh,
  t,
  children,
}: ConsoleShellProps) {
  return (
    <>
      <div className="console-noise" />
      <main className="console-shell">
        <header className="console-masthead mb-5">
          <div className="min-w-0">
            <p className="shell-kicker m-0">{t('hero.kicker')}</p>
            <h1 className="mt-1 font-display text-[clamp(2rem,4vw,3.6rem)] font-semibold leading-[0.95] tracking-[-0.05em] text-[color:var(--cp-text)]">
              {t('hero.title')}
            </h1>
            <p className="mt-2 text-sm font-medium uppercase tracking-[0.18em] text-[color:var(--cp-muted)]">
              {t('hero.subtitle')}
            </p>
            <p className="mt-3 max-w-3xl text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('hero.hint')}
            </p>
          </div>

          <div className="console-toolbar">
            <label className="console-toolbar-control">
              <Globe className="h-4 w-4" />
              <span>{t('actions.language')}</span>
              <select
                className="console-toolbar-select"
                value={locale}
                onChange={(event) => setLocale(event.target.value as SupportedLocale)}
              >
                <option value="en">{t('locale.en')}</option>
                <option value="zh-CN">{t('locale.zh-CN')}</option>
              </select>
            </label>

            <button
              type="button"
              className="console-toolbar-button"
              onClick={onRefresh}
              disabled={isLoading}
            >
              <RefreshCcw className={`h-4 w-4 ${isLoading ? 'animate-spin' : ''}`} />
              {isLoading ? t('actions.reloading') : t('actions.refresh')}
            </button>
          </div>
        </header>

        <nav className="console-nav mb-6 grid grid-cols-2 md:grid-cols-6" aria-label={t('nav.primary')}>
          {navItems.map((item) => (
            <NavLink
              key={item.to}
              to={item.to}
              className={({ isActive }) =>
                isActive ? 'console-nav-link active' : 'console-nav-link'
              }
            >
              {t(item.labelKey)}
            </NavLink>
          ))}
        </nav>

        {children}
      </main>
    </>
  )
}
