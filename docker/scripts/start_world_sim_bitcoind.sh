#!/usr/bin/env bash
set -euo pipefail

bitcoin_bin_dir="${BITCOIN_BIN_DIR:-/opt/bitcoin/bin}"
bitcoind="${bitcoin_bin_dir}/bitcoind"
btc_node_data_dir="${BTC_NODE_DATA_DIR:-/home/bitcoin/.bitcoin}"
btc_rpc_port="${BTC_RPC_PORT:-28132}"
btc_p2p_port="${BTC_P2P_PORT:-28133}"
btc_fallback_fee="${BTC_FALLBACK_FEE:-0.0002}"
btc_rpc_user="${BTC_RPC_USER:-}"
btc_rpc_password="${BTC_RPC_PASSWORD:-}"

[[ -x "${bitcoind}" ]] || {
  echo "Missing executable bitcoind: ${bitcoind}" >&2
  exit 1
}

mkdir -p "${btc_node_data_dir}"

auth_args=()
if [[ -n "${btc_rpc_user}" || -n "${btc_rpc_password}" ]]; then
  : "${btc_rpc_user:?BTC_RPC_USER is required when explicit RPC auth is enabled}"
  : "${btc_rpc_password:?BTC_RPC_PASSWORD is required when explicit RPC auth is enabled}"
  auth_args+=("-rpcuser=${btc_rpc_user}" "-rpcpassword=${btc_rpc_password}")
fi

exec "${bitcoind}" \
  -regtest=1 \
  -printtoconsole \
  -server=1 \
  -txindex=1 \
  -rpcbind=0.0.0.0 \
  -rpcallowip=0.0.0.0/0 \
  -rpcport="${btc_rpc_port}" \
  -port="${btc_p2p_port}" \
  -fallbackfee="${btc_fallback_fee}" \
  -datadir="${btc_node_data_dir}" \
  "${auth_args[@]}"
