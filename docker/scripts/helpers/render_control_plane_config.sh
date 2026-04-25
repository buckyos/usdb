#!/usr/bin/env bash
set -euo pipefail

output_path="${1:-${CONTROL_PLANE_ROOT_DIR:-/data/usdb-control-plane}/config.toml}"
root_dir="${CONTROL_PLANE_ROOT_DIR:-/data/usdb-control-plane}"

mkdir -p "${root_dir}" "$(dirname "${output_path}")"

bitcoin_auth_block=""
case "${BTC_AUTH_MODE:-none}" in
  cookie)
    if [[ -n "${BTC_COOKIE_FILE:-}" ]]; then
      bitcoin_auth_block=$'\n[bitcoin]\n'
      bitcoin_auth_block+="url = \"${BTC_RPC_URL:-http://btc-node:8332}\"\n"
      bitcoin_auth_block+="auth_mode = \"cookie\"\n"
      bitcoin_auth_block+="cookie_file = \"${BTC_COOKIE_FILE}\"\n"
    else
      echo "BTC_AUTH_MODE=cookie requires BTC_COOKIE_FILE for control plane" >&2
      exit 1
    fi
    ;;
  userpass)
    : "${BTC_RPC_USER:?BTC_RPC_USER is required when BTC_AUTH_MODE=userpass}"
    : "${BTC_RPC_PASSWORD:?BTC_RPC_PASSWORD is required when BTC_AUTH_MODE=userpass}"
    bitcoin_auth_block=$'\n[bitcoin]\n'
    bitcoin_auth_block+="url = \"${BTC_RPC_URL:-http://btc-node:8332}\"\n"
    bitcoin_auth_block+="auth_mode = \"userpass\"\n"
    bitcoin_auth_block+="rpc_user = \"${BTC_RPC_USER}\"\n"
    bitcoin_auth_block+="rpc_password = \"${BTC_RPC_PASSWORD}\"\n"
    ;;
  none)
    bitcoin_auth_block=$'\n[bitcoin]\n'
    bitcoin_auth_block+="url = \"${BTC_RPC_URL:-http://btc-node:8332}\"\n"
    bitcoin_auth_block+="auth_mode = \"none\"\n"
    ;;
  *)
    echo "Unsupported BTC_AUTH_MODE=${BTC_AUTH_MODE:-}" >&2
    exit 1
    ;;
esac

cat >"${output_path}" <<EOF
root_dir = "${root_dir}"

[server]
host = "${CONTROL_PLANE_HOST:-0.0.0.0}"
port = ${CONTROL_PLANE_PORT:-28040}

[rpc]
balance_history_url = "${BALANCE_HISTORY_RPC_URL:-http://balance-history:28010}"
usdb_indexer_url = "${USDB_INDEXER_RPC_URL:-http://usdb-indexer:28020}"
ethw_url = "${ETHW_RPC_URL:-http://ethw-node:8545}"
ord_url = "${ORD_RPC_URL:-http://ord-server:${ORD_SERVER_PORT:-28030}}"
EOF

printf "%b\n" "${bitcoin_auth_block}" >>"${output_path}"

cat >>"${output_path}" <<EOF

[bootstrap]
bootstrap_manifest = "${CONTROL_PLANE_BOOTSTRAP_MANIFEST:-/bootstrap/bootstrap-manifest.json}"
snapshot_marker = "${CONTROL_PLANE_SNAPSHOT_MARKER:-/data/balance-history/bootstrap/snapshot-loader.done.json}"
ethw_init_marker = "${CONTROL_PLANE_ETHW_INIT_MARKER:-/data/ethw/bootstrap/ethw-init.done.json}"
ethw_identity_marker = "${CONTROL_PLANE_ETHW_IDENTITY_MARKER:-/data/ethw/bootstrap/ethw-sim-identity.json}"
ethw_genesis = "${CONTROL_PLANE_ETHW_GENESIS:-/bootstrap/ethw-genesis.json}"
sourcedao_bootstrap_state = "${CONTROL_PLANE_SOURCEDAO_STATE:-/bootstrap/sourcedao-bootstrap-state.json}"
sourcedao_bootstrap_marker = "${CONTROL_PLANE_SOURCEDAO_MARKER:-/bootstrap/sourcedao-bootstrap.done.json}"
world_sim_bootstrap_marker = "${CONTROL_PLANE_WORLD_SIM_MARKER:-/data/world-sim/bootstrap/world-sim-bootstrap.done.json}"

[web]
console_root = "${CONTROL_PLANE_CONSOLE_ROOT:-/opt/usdb/web/usdb-console-app/dist}"
balance_history_explorer_root = "${CONTROL_PLANE_BH_EXPLORER_ROOT:-/opt/usdb/web/balance-history-browser/dist}"
usdb_indexer_explorer_root = "${CONTROL_PLANE_INDEXER_EXPLORER_ROOT:-/opt/usdb/web/usdb-indexer-browser/dist}"
sourcedao_web_url = "${CONTROL_PLANE_SOURCEDAO_WEB_URL:-http://127.0.0.1:3050}"

[development_mint]
ord_bin = "${CONTROL_PLANE_ORD_BIN:-/opt/ord/bin/ord}"
ord_data_dir = "${CONTROL_PLANE_ORD_DATA_DIR:-/data/ord}"
ord_fee_rate = ${CONTROL_PLANE_ORD_FEE_RATE:-1.0}
EOF
