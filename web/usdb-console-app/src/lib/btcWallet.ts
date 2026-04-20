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
  source: string
  address: string | null
  addresses: string[]
  publicKey: string | null
  network: string | null
  balance: BtcWalletBalance | null
  inscriptions: BtcWalletInscription[]
}

interface InjectedBtcProvider {
  connect?: () => Promise<{ address?: string; publicKey?: string }>
  requestAccounts?: () => Promise<string[]>
  getAccounts?: () => Promise<string[]>
  getPublicKey?: () => Promise<string>
  getNetwork?: () => Promise<string>
  getBalance?: () => Promise<BtcWalletBalance>
  getInscriptions?: (cursor?: number, size?: number) => Promise<{ list?: BtcWalletInscription[] }>
}

interface InjectedWindowLike {
  okxwallet?: {
    bitcoin?: InjectedBtcProvider
  }
  bitcoin?: InjectedBtcProvider
}

declare global {
  interface Window extends InjectedWindowLike {}
}

function getGlobalWindow(): InjectedWindowLike | null {
  if (typeof window === 'undefined') return null
  return window
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
    source,
    address,
    addresses: accounts.length > 0 ? accounts : address ? [address] : [],
    publicKey,
    network,
    balance,
    inscriptions: inscriptionsResult?.list ?? [],
  }
}

export async function readBtcWalletSnapshot(): Promise<BtcWalletSnapshot | null> {
  const detected = detectBtcWalletProvider()
  if (!detected) return null

  const { provider, source } = detected
  const addresses = provider.getAccounts ? await provider.getAccounts() : []
  const address = addresses[0] ?? null
  const publicKey = provider.getPublicKey ? await provider.getPublicKey() : null
  const network = provider.getNetwork ? await provider.getNetwork() : null
  const balance = provider.getBalance ? await provider.getBalance() : null
  const inscriptionsResult = provider.getInscriptions
    ? await provider.getInscriptions(0, 10)
    : null

  return {
    source,
    address,
    addresses,
    publicKey,
    network,
    balance,
    inscriptions: inscriptionsResult?.list ?? [],
  }
}
