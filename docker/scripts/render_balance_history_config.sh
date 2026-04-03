#!/usr/bin/env bash
set -euo pipefail

output_path="${1:-${BH_ROOT_DIR:-/data/balance-history}/config.toml}"
root_dir="${BH_ROOT_DIR:-/data/balance-history}"

mkdir -p "${root_dir}" "$(dirname "${output_path}")"

btc_auth_mode="${BTC_AUTH_MODE:-cookie}"
btc_auth_block=""
case "${btc_auth_mode}" in
  cookie)
    if [[ -n "${BTC_COOKIE_FILE:-}" ]]; then
      btc_auth_block=$'\n[btc.auth]\n'
      btc_auth_block+="CookieFile = \"${BTC_COOKIE_FILE}\"\n"
    fi
    ;;
  userpass)
    : "${BTC_RPC_USER:?BTC_RPC_USER is required when BTC_AUTH_MODE=userpass}"
    : "${BTC_RPC_PASSWORD:?BTC_RPC_PASSWORD is required when BTC_AUTH_MODE=userpass}"
    btc_auth_block=$'\n[btc.auth]\n'
    btc_auth_block+="UserPass = [\n"
    btc_auth_block+="    \"${BTC_RPC_USER}\",\n"
    btc_auth_block+="    \"${BTC_RPC_PASSWORD}\",\n"
    btc_auth_block+="]\n"
    ;;
  none)
    btc_auth_block='auth = "None"'
    ;;
  *)
    echo "Unsupported BTC_AUTH_MODE=${btc_auth_mode}" >&2
    exit 1
    ;;
esac

snapshot_extra=""
if [[ -n "${BH_SNAPSHOT_SIGNING_KEY_FILE:-}" ]]; then
  snapshot_extra+="signing_key_file = \"${BH_SNAPSHOT_SIGNING_KEY_FILE}\"\n"
fi
if [[ -n "${BH_SNAPSHOT_TRUSTED_KEYS_FILE:-}" ]]; then
  snapshot_extra+="trusted_keys_file = \"${BH_SNAPSHOT_TRUSTED_KEYS_FILE}\"\n"
fi

cat >"${output_path}" <<EOF
root_dir = "${root_dir}"

[btc]
network = "${BTC_NETWORK:-bitcoin}"
data_dir = "${BTC_DATA_DIR:-/data/bitcoind}"
rpc_url = "${BTC_RPC_URL:-http://btc-node:8332}"
EOF

if [[ -n "${btc_auth_block}" ]]; then
  printf "%b\n" "${btc_auth_block}" >>"${output_path}"
fi

cat >>"${output_path}" <<EOF

[ordinals]
rpc_url = "${ORD_RPC_URL:-http://ord-server:28030}"

[electrs]
rpc_url = "${ELECTRS_RPC_URL:-tcp://electrs:50001}"

[sync]
local_loader_threshold = ${BH_SYNC_LOCAL_LOADER_THRESHOLD:-500}
batch_size = ${BH_SYNC_BATCH_SIZE:-128}
max_sync_block_height = ${BH_SYNC_MAX_SYNC_BLOCK_HEIGHT:-4294967295}

[rpc_server]
host = "${BH_RPC_HOST:-0.0.0.0}"
port = ${BH_RPC_PORT:-28010}

[snapshot]
trust_mode = "${BH_SNAPSHOT_TRUST_MODE:-dev}"
EOF

if [[ -n "${snapshot_extra}" ]]; then
  printf "%b" "${snapshot_extra}" >>"${output_path}"
fi
