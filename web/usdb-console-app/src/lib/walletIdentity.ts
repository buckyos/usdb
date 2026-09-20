import { Address, NETWORK, TEST_NETWORK } from '@scure/btc-signer'

export type IdentityKind = 'usdb' | 'btc'
export type BitcoinNetwork = 'mainnet' | 'testnet' | 'testnet4' | 'signet' | 'regtest'
export interface WalletSnapshot {
  address: string
  network: string | null
  genesis: string | null
}
interface Events {
  on?: (event: string, listener: () => void) => void
  removeListener?: (event: string, listener: () => void) => void
}
interface EvmProvider extends Events {
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>
}
interface BitcoinProvider extends Events {
  requestAccounts?: () => Promise<string[]>
  connect?: () => Promise<{ address?: string }>
  getAccounts?: () => Promise<string[]>
  getNetwork?: () => Promise<string>
  getChain?: () => Promise<{ enum?: string }>
}
export interface WalletAdapter {
  id: string
  name: string
  kind: IdentityKind
  read: (connect: boolean) => Promise<WalletSnapshot>
  subscribe: (changed: () => void, disconnected: () => void) => () => void
  switchChain?: (chainId: string) => Promise<void>
}

/** Canonical quantities avoid decimal/hex mismatches and unsafe integer coercion. */
export function chainId(value: unknown): string | null {
  if (typeof value === 'number' && (!Number.isSafeInteger(value) || value < 1)) return null
  if (typeof value !== 'string' && typeof value !== 'number') return null
  if (!/^(0x[0-9a-f]+|[0-9]+)$/i.test(String(value))) return null
  const number = BigInt(value)
  return number > 0n ? `0x${number.toString(16)}` : null
}

export function evmAddress(value: unknown): string | null {
  return typeof value === 'string' && /^0x[0-9a-f]{40}$/i.test(value.trim()) ? value.trim() : null
}

export function genesisHash(value: unknown): string | null {
  return typeof value === 'string' && /^0x[0-9a-f]{64}$/i.test(value) ? value.toLowerCase() : null
}

export function bitcoinNetwork(value: unknown): BitcoinNetwork | null {
  const names: Record<string, BitcoinNetwork> = {
    'btc-mainnet': 'mainnet', 'btc-regtest': 'regtest',
    main: 'mainnet', mainnet: 'mainnet', bitcoin: 'mainnet', livenet: 'mainnet',
    test: 'testnet', testnet: 'testnet', testnet3: 'testnet', testnet4: 'testnet4', signet: 'signet', regtest: 'regtest',
    BITCOIN_MAINNET: 'mainnet', BITCOIN_TESTNET: 'testnet', BITCOIN_TESTNET4: 'testnet4', BITCOIN_SIGNET: 'signet',
  }
  return typeof value === 'string' && Object.prototype.hasOwnProperty.call(names, value) ? names[value] : null
}

/** Decode checksums; testnet/signet share address formats, so provider network is checked separately. */
export function bitcoinAddress(value: string, network: BitcoinNetwork | null): string | null {
  if (!network) return null
  const address = value.trim()
  const parameters = network === 'mainnet' ? NETWORK : network === 'regtest' ? { ...TEST_NETWORK, bech32: 'bcrt' } : TEST_NETWORK
  try { Address(parameters).decode(address); return address } catch { return null }
}

/** Render native units exactly, including values above Number.MAX_SAFE_INTEGER. */
export function nativeBalance(hex: unknown): string | null {
  if (typeof hex !== 'string' || !/^0x[0-9a-f]+$/i.test(hex)) return null
  const value = BigInt(hex)
  const fraction = (value % 10n ** 18n).toString().padStart(18, '0').replace(/0+$/, '')
  return `${value / 10n ** 18n}${fraction ? `.${fraction}` : ''}`
}

/** Bound reads and prompts without pretending to cancel an extension-owned request. */
async function bounded<T>(promise: Promise<T>, timeout = 15000): Promise<T> {
  let timer: ReturnType<typeof setTimeout> | undefined
  try {
    return await Promise.race([promise, new Promise<never>((_, reject) => {
      timer = setTimeout(() => reject(new Error('WALLET_TIMEOUT')), timeout)
    })])
  } finally { clearTimeout(timer) }
}

function listen(provider: Events, events: string[], changed: () => void, disconnected: () => void) {
  if (!provider.on || !provider.removeListener) throw new Error('WALLET_UNSUPPORTED')
  for (const event of events) provider.on(event, changed)
  provider.on('disconnect', disconnected)
  return () => {
    for (const event of events) provider.removeListener?.(event, changed)
    provider.removeListener?.('disconnect', disconnected)
  }
}

function evmAdapter(provider: EvmProvider, id: string, name: string): WalletAdapter {
  const request = (method: string, params?: unknown[]) => bounded(provider.request({ method, params }))
  return {
    id, name, kind: 'usdb',
    async read(connect) {
      if (connect) await bounded(provider.request({ method: 'eth_requestAccounts' }), 60000)
      const accounts = await request('eth_accounts')
      const address = evmAddress(Array.isArray(accounts) ? accounts[0] : null)
      if (!address) throw new Error('WALLET_NO_ACCOUNT')
      const network = chainId(await request('eth_chainId'))
      const block = await request('eth_getBlockByNumber', ['0x0', false]).catch(() => null) as { hash?: unknown } | null
      const after = await request('eth_accounts')
      if (!Array.isArray(after) || after[0]?.toLowerCase() !== address.toLowerCase() || chainId(await request('eth_chainId')) !== network) {
        throw new Error('WALLET_CHANGED')
      }
      return { address, network, genesis: genesisHash(block?.hash) }
    },
    subscribe: (changed, disconnected) => listen(provider, ['accountsChanged', 'chainChanged'], changed, disconnected),
    async switchChain(target) {
      await bounded(provider.request({ method: 'wallet_switchEthereumChain', params: [{ chainId: target }] }), 60000)
    },
  }
}

function btcAdapter(provider: BitcoinProvider, id: string, name: string): WalletAdapter {
  async function network() {
    // A recognized getChain enum takes precedence; Fractal must not become Bitcoin via livenet.
    if (provider.getChain) return bitcoinNetwork((await bounded(provider.getChain())).enum)
    return provider.getNetwork ? bitcoinNetwork(await bounded(provider.getNetwork())) : null
  }
  return {
    id, name, kind: 'btc',
    async read(connect) {
      if (!provider.getAccounts) throw new Error('WALLET_UNSUPPORTED')
      if (connect) {
        if (provider.requestAccounts) await bounded(provider.requestAccounts(), 60000)
        else if (provider.connect) await bounded(provider.connect(), 60000)
        else throw new Error('WALLET_UNSUPPORTED')
      }
      const accounts = await bounded(provider.getAccounts())
      if (!Array.isArray(accounts) || typeof accounts[0] !== 'string' || !accounts[0]) throw new Error('WALLET_NO_ACCOUNT')
      const selected = await network()
      const after = await bounded(provider.getAccounts())
      if (after[0] !== accounts[0] || await network() !== selected) throw new Error('WALLET_CHANGED')
      return { address: accounts[0], network: selected, genesis: null }
    },
    subscribe: (changed, disconnected) => listen(provider, ['accountsChanged', 'accountChanged', 'networkChanged', 'chainChanged'], changed, disconnected),
  }
}

/** Discover only providers; never request accounts until the operator chooses Connect. */
export function discoverWallets(kind: IdentityKind, update: (adapters: WalletAdapter[]) => void): () => void {
  const host = window as unknown as { ethereum?: EvmProvider; unisat?: BitcoinProvider; okxwallet?: { bitcoin?: BitcoinProvider } }
  const adapters: WalletAdapter[] = []
  const providers = new Set<unknown>()
  function add(provider: EvmProvider | BitcoinProvider | undefined, id: string, name: string) {
    if (!provider || providers.has(provider) || adapters.some(adapter => adapter.id === id)) return
    if (kind === 'usdb' && typeof (provider as EvmProvider).request !== 'function') return
    providers.add(provider)
    adapters.push(kind === 'usdb' ? evmAdapter(provider as EvmProvider, id, name) : btcAdapter(provider as BitcoinProvider, id, name))
    update([...adapters])
  }
  const announced = (event: Event) => {
    const detail = (event as CustomEvent).detail
    if (detail && typeof detail.info?.uuid === 'string' && typeof detail.info?.name === 'string') {
      add(detail.provider, detail.info.uuid.slice(0, 80), detail.info.name.slice(0, 80))
    }
  }
  const scan = () => {
    if (kind === 'usdb') {
      window.dispatchEvent(new Event('eip6963:requestProvider'))
      add(host.ethereum, 'injected-ethereum', 'Browser EVM wallet')
    } else {
      add(host.unisat, 'unisat', 'UniSat')
      add(host.okxwallet?.bitcoin, 'okx-bitcoin', 'OKX Bitcoin')
    }
  }
  if (kind === 'usdb') window.addEventListener('eip6963:announceProvider', announced)
  scan()
  window.addEventListener('focus', scan)
  return () => { window.removeEventListener('eip6963:announceProvider', announced); window.removeEventListener('focus', scan) }
}

export interface WalletState { status: 'disconnected' | 'loading' | 'connected' | 'error'; snapshot: WalletSnapshot | null; error?: string }

/** Invalidate every in-flight read on events, disconnect and disposal. No persisted wallet authority. */
export class WalletSession {
  private revision = 0
  private active = false
  private disposed = false
  private unsubscribe: (() => void) | undefined
  constructor(private adapter: WalletAdapter, private update: (state: WalletState) => void) {}

  async refresh(connect = false) {
    if (this.disposed || (!connect && !this.active)) return
    this.active = true
    const revision = ++this.revision
    this.update({ status: 'loading', snapshot: null })
    try {
      this.unsubscribe ??= this.adapter.subscribe(() => { void this.refresh() }, () => this.disconnect())
      const snapshot = await this.adapter.read(connect)
      if (revision === this.revision && !this.disposed) this.update({ status: 'connected', snapshot })
    } catch (error) {
      if (revision !== this.revision || this.disposed) return
      const code = (error as { code?: number })?.code
      const message = error instanceof Error ? error.message : ''
      this.update({ status: 'error', snapshot: null, error: code === 4001 ? 'WALLET_REJECTED' : code === 4902 ? 'WALLET_UNKNOWN_CHAIN' : /^WALLET_[A-Z_]+$/.test(message) ? message : 'WALLET_UNAVAILABLE' })
    }
  }

  async switchChain(target: string) {
    if (!this.active || this.disposed || !this.adapter.switchChain) return
    const revision = ++this.revision
    this.update({ status: 'loading', snapshot: null })
    try {
      await this.adapter.switchChain(target)
      if (revision === this.revision) await this.refresh()
    } catch (error) {
      if (revision !== this.revision || this.disposed) return
      const code = (error as { code?: number })?.code
      this.update({ status: 'error', snapshot: null, error: code === 4001 ? 'WALLET_REJECTED' : code === 4902 ? 'WALLET_UNKNOWN_CHAIN' : 'WALLET_UNAVAILABLE' })
    }
  }

  disconnect() {
    this.active = false
    ++this.revision
    this.unsubscribe?.()
    this.unsubscribe = undefined
    if (!this.disposed) this.update({ status: 'disconnected', snapshot: null })
  }

  dispose() { this.disposed = true; this.disconnect() }
}
