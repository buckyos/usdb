// Run with node --test. Bundles the real TypeScript with the app's existing esbuild.
import assert from 'node:assert/strict'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'
import { webcrypto } from 'node:crypto'
import { build } from '../web/usdb-console-app/node_modules/esbuild/lib/main.js'

const app = fileURLToPath(new URL('../web/usdb-console-app/', import.meta.url))
const bundle = await build({
  stdin: { contents: `export * from './src/lib/walletIdentity'; export * as dev from './src/lib/btcWallet'; export { WIF, TEST_NETWORK } from '@scure/btc-signer'`, resolveDir: app },
  bundle: true, write: false, platform: 'browser', format: 'esm', target: 'es2022',
})
globalThis.crypto ??= webcrypto
const wallet = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].contents).toString('base64')}`)
const address = `0x${'1'.repeat(40)}`
const hash = `0x${'a'.repeat(64)}`

class Host extends EventTarget {
  localStorage = new Map()
  constructor() {
    super()
    this.localStorage.removeItem = this.localStorage.delete.bind(this.localStorage)
  }
}
globalThis.CustomEvent ??= class extends Event { constructor(type, init) { super(type); this.detail = init.detail } }

function events() {
  const listeners = new Map()
  return {
    listeners,
    on(event, fn) { if (!listeners.has(event)) listeners.set(event, new Set()); listeners.get(event).add(fn) },
    removeListener(event, fn) { listeners.get(event)?.delete(fn) },
    emit(event) { for (const listener of listeners.get(event) ?? []) listener() },
  }
}
function fixture(kind, provider) {
  globalThis.window = new Host()
  if (kind === 'usdb') window.ethereum = provider
  else window.unisat = provider
  let adapters = []
  const stop = wallet.discoverWallets(kind, values => { adapters = values })
  return { adapter: adapters[0], stop, get adapters() { return adapters } }
}
const tick = () => new Promise(resolve => setTimeout(resolve, 0))

test('quantities stay exact; Bitcoin checksum and actual chain identity are independent', () => {
  assert.equal(wallet.chainId(123), '0x7b')
  assert.equal(wallet.chainId('0x007B'), '0x7b')
  assert.equal(wallet.chainId(Number.MAX_SAFE_INTEGER + 1), null)
  assert.equal(wallet.chainId('1e3'), null)
  assert.equal(wallet.nativeBalance('0xde0b6b3a7640001'), '1.000000000000000001')
  assert.equal(wallet.nativeBalance('0x0'), '0')
  const btc = '1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa'
  assert.equal(wallet.bitcoinAddress(btc, 'mainnet'), btc)
  assert.equal(wallet.bitcoinAddress(btc, 'testnet'), null)
  assert.equal(wallet.bitcoinAddress(btc.slice(0, -1) + 'b', 'mainnet'), null)
  assert.equal(wallet.bitcoinNetwork('FRACTAL_BITCOIN_MAINNET'), null)
  assert.equal(wallet.bitcoinNetwork('__proto__'), null)
  assert.equal(wallet.bitcoinNetwork('signet'), 'signet')
  assert.equal(wallet.bitcoinNetwork('btc-mainnet'), 'mainnet')
  assert.equal(wallet.bitcoinNetwork('btc-regtest'), 'regtest')
})

test('discovery deduplicates EIP-6963 and fallback without asking for account permissions', async () => {
  let requests = 0
  const provider = { ...events(), async request() { requests++; return [] } }
  const found = fixture('usdb', provider)
  const announce = (uuid, p = provider) => window.dispatchEvent(new CustomEvent('eip6963:announceProvider', { detail: { info: { uuid, name: 'Example' }, provider: p } }))
  announce('first')
  announce('second', { ...provider })
  announce('second', { ...provider })
  assert.equal(found.adapters.length, 2)
  assert.equal(requests, 0)
  found.stop()
  announce('third', { ...provider })
  assert.equal(found.adapters.length, 2)
})

test('EVM reads validate accounts and chain after genesis, rejecting a mixed observation', async () => {
  let calls = 0
  const provider = { ...events(), async request({ method }) {
    if (method === 'eth_accounts') return [address]
    if (method === 'eth_chainId') return ++calls === 1 ? '0x7b' : '0x7c'
    if (method === 'eth_getBlockByNumber') return { hash }
    throw new Error('Unexpected method')
  } }
  const found = fixture('usdb', provider)
  await assert.rejects(found.adapter.read(false), /WALLET_CHANGED/)
  found.stop()
})

test('BTC getChain takes precedence over ambiguous legacy network and reads never connect implicitly', async () => {
  let requests = 0
  const provider = { ...events(), async requestAccounts() { requests++; return [] },
    async getAccounts() { return ['1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa'] },
    async getNetwork() { return 'livenet' }, async getChain() { return { enum: 'FRACTAL_BITCOIN_MAINNET' } },
  }
  const found = fixture('btc', provider)
  assert.equal((await found.adapter.read(false)).network, null)
  assert.equal(requests, 0)
  provider.getChain = async () => ({ enum: 'BITCOIN_MAINNET' })
  assert.equal((await found.adapter.read(true)).network, 'mainnet')
  assert.equal(requests, 1)
  found.stop()
})

test('events clear old identity immediately; disconnect drops late reads and releases listeners', async () => {
  const providerEvents = events()
  let current = address
  let release
  let slow = false
  const found = fixture('usdb', { ...providerEvents, async request({ method }) {
    if (method === 'eth_requestAccounts') return [current]
    if (method === 'eth_accounts') return [current]
    if (method === 'eth_chainId') return '0x7b'
    if (method === 'eth_getBlockByNumber') {
      if (slow) await new Promise(resolve => { release = resolve })
      return { hash }
    }
    throw new Error('Unexpected method')
  } })
  let state
  const session = new wallet.WalletSession(found.adapter, value => { state = value })
  await session.refresh(true)
  assert.equal(state.snapshot.address, address)
  slow = true
  current = `0x${'2'.repeat(40)}`
  providerEvents.emit('accountsChanged')
  assert.equal(state.status, 'loading')
  assert.equal(state.snapshot, null)
  await tick()
  session.disconnect()
  release()
  await tick()
  assert.equal(state.status, 'disconnected')
  assert.equal([...providerEvents.listeners.values()].flatMap(set => [...set]).length, 0)
  await session.refresh()
  assert.equal(state.status, 'disconnected')
  session.dispose()
  found.stop()
})

test('authorization rejection is distinct and disposed sessions cannot publish pending results', async () => {
  let state
  const adapter = { subscribe: () => () => {}, read: async () => { throw { code: 4001 } } }
  const session = new wallet.WalletSession(adapter, value => { state = value })
  await session.refresh(true)
  assert.equal(state.error, 'WALLET_REJECTED')
  let release
  adapter.read = () => new Promise(resolve => { release = resolve })
  const pending = session.refresh(true)
  session.dispose()
  release({ address, network: '0x7b', genesis: hash })
  await pending
  assert.equal(state.status, 'loading')
  assert.equal(state.snapshot, null)
})

test('developer WIF is neither restored from nor written to browser storage', async () => {
  globalThis.window = new Host()
  const key = 'usdb.devRegtestWallet.v1'
  window.localStorage.set(key, 'legacy-secret-must-not-be-restored')
  assert.equal(await wallet.dev.readDevRegtestWalletSnapshot(), null)
  assert.equal(window.localStorage.has(key), false)
  const bytes = new Uint8Array(32); bytes[31] = 1 // Public test vector, not an operator key.
  const wif = wallet.WIF(wallet.TEST_NETWORK).encode(bytes)
  const snapshot = await wallet.dev.importDevRegtestWallet({ wif })
  assert.ok(snapshot.address.startsWith('bcrt1'))
  assert.equal(window.localStorage.size, 0)
  wallet.dev.clearDevRegtestWallet()
  assert.equal(await wallet.dev.readDevRegtestWalletSnapshot(), null)
})
