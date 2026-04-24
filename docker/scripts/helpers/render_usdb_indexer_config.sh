#!/usr/bin/env bash
set -euo pipefail

output_path="${1:-${USDB_INDEXER_ROOT_DIR:-/data/usdb-indexer}/config.json}"
root_dir="${USDB_INDEXER_ROOT_DIR:-/data/usdb-indexer}"

mkdir -p "${root_dir}" "$(dirname "${output_path}")"

emit_btc_auth_json() {
  case "${BTC_AUTH_MODE:-cookie}" in
    cookie)
      if [[ -n "${BTC_COOKIE_FILE:-}" ]]; then
        printf '"auth": {"CookieFile": "%s"},\n' "${BTC_COOKIE_FILE}"
      fi
      ;;
    userpass)
      : "${BTC_RPC_USER:?BTC_RPC_USER is required when BTC_AUTH_MODE=userpass}"
      : "${BTC_RPC_PASSWORD:?BTC_RPC_PASSWORD is required when BTC_AUTH_MODE=userpass}"
      printf '"auth": {"UserPass": ["%s", "%s"]},\n' "${BTC_RPC_USER}" "${BTC_RPC_PASSWORD}"
      ;;
    none)
      printf '"auth": "None",\n'
      ;;
    *)
      echo "Unsupported BTC_AUTH_MODE=${BTC_AUTH_MODE:-}" >&2
      exit 1
      ;;
  esac
}

fixture_json="null"
if [[ -n "${INSCRIPTION_FIXTURE_FILE:-}" ]]; then
  fixture_json="\"${INSCRIPTION_FIXTURE_FILE}\""
fi

cat >"${output_path}" <<EOF
{
  "isolate": null,
  "bitcoin": {
    "network": "${BTC_NETWORK:-bitcoin}",
    "data_dir": "${BTC_DATA_DIR:-/data/bitcoind}",
    "rpc_url": "${BTC_RPC_URL:-http://btc-node:8332}",
EOF

emit_btc_auth_json >>"${output_path}"

cat >>"${output_path}" <<EOF
    "block_magic": null
  },
  "ordinals": {
    "rpc_url": "${ORD_RPC_URL:-http://ord-server:28030}"
  },
  "balance_history": {
    "rpc_url": "${BALANCE_HISTORY_RPC_URL:-http://balance-history:28010}"
  },
  "usdb": {
    "genesis_block_height": ${USDB_GENESIS_BLOCK_HEIGHT:-900000},
    "active_address_page_size": ${ACTIVE_ADDRESS_PAGE_SIZE:-1024},
    "balance_query_batch_size": ${BALANCE_QUERY_BATCH_SIZE:-1024},
    "balance_query_concurrency": ${BALANCE_QUERY_CONCURRENCY:-4},
    "balance_query_timeout_ms": ${BALANCE_QUERY_TIMEOUT_MS:-10000},
    "balance_query_max_retries": ${BALANCE_QUERY_MAX_RETRIES:-2},
    "inscription_source": "${INSCRIPTION_SOURCE:-bitcoind}",
    "inscription_fixture_file": ${fixture_json},
    "inscription_source_shadow_compare": ${INSCRIPTION_SOURCE_SHADOW_COMPARE:-false},
    "inscription_source_shadow_fail_fast": ${INSCRIPTION_SOURCE_SHADOW_FAIL_FAST:-false},
    "rpc_server_host": "${USDB_INDEXER_RPC_HOST:-0.0.0.0}",
    "rpc_server_port": ${USDB_INDEXER_RPC_PORT:-28020},
    "rpc_server_enabled": ${USDB_RPC_SERVER_ENABLED:-true},
    "pass_energy_leaderboard_cache_enabled": ${PASS_ENERGY_LEADERBOARD_CACHE_ENABLED:-true},
    "pass_energy_leaderboard_cache_top_k": ${PASS_ENERGY_LEADERBOARD_CACHE_TOP_K:-1000}
  }
}
EOF
