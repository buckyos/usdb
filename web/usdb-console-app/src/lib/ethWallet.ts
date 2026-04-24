export interface EthWalletSnapshot {
  source: string
  address: string | null
  chainId: string | null
  networkId: string | null
  balanceWei: string | null
}

interface Eip1193Provider {
  isMetaMask?: boolean
  request: (args: { method: string; params?: unknown[] }) => Promise<unknown>
}

interface EthereumWindow extends Window {
  ethereum?: Eip1193Provider
}

function getEthereumProvider() {
  if (typeof window === 'undefined') return null
  return (window as EthereumWindow).ethereum ?? null
}

function normalizeAddress(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const trimmed = value.trim()
  return /^0x[a-fA-F0-9]{40}$/.test(trimmed) ? trimmed : null
}

function normalizeHexQuantity(value: unknown): string | null {
  if (typeof value !== 'string') return null
  const trimmed = value.trim().toLowerCase()
  if (!/^0x[0-9a-f]+$/.test(trimmed)) return null
  return trimmed
}

function firstAccount(value: unknown): string | null {
  if (!Array.isArray(value)) return null
  return normalizeAddress(value[0])
}

function providerSource(provider: Eip1193Provider) {
  return provider.isMetaMask ? 'MetaMask' : 'EIP-1193'
}

async function readProviderSnapshot(
  provider: Eip1193Provider,
  account: string | null,
): Promise<EthWalletSnapshot> {
  const [chainId, networkId, balanceWei] = await Promise.all([
    provider.request({ method: 'eth_chainId' }).catch(() => null),
    provider.request({ method: 'net_version' }).catch(() => null),
    account
      ? provider.request({ method: 'eth_getBalance', params: [account, 'latest'] }).catch(() => null)
      : Promise.resolve(null),
  ])

  return {
    source: providerSource(provider),
    address: account,
    chainId: normalizeHexQuantity(chainId),
    networkId: typeof networkId === 'string' ? networkId : null,
    balanceWei: normalizeHexQuantity(balanceWei),
  }
}

export function detectEthWalletProvider() {
  return getEthereumProvider() != null
}

export async function readEthWalletSnapshot(): Promise<EthWalletSnapshot | null> {
  const provider = getEthereumProvider()
  if (!provider) return null

  const accounts = await provider.request({ method: 'eth_accounts' }).catch(() => [])
  return readProviderSnapshot(provider, firstAccount(accounts))
}

export async function connectEthWallet(): Promise<EthWalletSnapshot> {
  const provider = getEthereumProvider()
  if (!provider) {
    throw new Error('No injected EIP-1193 / MetaMask wallet provider was found.')
  }

  const accounts = await provider.request({ method: 'eth_requestAccounts' })
  const account = firstAccount(accounts)
  if (!account) {
    throw new Error('The injected ETH wallet did not return a valid account.')
  }

  return readProviderSnapshot(provider, account)
}
