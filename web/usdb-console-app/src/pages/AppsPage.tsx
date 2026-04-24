import { AppWindow, ExternalLink } from 'lucide-react'
import { FieldValueList } from '../components/FieldValueList'
import type { Tone } from '../lib/console'
import { displayList, displayText } from '../lib/format'
import type { AppEntry, OverviewResponse } from '../lib/types'

interface AppsPageProps {
  data?: OverviewResponse
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

function appTone(app: AppEntry): Tone {
  if (!app.available || app.status === 'offline') return 'danger'
  if (app.status === 'ready' || app.status === 'configured') return 'success'
  if (app.status === 'degraded' || app.status === 'pending' || app.status === 'starting') {
    return 'warning'
  }
  return 'neutral'
}

function appTitle(app: AppEntry, t: AppsPageProps['t']) {
  return t(`apps.${app.id}.title`, app.id)
}

function appBody(app: AppEntry, t: AppsPageProps['t']) {
  return t(`apps.${app.id}.body`, app.kind)
}

function appStatusLabel(app: AppEntry, t: AppsPageProps['t']) {
  return t(`apps.status.${app.status}`, app.status)
}

function appKindLabel(app: AppEntry, t: AppsPageProps['t']) {
  return t(`apps.kind.${app.kind}`, app.kind)
}

function appActionLabel(app: AppEntry, t: AppsPageProps['t']) {
  return app.target === 'external' ? t('actions.openExternalApp') : t('actions.openApp')
}

function renderAppCard(app: AppEntry, t: AppsPageProps['t']) {
  const targetProps =
    app.target === 'external' ? { target: '_blank', rel: 'noreferrer' } : { target: '_blank' }

  return (
    <article key={app.id} className="console-card flex min-h-full flex-col">
      <div className="mb-5 flex items-start justify-between gap-4">
        <div className="min-w-0">
          <p className="shell-kicker m-0">{appKindLabel(app, t)}</p>
          <h3 className="mt-2 text-xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
            {appTitle(app, t)}
          </h3>
          <p className="mt-3 text-sm leading-6 text-[color:var(--cp-muted)]">
            {appBody(app, t)}
          </p>
        </div>
        <span className="status-pill shrink-0" data-tone={appTone(app)}>
          {appStatusLabel(app, t)}
        </span>
      </div>

      <FieldValueList
        items={[
          {
            label: t('fields.appUrl'),
            value: displayText(app.url, t),
            helpText: t('help.fields.appUrl'),
          },
          {
            label: t('fields.runtimeProfile'),
            value: displayText(app.runtime_profile, t),
            helpText: t('help.fields.runtimeProfile'),
          },
          {
            label: t('fields.network'),
            value: displayText(app.network, t),
            helpText: t('help.fields.network'),
          },
          {
            label: t('fields.dependencies'),
            value: displayList(app.depends_on, t),
            helpText: t('help.fields.dependencies'),
          },
          {
            label: t('fields.statusMessage'),
            value: displayText(app.status_message, t),
            helpText: t('help.fields.statusMessage'),
          },
        ]}
      />

      <div className="mt-6 flex flex-wrap gap-3">
        <a className="console-action-button inline-flex items-center gap-2 no-underline" href={app.url} {...targetProps}>
          <ExternalLink className="h-4 w-4" />
          {appActionLabel(app, t)}
        </a>
        {app.service_id ? (
          <a
            className="console-secondary-button inline-flex items-center gap-2 no-underline"
            href={`#/services/${app.service_id}`}
          >
            <AppWindow className="h-4 w-4" />
            {t('actions.inspectService')}
          </a>
        ) : null}
      </div>
    </article>
  )
}

export function AppsPage({ data, t }: AppsPageProps) {
  const apps = data?.apps ?? []

  return (
    <div className="grid gap-5">
      <section className="console-page-intro">
        <h2 className="text-2xl font-semibold tracking-[-0.03em] text-[color:var(--cp-text)]">
          {t('pages.apps.title')}
        </h2>
        <p className="mt-3 max-w-4xl text-sm leading-7 text-[color:var(--cp-muted)]">
          {t('pages.apps.subtitle')}
        </p>
      </section>

      <section className="console-card">
        <div className="flex flex-wrap items-start justify-between gap-4">
          <div>
            <h3 className="text-base font-semibold text-[color:var(--cp-text)]">
              {t('pages.apps.runtimeTitle')}
            </h3>
            <p className="mt-2 max-w-3xl text-sm leading-6 text-[color:var(--cp-muted)]">
              {t('pages.apps.runtimeBody')}
            </p>
          </div>
          <span className="status-pill" data-tone={data ? 'success' : 'neutral'}>
            {data ? t('apps.status.configured') : t('common.notYetAvailable')}
          </span>
        </div>
      </section>

      {apps.length > 0 ? (
        <section className="grid gap-5 xl:grid-cols-3">
          {apps.map((app) => renderAppCard(app, t))}
        </section>
      ) : (
        <section className="console-card">
          <p className="text-sm text-[color:var(--cp-muted)]">
            {t('pages.apps.unavailable')}
          </p>
        </section>
      )}
    </div>
  )
}
