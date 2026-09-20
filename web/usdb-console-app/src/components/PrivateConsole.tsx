import { useEffect, useState, type FormEvent, type PropsWithChildren } from 'react'
import useSWR, { SWRConfig } from 'swr'
import { useI18n } from '../i18n/provider'

async function session() {
  const response = await fetch('/api/auth/session', { cache: 'no-store' })
  if (!response.ok) throw new Error(`HTTP ${response.status}`)
  return response.json() as Promise<{ authenticated: boolean }>
}

/** Keep the token out of URLs and browser storage; the server owns the session. */
export function PrivateConsole({ children }: PropsWithChildren) {
  const { locale } = useI18n()
  const zh = locale === 'zh-CN'
  const { data, error, mutate } = useSWR('/api/auth/session', session, { refreshInterval: 30000 })
  const [token, setToken] = useState('')
  const [message, setMessage] = useState('')
  const [busy, setBusy] = useState(false)
  useEffect(() => {
    const refresh = () => { void mutate() }
    window.addEventListener('usdb-session-expired', refresh)
    return () => window.removeEventListener('usdb-session-expired', refresh)
  }, [mutate])

  async function login(event: FormEvent) {
    event.preventDefault()
    setBusy(true)
    setMessage('')
    try {
      const response = await fetch('/api/auth/login', { method: 'POST', headers: { 'Content-Type': 'application/json' }, body: JSON.stringify({ token }) })
      if (!response.ok) throw new Error(zh ? '登录失败，请检查令牌和访问地址。' : 'Sign-in failed. Check the token and console address.')
      await mutate()
    } catch (failure) { setMessage(String(failure)) }
    finally { setToken(''); setBusy(false) }
  }

  async function logout() {
    try {
      const response = await fetch('/api/auth/logout', { method: 'POST' })
      if (!response.ok) throw new Error('Sign-out failed')
      await mutate({ authenticated: false }, { revalidate: false })
    } catch {
      setMessage(zh ? '退出失败，请重试。' : 'Sign-out failed. Please retry.')
    }
  }

  if (data?.authenticated && !error) return (
    <>
      <div className="flex justify-end gap-4 p-3 text-sm">
        <span>{zh ? '私有控制台 · 会话最长 12 小时' : 'Private console · 12-hour session'}</span>
        <button className="underline" onClick={() => void logout()}>{zh ? '退出登录' : 'Sign out'}</button>
        {message && <span role="alert">{message}</span>}
      </div>
      {/* Discard authenticated query data when the session ends. */}
      <SWRConfig value={{ provider: () => new Map() }}>{children}</SWRConfig>
    </>
  )
  return (
    <main className="mx-auto mt-20 max-w-lg p-6">
      <form className="console-card grid gap-5" onSubmit={event => void login(event)}>
        <h1 className="text-2xl font-semibold">{zh ? 'USDB 私有控制台' : 'USDB private console'}</h1>
        <p>{zh ? '通过 SSH 隧道访问，在节点上运行以下命令获取登录令牌：' : 'Connect through an SSH tunnel. Retrieve your login token on the node:'}</p>
        <code>usdb-node console token</code>
        <label className="grid gap-2">{zh ? '访问令牌' : 'Access token'}
          <input className="rounded border p-3 text-black" type="password" autoComplete="off" value={token} onChange={event => setToken(event.target.value)} required />
        </label>
        <button className="rounded bg-blue-700 p-3 text-white disabled:opacity-50" disabled={busy || !data}>{zh ? '登录' : 'Sign in'}</button>
        <p role="alert">{error ? (zh ? '无法连接控制台，请检查 SSH 隧道和控制台服务。' : 'Cannot reach the console. Check the SSH tunnel and console service.') : message}</p>
      </form>
    </main>
  )
}
