import { TEST_NETWORK, Transaction, WIF, getAddress } from '@scure/btc-signer'
import { pubECDSA, sha256, signECDSA } from '@scure/btc-signer/utils.js'

export interface BtcWalletBalance {
  confirmed: number
  unconfirmed: number
  total: number
}

export interface BtcWalletInscription {
  inscriptionId: string
  inscriptionNumber?: number | string
  address?: string
  outputValue?: number
  timestamp?: number
  output?: string
  location?: string
  contentType?: string
}

export interface BtcWalletSnapshot {
  mode: BtcWalletMode
  source: string
  address: string | null
  addresses: string[]
  publicKey: string | null
  network: string | null
  balance: BtcWalletBalance | null
  inscriptions: BtcWalletInscription[]
}

export type BtcWalletMode = 'browser' | 'dev-regtest'
export type BtcPsbtTextFormat = 'base64' | 'hex'

export interface BtcWalletMessageSignatureResult {
  mode: BtcWalletMode
  source: string
  signature: string
  signatureType: string
}

export interface BtcWalletPsbtSignatureOptions {
  finalize?: boolean
}

export interface BtcWalletPsbtSignatureResult {
  mode: BtcWalletMode
  source: string
  inputFormat: BtcPsbtTextFormat | null
  outputPsbt: string
  finalized: boolean | null
  extractedTxHex: string | null
  signedInputs: number | null
}

export interface DevRegtestPsbtSignatureResult {
  inputFormat: BtcPsbtTextFormat
  outputPsbt: string
  finalized: boolean
  extractedTxHex: string | null
  signedInputs: number
}

export interface BtcWalletAdapterCapabilities {
  canConnect: boolean
  canReadSnapshot: boolean
  canSignMessage: boolean
  canSignPsbt: boolean
}

export interface BtcWalletAdapter {
  mode: BtcWalletMode
  source: string
  capabilities: BtcWalletAdapterCapabilities
  connect?: () => Promise<BtcWalletSnapshot>
  readSnapshot: () => Promise<BtcWalletSnapshot | null>
  signMessage?: (message: string) => Promise<BtcWalletMessageSignatureResult>
  signPsbt?: (
    psbt: string,
    options?: BtcWalletPsbtSignatureOptions,
  ) => Promise<BtcWalletPsbtSignatureResult>
}

interface DevRegtestWalletRecord {
  wif: string
  address: string
  publicKey: string
  importedAt: string
}

interface InjectedBtcProvider {
  connect?: () => Promise<{ address?: string; publicKey?: string }>
  requestAccounts?: () => Promise<string[]>
  getAccounts?: () => Promise<string[]>
  getPublicKey?: () => Promise<string>
  getNetwork?: () => Promise<string>
  getBalance?: () => Promise<BtcWalletBalance>
  getInscriptions?: (cursor?: number, size?: number) => Promise<{ list?: BtcWalletInscription[] }>
  signMessage?: (message: string, type?: 'ecdsa' | 'bip322-simple') => Promise<string>
  signPsbt?: (psbt: string, options?: { autoFinalized?: boolean }) => Promise<string>
  signPsbts?: (psbts: string[], options?: { autoFinalized?: boolean }) => Promise<string[]>
}

interface InjectedWindowLike {
  okxwallet?: {
    bitcoin?: InjectedBtcProvider
  }
  bitcoin?: InjectedBtcProvider
}

interface BufferLike {
  from(value: Uint8Array | string, encoding?: string): ArrayLike<number> & {
    toString(encoding: string): string
  }
}

declare global {
  interface Window extends InjectedWindowLike {}
}

const DEV_REGTEST_STORAGE_KEY = 'usdb.devRegtestWallet.v1'
const REGTEST_NETWORK = {
  ...TEST_NETWORK,
  bech32: 'bcrt',
}

function getGlobalWindow(): InjectedWindowLike | null {
  if (typeof window === 'undefined') return null
  return window
}

function getStorage(): Storage | null {
  if (typeof window === 'undefined') return null
  return window.localStorage
}

function bytesToHex(bytes: Uint8Array) {
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('')
}

function bytesToBase64(bytes: Uint8Array) {
  if (typeof btoa !== 'function') {
    const buffer = (globalThis as { Buffer?: BufferLike }).Buffer
    if (!buffer) {
      throw new Error('No base64 encoder is available in the current runtime.')
    }
    return buffer.from(bytes).toString('base64')
  }
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary)
}

function base64ToBytes(value: string) {
  if (typeof atob !== 'function') {
    const buffer = (globalThis as { Buffer?: BufferLike }).Buffer
    if (!buffer) {
      throw new Error('No base64 decoder is available in the current runtime.')
    }
    return Uint8Array.from(buffer.from(value, 'base64'))
  }

  const normalized = value.replace(/\s+/g, '')
  const binary = atob(normalized)
  return Uint8Array.from(binary, (char) => char.charCodeAt(0))
}

function hexToBytes(value: string) {
  const normalized = value.trim().replace(/\s+/g, '')
  if (!/^[0-9a-fA-F]+$/.test(normalized) || normalized.length % 2 !== 0) {
    throw new Error('PSBT hex input must contain an even-length hexadecimal string.')
  }

  const result = new Uint8Array(normalized.length / 2)
  for (let i = 0; i < normalized.length; i += 2) {
    result[i / 2] = Number.parseInt(normalized.slice(i, i + 2), 16)
  }
  return result
}

function decodePsbtText(value: string): { bytes: Uint8Array; format: BtcPsbtTextFormat } {
  const normalized = value.trim()
  if (!normalized) {
    throw new Error('A PSBT payload is required.')
  }

  const compact = normalized.replace(/\s+/g, '')
  if (/^[0-9a-fA-F]+$/.test(compact) && compact.length % 2 === 0) {
    return { bytes: hexToBytes(compact), format: 'hex' }
  }

  try {
    return { bytes: base64ToBytes(compact), format: 'base64' }
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new Error(`Failed to decode PSBT payload: ${message}`)
  }
}

function readDevRegtestWalletRecord(): DevRegtestWalletRecord | null {
  const storage = getStorage()
  if (!storage) return null

  const raw = storage.getItem(DEV_REGTEST_STORAGE_KEY)
  if (!raw) return null

  try {
    const parsed = JSON.parse(raw) as Partial<DevRegtestWalletRecord>
    if (
      typeof parsed.wif !== 'string' ||
      typeof parsed.address !== 'string' ||
      typeof parsed.publicKey !== 'string'
    ) {
      return null
    }

    return {
      wif: parsed.wif,
      address: parsed.address,
      publicKey: parsed.publicKey,
      importedAt: typeof parsed.importedAt === 'string' ? parsed.importedAt : new Date().toISOString(),
    }
  } catch {
    return null
  }
}

function writeDevRegtestWalletRecord(record: DevRegtestWalletRecord) {
  const storage = getStorage()
  if (!storage) return
  storage.setItem(DEV_REGTEST_STORAGE_KEY, JSON.stringify(record))
}

function parseRegtestWalletFromWif(wif: string) {
  const normalizedWif = wif.trim()
  if (!normalizedWif) {
    throw new Error('A regtest WIF private key is required.')
  }

  const privateKey = WIF(REGTEST_NETWORK).decode(normalizedWif)
  const publicKey = pubECDSA(privateKey, true)
  const derivedAddress = getAddress('wpkh', privateKey, REGTEST_NETWORK)

  if (!derivedAddress) {
    throw new Error('Failed to derive a regtest address from the provided WIF.')
  }

  return {
    normalizedWif,
    privateKey,
    publicKey,
    derivedAddress,
  }
}

async function connectInjectedBtcWallet(
  provider: InjectedBtcProvider,
  source: string,
): Promise<BtcWalletSnapshot> {
  let connectedAddress: string | null = null
  let connectedPublicKey: string | null = null

  if (provider.connect) {
    const result = await provider.connect()
    connectedAddress = result.address ?? null
    connectedPublicKey = result.publicKey ?? null
  } else if (provider.requestAccounts) {
    const addresses = await provider.requestAccounts()
    connectedAddress = addresses[0] ?? null
  } else {
    throw new Error('The injected BTC wallet does not expose a supported connect method.')
  }

  const accounts = provider.getAccounts ? await provider.getAccounts() : []
  const address = connectedAddress ?? accounts[0] ?? null
  const publicKey =
    connectedPublicKey ?? (provider.getPublicKey ? await provider.getPublicKey() : null)
  const network = provider.getNetwork ? await provider.getNetwork() : null
  const balance = provider.getBalance ? await provider.getBalance() : null
  const inscriptionsResult = provider.getInscriptions
    ? await provider.getInscriptions(0, 10)
    : null

  return {
    mode: 'browser',
    source,
    address,
    addresses: accounts.length > 0 ? accounts : address ? [address] : [],
    publicKey,
    network,
    balance,
    inscriptions: inscriptionsResult?.list ?? [],
  }
}

async function readInjectedBtcWalletSnapshot(
  provider: InjectedBtcProvider,
  source: string,
): Promise<BtcWalletSnapshot | null> {
  const addresses = provider.getAccounts ? await provider.getAccounts() : []
  const address = addresses[0] ?? null
  const publicKey = provider.getPublicKey ? await provider.getPublicKey() : null
  const network = provider.getNetwork ? await provider.getNetwork() : null
  const balance = provider.getBalance ? await provider.getBalance() : null
  const inscriptionsResult = provider.getInscriptions
    ? await provider.getInscriptions(0, 10)
    : null

  return {
    mode: 'browser',
    source,
    address,
    addresses,
    publicKey,
    network,
    balance,
    inscriptions: inscriptionsResult?.list ?? [],
  }
}

export function detectBtcWalletProvider(): {
  provider: InjectedBtcProvider
  source: string
} | null {
  const globalWindow = getGlobalWindow()
  if (!globalWindow) return null

  if (globalWindow.okxwallet?.bitcoin) {
    return {
      provider: globalWindow.okxwallet.bitcoin,
      source: 'OKX Wallet',
    }
  }

  if (globalWindow.bitcoin) {
    return {
      provider: globalWindow.bitcoin,
      source: 'Injected Bitcoin Provider',
    }
  }

  return null
}

export async function connectBtcWallet(): Promise<BtcWalletSnapshot> {
  const detected = detectBtcWalletProvider()
  if (!detected) {
    throw new Error('No injected BTC wallet provider was found.')
  }

  const { provider, source } = detected
  return connectInjectedBtcWallet(provider, source)
}

export async function readBtcWalletSnapshot(): Promise<BtcWalletSnapshot | null> {
  const detected = detectBtcWalletProvider()
  if (!detected) return null

  const { provider, source } = detected
  return readInjectedBtcWalletSnapshot(provider, source)
}

export async function importDevRegtestWallet(params: {
  wif: string
  address?: string | null
}): Promise<BtcWalletSnapshot> {
  const { normalizedWif, publicKey, derivedAddress } = parseRegtestWalletFromWif(params.wif)
  const address = params.address?.trim() || derivedAddress

  writeDevRegtestWalletRecord({
    wif: normalizedWif,
    address,
    publicKey: bytesToHex(publicKey),
    importedAt: new Date().toISOString(),
  })

  return {
    mode: 'dev-regtest',
    source: 'Dev Regtest Wallet',
    address,
    addresses: [address],
    publicKey: bytesToHex(publicKey),
    network: 'regtest',
    balance: null,
    inscriptions: [],
  }
}

export async function readDevRegtestWalletSnapshot(): Promise<BtcWalletSnapshot | null> {
  const record = readDevRegtestWalletRecord()
  if (!record) return null

  return {
    mode: 'dev-regtest',
    source: 'Dev Regtest Wallet',
    address: record.address,
    addresses: [record.address],
    publicKey: record.publicKey,
    network: 'regtest',
    balance: null,
    inscriptions: [],
  }
}

export function clearDevRegtestWallet() {
  const storage = getStorage()
  if (!storage) return
  storage.removeItem(DEV_REGTEST_STORAGE_KEY)
}

export async function signDevRegtestWalletMessage(message: string): Promise<string> {
  const record = readDevRegtestWalletRecord()
  if (!record) {
    throw new Error('No dev regtest wallet is currently imported.')
  }

  const { privateKey } = parseRegtestWalletFromWif(record.wif)
  const digest = sha256(new TextEncoder().encode(message))
  const signature = signECDSA(digest, privateKey)
  return bytesToBase64(signature)
}

export async function signDevRegtestWalletPsbt(
  psbtText: string,
  options?: { finalize?: boolean },
): Promise<DevRegtestPsbtSignatureResult> {
  const record = readDevRegtestWalletRecord()
  if (!record) {
    throw new Error('No dev regtest wallet is currently imported.')
  }

  const { privateKey } = parseRegtestWalletFromWif(record.wif)
  const { bytes, format } = decodePsbtText(psbtText)
  const transaction = Transaction.fromPSBT(bytes)
  const signedInputs = transaction.sign(privateKey)

  if (signedInputs === 0) {
    throw new Error('The imported dev wallet could not sign any input in this PSBT.')
  }

  let finalized = false
  let extractedTxHex: string | null = null
  if (options?.finalize) {
    transaction.finalize()
    extractedTxHex = bytesToHex(transaction.extract())
    finalized = true
  }

  const outputBytes = transaction.toPSBT()
  return {
    inputFormat: format,
    outputPsbt: format === 'hex' ? bytesToHex(outputBytes) : bytesToBase64(outputBytes),
    finalized,
    extractedTxHex,
    signedInputs,
  }
}

export function getBtcWalletAdapter(mode: BtcWalletMode): BtcWalletAdapter | null {
  if (mode === 'browser') {
    const detected = detectBtcWalletProvider()
    if (!detected) return null

    const { provider, source } = detected
    return {
      mode: 'browser',
      source,
      capabilities: {
        canConnect: Boolean(provider.connect || provider.requestAccounts),
        canReadSnapshot: true,
        canSignMessage: Boolean(provider.signMessage),
        canSignPsbt: Boolean(provider.signPsbt || provider.signPsbts),
      },
      connect: () => connectInjectedBtcWallet(provider, source),
      readSnapshot: () => readInjectedBtcWalletSnapshot(provider, source),
      signMessage: provider.signMessage
        ? async (message) => ({
            mode: 'browser',
            source,
            signature: await provider.signMessage!(message, 'ecdsa'),
            signatureType: 'provider',
          })
        : undefined,
      signPsbt: provider.signPsbt
        ? async (psbt, options) => {
            const { format } = decodePsbtText(psbt)
            return {
              mode: 'browser',
              source,
              inputFormat: format,
              outputPsbt: await provider.signPsbt!(psbt, {
                autoFinalized: Boolean(options?.finalize),
              }),
              finalized: null,
              extractedTxHex: null,
              signedInputs: null,
            }
          }
        : undefined,
    }
  }

  return {
    mode: 'dev-regtest',
    source: 'Dev Regtest Wallet',
    capabilities: {
      canConnect: true,
      canReadSnapshot: true,
      canSignMessage: true,
      canSignPsbt: true,
    },
    connect: () => readDevRegtestWalletSnapshot().then((snapshot) => {
      if (!snapshot) {
        throw new Error('No dev regtest wallet is currently imported.')
      }
      return snapshot
    }),
    readSnapshot: readDevRegtestWalletSnapshot,
    signMessage: async (message) => ({
      mode: 'dev-regtest',
      source: 'Dev Regtest Wallet',
      signature: await signDevRegtestWalletMessage(message),
      signatureType: 'ecdsa-sha256-dev',
    }),
    signPsbt: async (psbt, options) => {
      const result = await signDevRegtestWalletPsbt(psbt, options)
      return {
        mode: 'dev-regtest',
        source: 'Dev Regtest Wallet',
        inputFormat: result.inputFormat,
        outputPsbt: result.outputPsbt,
        finalized: result.finalized,
        extractedTxHex: result.extractedTxHex,
        signedInputs: result.signedInputs,
      }
    },
  }
}

function requireBtcWalletAdapter(mode: BtcWalletMode) {
  const adapter = getBtcWalletAdapter(mode)
  if (!adapter) {
    throw new Error(`No BTC wallet adapter is currently available for mode: ${mode}`)
  }
  return adapter
}

export function getBtcWalletAdapterCapabilities(mode: BtcWalletMode): BtcWalletAdapterCapabilities | null {
  return getBtcWalletAdapter(mode)?.capabilities ?? null
}

export async function connectBtcWalletByMode(mode: BtcWalletMode): Promise<BtcWalletSnapshot> {
  const adapter = requireBtcWalletAdapter(mode)
  if (!adapter.connect) {
    throw new Error(`BTC wallet mode ${mode} does not support connect.`)
  }
  return adapter.connect()
}

export async function readBtcWalletSnapshotByMode(mode: BtcWalletMode): Promise<BtcWalletSnapshot | null> {
  const adapter = getBtcWalletAdapter(mode)
  if (!adapter) return null
  return adapter.readSnapshot()
}

export async function signBtcWalletMessage(
  mode: BtcWalletMode,
  message: string,
): Promise<BtcWalletMessageSignatureResult> {
  const adapter = requireBtcWalletAdapter(mode)
  if (!adapter.signMessage) {
    throw new Error(`BTC wallet mode ${mode} does not expose message signing.`)
  }
  return adapter.signMessage(message)
}

export async function signBtcWalletPsbt(
  mode: BtcWalletMode,
  psbt: string,
  options?: BtcWalletPsbtSignatureOptions,
): Promise<BtcWalletPsbtSignatureResult> {
  const adapter = requireBtcWalletAdapter(mode)
  if (!adapter.signPsbt) {
    throw new Error(`BTC wallet mode ${mode} does not expose PSBT signing.`)
  }
  return adapter.signPsbt(psbt, options)
}
