import { Navigate, Route, Routes } from 'react-router-dom'
import useSWR from 'swr'
import { ConsoleShell } from './components/ConsoleShell'
import { fetchOverview } from './lib/api'
import { AppsPage } from './pages/AppsPage'
import { BootstrapPage } from './pages/BootstrapPage'
import { MePage } from './pages/MePage'
import { OverviewPage } from './pages/OverviewPage'
import { ProtocolPage } from './pages/ProtocolPage'
import { ServicesPage } from './pages/ServicesPage'
import { useI18n } from './i18n/provider'

export function App() {
  const { locale, setLocale, t } = useI18n()
  const { data, error, isLoading, mutate } = useSWR('/api/system/overview', fetchOverview, {
    refreshInterval: 8000,
    revalidateOnFocus: false,
  })

  return (
    <ConsoleShell
      locale={locale}
      setLocale={setLocale}
      isLoading={isLoading}
      onRefresh={() => void mutate()}
      t={(key) => t(key)}
    >
      {error ? (
        <section className="console-card mb-5 border-[color:var(--cp-danger)]">
          <p className="text-sm text-[color:var(--cp-danger)]">
            {t('errors.loadOverview')} {error.message}
          </p>
        </section>
      ) : null}

      <Routes>
        <Route path="/" element={<Navigate to="/overview" replace />} />
        <Route path="/overview" element={<OverviewPage data={data} locale={locale} t={t} />} />
        <Route path="/apps" element={<AppsPage data={data} t={t} />} />
        <Route path="/services" element={<Navigate to="/services/btc-node" replace />} />
        <Route
          path="/services/:serviceId"
          element={<ServicesPage data={data} locale={locale} t={t} />}
        />
        <Route path="/bootstrap" element={<BootstrapPage data={data} t={t} />} />
        <Route path="/protocol" element={<ProtocolPage data={data} locale={locale} t={t} />} />
        <Route path="/me" element={<Navigate to="/me/eth" replace />} />
        <Route path="/me/:identityKind" element={<MePage data={data} locale={locale} t={t} />} />
        <Route path="*" element={<Navigate to="/overview" replace />} />
      </Routes>
    </ConsoleShell>
  )
}
