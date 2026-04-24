#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="${USDB_INDEXER_ROOT_DIR:-/data/usdb-indexer}"
config_path="${root_dir}/config.json"

"${script_dir}/../helpers/render_usdb_indexer_config.sh" "${config_path}"

wait_url() {
  local url="$1"
  local timeout_secs="$2"
  local target="${url#*://}"
  local host="${target%%:*}"
  local port="${target##*:}"
  "${script_dir}/../helpers/wait_for_tcp.sh" "${host}" "${port}" "${timeout_secs}"
}

wait_url "${BALANCE_HISTORY_RPC_URL:-http://balance-history:28010}" "${WAIT_FOR_BH_TIMEOUT_SECS:-120}"
wait_url "${BTC_RPC_URL:-http://btc-node:8332}" "${WAIT_FOR_BTC_TIMEOUT_SECS:-120}"

if [[ "${INSCRIPTION_SOURCE:-bitcoind}" == "ord" ]]; then
  wait_url "${ORD_RPC_URL:-http://ord-server:28030}" "${WAIT_FOR_ORD_TIMEOUT_SECS:-120}"
fi

if [[ -n "${USDB_INDEXER_EXTRA_ARGS:-}" ]]; then
  # shellcheck disable=SC2086
  exec usdb-indexer --root-dir "${root_dir}" ${USDB_INDEXER_EXTRA_ARGS}
fi

exec usdb-indexer --root-dir "${root_dir}"
