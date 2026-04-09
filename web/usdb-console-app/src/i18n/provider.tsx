/* eslint-disable react-refresh/only-export-components */
import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type PropsWithChildren,
} from 'react'
import { dictionaries } from './dictionaries'

export type SupportedLocale = 'en' | 'zh-CN'

interface I18nContextValue {
  locale: SupportedLocale
  setLocale: (locale: SupportedLocale) => void
  t: (key: string, fallback?: string, variables?: Record<string, string | number>) => string
}

const storageKey = 'usdb.console.react.locale.v1'
const I18nContext = createContext<I18nContextValue | null>(null)

function normalizeLocale(locale?: string | null): SupportedLocale {
  if (!locale) return 'en'
  if (locale === 'zh-CN' || locale.toLowerCase().startsWith('zh')) return 'zh-CN'
  return 'en'
}

function interpolate(
  message: string,
  variables?: Record<string, string | number>,
) {
  if (!variables) {
    return message
  }

  return Object.entries(variables).reduce((acc, [key, value]) => {
    return acc.split(`{{${key}}}`).join(String(value))
  }, message)
}

export function I18nProvider({ children }: PropsWithChildren) {
  const [locale, setLocale] = useState<SupportedLocale>(() => {
    const urlLocale = new URLSearchParams(window.location.search).get('lang')
    const stored = window.localStorage.getItem(storageKey)
    return normalizeLocale(urlLocale ?? stored ?? window.navigator.language)
  })

  useEffect(() => {
    window.localStorage.setItem(storageKey, locale)
    document.documentElement.lang = locale
    document.documentElement.dir = 'ltr'
  }, [locale])

  const value = useMemo<I18nContextValue>(() => {
    return {
      locale,
      setLocale,
      t: (key, fallback = key, variables) => {
        const current = dictionaries[locale][key] ?? dictionaries.en[key] ?? fallback
        return interpolate(current, variables)
      },
    }
  }, [locale])

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>
}

export function useI18n() {
  const context = useContext(I18nContext)

  if (!context) {
    throw new Error('useI18n must be used within I18nProvider')
  }

  return context
}
