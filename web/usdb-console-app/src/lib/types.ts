export interface ServiceProbe<T> {
  name: string
  rpc_url: string
  reachable: boolean
  latency_ms?: number | null
  error?: string | null
  data?: T | null
}

export interface ArtifactSummary {
  path: string
  exists: boolean
  error?: string | null
  data?: Record<string, unknown> | null
}

export interface BootstrapStepSummary {
  step: string
  state: string
  artifact_path?: string | null
  error?: string | null
}

export interface BtcNodeSummary {
  chain?: string | null
  blocks?: number | null
  headers?: number | null
  best_block_hash?: string | null
  best_block_time?: number | null
  verification_progress?: number | null
  initial_block_download?: boolean | null
}

export interface BalanceHistorySummary {
  network?: string | null
  rpc_alive?: boolean | null
  query_ready?: boolean | null
  consensus_ready?: boolean | null
  phase?: string | null
  message?: string | null
  current?: number | null
  total?: number | null
  stable_height?: number | null
  stable_block_hash?: string | null
  latest_block_commit?: string | null
  snapshot_verification_state?: string | null
  snapshot_signing_key_id?: string | null
  script_registry?: BalanceHistoryScriptRegistryStatus | null
  blockers?: string[]
}

export interface BalanceHistoryScriptRegistryStatus {
  available: boolean
  count?: number | null
  policy: string
}

export interface UsdbIndexerSummary {
  network?: string | null
  rpc_alive?: boolean | null
  query_ready?: boolean | null
  consensus_ready?: boolean | null
  message?: string | null
  current?: number | null
  total?: number | null
  synced_block_height?: number | null
  balance_history_stable_height?: number | null
  upstream_snapshot_id?: string | null
  local_state_commit?: string | null
  system_state_id?: string | null
  blockers?: string[]
}

export interface EthwSummary {
  client_version?: string | null
  chain_id?: string | null
  network_id?: string | null
  block_number?: number | null
  latest_block_hash?: string | null
  latest_block_time?: number | null
  syncing?: boolean | Record<string, unknown> | null
  query_ready?: boolean | null
  consensus_ready?: boolean | null
}

export interface OrdSummary {
  http_status?: number | null
  backend_ready?: boolean | null
  query_ready?: boolean | null
  synced_block_height?: number | null
  btc_tip_height?: number | null
  sync_gap?: number | null
}

export interface ServicesSummary {
  btc_node: ServiceProbe<BtcNodeSummary>
  balance_history: ServiceProbe<BalanceHistorySummary>
  usdb_indexer: ServiceProbe<UsdbIndexerSummary>
  ethw: ServiceProbe<EthwSummary>
  ord: ServiceProbe<OrdSummary>
}

export interface CapabilitiesSummary {
  ord_available: boolean
  btc_console_mode: string
  btc_runtime_profile: string
  ethw_runtime_profile: string
}

export interface BootstrapSummary {
  bootstrap_manifest: ArtifactSummary
  snapshot_marker: ArtifactSummary
  ethw_init_marker: ArtifactSummary
  ethw_genesis: ArtifactSummary
  sourcedao_bootstrap_state: ArtifactSummary
  sourcedao_bootstrap_marker: ArtifactSummary
  steps: BootstrapStepSummary[]
  overall_state: string
}

export interface SourceDaoBootstrapOperation {
  name: string
  status: string
  tx_hash?: string | null
  details?: string | null
}

export interface SourceDaoBootstrapModule {
  address?: string | null
  source?: string | null
  implementation_address?: string | null
  proxy_tx_hash?: string | null
  implementation_tx_hash?: string | null
  wiring_tx_hash?: string | null
}

export interface SourceDaoBootstrapState {
  state_version?: string | null
  generated_at?: string | null
  completed_at?: string | null
  status?: string | null
  scope?: string | null
  message?: string | null
  current_step?: string | null
  last_error?: string | null
  rpc_url?: string | null
  repo_dir?: string | null
  config_path?: string | null
  artifacts_dir?: string | null
  chain_id?: number | null
  dao_address?: string | null
  dividend_address?: string | null
  bootstrap_admin?: string | null
  warnings?: string[]
  operations?: SourceDaoBootstrapOperation[]
  final_wiring?: Record<string, string | null>
  modules?: Record<string, SourceDaoBootstrapModule | null>
}

export interface ExplorerLinks {
  control_console: string
  balance_history: string
  usdb_indexer: string
  sourcedao_web: string
}

export interface AppEntry {
  id: string
  kind: string
  url: string
  target: string
  runtime_profile: string
  network?: string | null
  service_id?: string | null
  available: boolean
  status: string
  status_message?: string | null
  depends_on: string[]
}

export interface OverviewResponse {
  service: string
  generated_at_ms: number
  services: ServicesSummary
  capabilities: CapabilitiesSummary
  bootstrap: BootstrapSummary
  explorers: ExplorerLinks
  apps: AppEntry[]
}

export interface BalanceHistorySyncStatus {
  phase: string
  current: number
  total: number
  message?: string | null
}

export interface AddressBalanceRow {
  block_height: number
  balance: number
  delta: number
}

export interface UsdbRpcInfo {
  service: string
  api_version: string
  network: string
  features: string[]
}

export interface UsdbIndexerSyncStatus {
  genesis_block_height: number
  synced_block_height?: number | null
  balance_history_stable_height?: number | null
  current: number
  total: number
  message?: string | null
}

export interface PassStatsAtHeight {
  resolved_height: number
  total_count: number
  active_count: number
  dormant_count: number
  consumed_count: number
  burned_count: number
  invalid_count: number
}

export interface RpcActiveBalanceSnapshot {
  block_height: number
  total_balance: number
  active_address_count: number
}

export interface PassBlockCommitInfo {
  block_height: number
  balance_history_block_height: number
  balance_history_block_commit: string
  mutation_root: string
  block_commit: string
  commit_protocol_version: string
  commit_hash_algo: string
}

export interface PassSnapshot {
  inscription_id: string
  inscription_number: number
  mint_txid: string
  mint_block_height: number
  mint_owner: string
  eth_main: string
  eth_collab?: string | null
  prev: string[]
  invalid_code?: string | null
  invalid_reason?: string | null
  owner: string
  state: string
  satpoint: string
  last_event_id: number
  last_event_type: string
  resolved_height: number
}

export interface PassHistoryEvent {
  event_id: number
  inscription_id: string
  block_height: number
  event_type: string
  state: string
  owner: string
  satpoint: string
}

export interface PassHistoryPage {
  resolved_height: number
  total: number
  items: PassHistoryEvent[]
}

export interface PassEnergySnapshot {
  inscription_id: string
  query_block_height: number
  record_block_height: number
  state: string
  active_block_height: number
  owner_address: string
  owner_balance: number
  owner_delta: number
  energy: number
}

export interface PassEnergyRangeItem {
  inscription_id: string
  record_block_height: number
  state: string
  active_block_height: number
  owner_address: string
  owner_balance: number
  owner_delta: number
  energy: number
}

export interface PassEnergyRangePage {
  resolved_height: number
  total: number
  items: PassEnergyRangeItem[]
}

export interface PassEnergyLeaderboardItem {
  inscription_id: string
  owner: string
  record_block_height: number
  state: string
  energy: number
}

export interface PassEnergyLeaderboardPage {
  resolved_height: number
  total: number
  items: PassEnergyLeaderboardItem[]
}

export interface BtcMintPrepareRuntimeSummary {
  btc_network: string
  btc_runtime_profile: string
  btc_console_mode: string
  ord_available: boolean
  ord_query_ready: boolean
  balance_history_ready: boolean
  usdb_indexer_ready: boolean
  ord_synced_block_height?: number | null
  btc_tip_height?: number | null
  ord_sync_gap?: number | null
}

export interface BtcMintPrepareActivePassSummary {
  inscription_id: string
  state: string
  owner: string
  eth_main: string
  eth_collab?: string | null
  prev: string[]
}

export interface BtcMintPrepareResponse {
  eligible: boolean
  prepare_mode: string
  blockers: string[]
  warnings: string[]
  runtime: BtcMintPrepareRuntimeSummary
  owner_address: string
  owner_script_hash: string
  eth_main: string
  eth_collab?: string | null
  prev: string[]
  suggested_prev: string[]
  active_pass?: BtcMintPrepareActivePassSummary | null
  inscription_payload: Record<string, unknown>
  inscription_payload_json: string
  prepare_request: Record<string, unknown>
}

export interface BtcMintExecuteResponse {
  btc_network: string
  btc_runtime_profile: string
  wallet_name: string
  owner_address: string
  inscription_payload_json: string
  inscription_id: string
  txid?: string | null
  ord_output: string
}

export interface BtcWorldSimIdentity {
  agent_id: number
  wallet_name: string
  owner_address: string
  is_ethw_aligned: boolean
}

export interface BtcWorldSimIdentitiesResponse {
  btc_network?: string | null
  btc_runtime_profile: string
  available: boolean
  marker_path: string
  identities: BtcWorldSimIdentity[]
  error?: string | null
}

export interface BtcWorldSimDevSignerResponse {
  btc_network?: string | null
  btc_runtime_profile: string
  available: boolean
  wallet_name: string
  owner_address?: string | null
  wif?: string | null
  error?: string | null
}

export interface EthwDevIdentityResponse {
  ethw_chain_id?: string | null
  ethw_network_id?: string | null
  ethw_runtime_profile: string
  available: boolean
  marker_path: string
  address?: string | null
  identity_mode?: string | null
  identity_scheme?: string | null
  identity_fingerprint?: string | null
  error?: string | null
}

export interface EthwAddressStatusResponse {
  ethw_chain_id?: string | null
  ethw_network_id?: string | null
  ethw_runtime_profile: string
  address: string
  balance_wei?: string | null
  latest_block_number?: string | null
  available: boolean
  error?: string | null
}
