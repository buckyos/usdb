#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ord_bin="${ORD_BIN:-/opt/ord/bin/ord}"
bitcoin_bin_dir="${BITCOIN_BIN_DIR:-/opt/bitcoin/bin}"
bitcoin_cli="${bitcoin_bin_dir}/bitcoin-cli"
btc_rpc_url="${BTC_RPC_URL:-http://btc-node:28132}"
btc_target="${btc_rpc_url#*://}"
btc_host="${btc_target%%:*}"
btc_port="${btc_target##*:}"
btc_data_dir="${BTC_DATA_DIR:-/data/bitcoind}"
btc_auth_mode="${BTC_AUTH_MODE:-cookie}"
btc_rpc_user="${BTC_RPC_USER:-}"
btc_rpc_password="${BTC_RPC_PASSWORD:-}"
cookie_file="${BTC_COOKIE_FILE:-${btc_data_dir}/regtest/.cookie}"
ord_data_dir="${ORD_DATA_DIR:-/data/ord}"
ord_server_port="${ORD_SERVER_PORT:-28130}"

require_file() {
  local path="${1:?path is required}"
  local label="${2:?label is required}"
  [[ -e "${path}" ]] || {
    echo "Missing ${label}: ${path}" >&2
    exit 1
  }
}

require_executable() {
  local path="${1:?path is required}"
  local label="${2:?label is required}"
  [[ -x "${path}" ]] || {
    echo "Missing executable ${label}: ${path}" >&2
    exit 1
  }
}

wait_for_bitcoin() {
  local timeout_secs="${WAIT_FOR_BTC_TIMEOUT_SECS:-120}"
  local start_ts now
  start_ts="$(date +%s)"
  while true; do
    if btc_cli getblockcount >/dev/null 2>&1; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > timeout_secs )); then
      echo "Timed out waiting for bitcoind RPC at ${btc_rpc_url}" >&2
      exit 1
    fi
    sleep 1
  done
}

btc_cli() {
  local args=(
    -regtest
    -datadir="${btc_data_dir}"
    -rpcconnect="${btc_host}"
    -rpcport="${btc_port}"
  )
  case "${btc_auth_mode}" in
    cookie)
      args+=(-rpccookiefile="${cookie_file}")
      ;;
    userpass)
      : "${btc_rpc_user:?BTC_RPC_USER is required when BTC_AUTH_MODE=userpass}"
      : "${btc_rpc_password:?BTC_RPC_PASSWORD is required when BTC_AUTH_MODE=userpass}"
      args+=(-rpcuser="${btc_rpc_user}" -rpcpassword="${btc_rpc_password}")
      ;;
    *)
      echo "Unsupported BTC_AUTH_MODE=${btc_auth_mode}" >&2
      exit 1
      ;;
  esac
  "${bitcoin_cli}" "${args[@]}" "$@"
}

require_executable "${ord_bin}" "ord binary"
require_executable "${bitcoin_cli}" "bitcoin-cli"
if [[ "${btc_auth_mode}" == "cookie" ]]; then
  require_file "${cookie_file}" "Bitcoin cookie file"
fi

mkdir -p "${ord_data_dir}"

"${script_dir}/../helpers/wait_for_tcp.sh" "${btc_host}" "${btc_port}" "${WAIT_FOR_BTC_TIMEOUT_SECS:-120}"
wait_for_bitcoin
ord_auth_args=()
case "${btc_auth_mode}" in
  cookie)
    ord_auth_args+=(--cookie-file "${cookie_file}")
    ;;
  userpass)
    ord_auth_args+=(--bitcoin-rpc-username "${btc_rpc_user}" --bitcoin-rpc-password "${btc_rpc_password}")
    ;;
esac

exec "${ord_bin}" \
  --regtest \
  --bitcoin-rpc-url "${btc_rpc_url}" \
  "${ord_auth_args[@]}" \
  --bitcoin-data-dir "${btc_data_dir}" \
  --data-dir "${ord_data_dir}" \
  --index-addresses \
  --index-transactions \
  server \
  --address 0.0.0.0 \
  --http \
  --http-port "${ord_server_port}" \
  ${ORD_SERVER_EXTRA_ARGS:-}
