#!/usr/bin/env bash
set -euo pipefail

data_dir="${BTC_NODE_DATA_DIR:-/data/bitcoin}"
rpcauth_file="${BTC_RPCAUTH_FILE:-/run/secrets/bitcoin-rpcauth}"
network="${BTC_NETWORK:-bitcoin}"

if [[ "${network}" != "bitcoin" ]]; then
  echo "Release Bitcoin Core image only supports BTC_NETWORK=bitcoin" >&2
  exit 1
fi
if [[ ! -r "${rpcauth_file}" ]]; then
  echo "Bitcoin RPC auth file is not readable: ${rpcauth_file}" >&2
  exit 1
fi

rpcauth="$(tr -d '\r\n' <"${rpcauth_file}")"
if [[ ! "${rpcauth}" =~ ^[A-Za-z0-9._-]+:[0-9a-fA-F]{32}\$[0-9a-fA-F]{64}$ ]]; then
  echo "Bitcoin RPC auth file must contain exactly one rpcauth value" >&2
  exit 1
fi

install -d -m 0700 "${data_dir}"

echo "Starting Bitcoin Core: resource_profile=${BTC_RESOURCE_PROFILE:-balanced-32g}, dbcache_mib=${BTC_DBCACHE_MB:-4096}, txindex=1"

args=(
  "-chain=main"
  "-datadir=${data_dir}"
  "-printtoconsole=1"
  "-server=1"
  "-listen=1"
  "-txindex=1"
  "-prune=0"
  "-rpcbind=0.0.0.0"
  "-rpcallowip=0.0.0.0/0"
  "-rpcport=8332"
  "-port=8333"
  "-rpcauth=${rpcauth}"
  "-dbcache=${BTC_DBCACHE_MB:-4096}"
  "-disablewallet=${BTC_DISABLE_WALLET:-1}"
)

if [[ -n "${BTC_EXTRA_ARGS:-}" ]]; then
  # Operator-only escape hatch. Quoting inside BTC_EXTRA_ARGS is not interpreted.
  read -r -a extra_args <<<"${BTC_EXTRA_ARGS}"
  args+=("${extra_args[@]}")
fi

exec /opt/bitcoin/bin/bitcoind "${args[@]}"
