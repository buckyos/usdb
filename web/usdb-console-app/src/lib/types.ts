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
  blockers?: string[]
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
  syncing?: boolean | Record<string, unknown> | null
  query_ready?: boolean | null
  consensus_ready?: boolean | null
}

export interface ServicesSummary {
  btc_node: ServiceProbe<BtcNodeSummary>
  balance_history: ServiceProbe<BalanceHistorySummary>
  usdb_indexer: ServiceProbe<UsdbIndexerSummary>
  ethw: ServiceProbe<EthwSummary>
}

export interface BootstrapSummary {
  bootstrap_manifest: ArtifactSummary
  snapshot_marker: ArtifactSummary
  ethw_init_marker: ArtifactSummary
  sourcedao_bootstrap_state: ArtifactSummary
  sourcedao_bootstrap_marker: ArtifactSummary
  steps: BootstrapStepSummary[]
  overall_state: string
}

export interface ExplorerLinks {
  control_console: string
  balance_history: string
  usdb_indexer: string
}

export interface OverviewResponse {
  service: string
  generated_at_ms: number
  services: ServicesSummary
  bootstrap: BootstrapSummary
  explorers: ExplorerLinks
}
