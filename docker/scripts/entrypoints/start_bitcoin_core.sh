#!/usr/bin/env bash
set -euo pipefail

data_dir="${BTC_NODE_DATA_DIR:-/data/bitcoin}"
rpcauth_file="${BTC_RPCAUTH_FILE:-/run/secrets/bitcoin-rpcauth}"
network="${BTC_NETWORK:-bitcoin}"
snapshot_mode="${SNAPSHOT_MODE:-none}"
txindex="${BTC_TXINDEX:-1}"
if [[ "${snapshot_mode}" == "assumeutxo" ]]; then
  txindex="${BTC_TXINDEX:-0}"
fi
if [[ "${txindex}" != "0" && "${txindex}" != "1" ]]; then
  echo "BTC_TXINDEX must be 0 or 1" >&2
  exit 1
fi

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

echo "Starting Bitcoin Core: resource_profile=${BTC_RESOURCE_PROFILE:-balanced-32g}, memory_limit=${BTC_MEMORY_LIMIT:-5g}, memory_swap_limit=${BTC_MEMORY_SWAP_LIMIT:-6g}, dbcache_mib=${BTC_DBCACHE_MB:-3072}, txindex=${txindex}, snapshot_mode=${snapshot_mode}, prune=0"

args=(
  "-chain=main"
  "-datadir=${data_dir}"
  "-printtoconsole=1"
  "-server=1"
  "-listen=1"
  "-txindex=${txindex}"
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
  if [[ "${snapshot_mode}" == "assumeutxo" ]]; then
    for arg in "${extra_args[@]}"; do
      key="${arg%%=*}"
      key="${key#-}"
      key="${key#-}"
      key="${key#no}"
      case "${key}" in
        chain|testnet|testnet4|regtest|signet|datadir|blocksdir|prune|txindex|conf|includeconf|reindex|reindex-chainstate|loadblock|rpcport|rpcauth)
          echo "BTC_EXTRA_ARGS cannot override native bootstrap option: ${key}" >&2
          exit 1
          ;;
      esac
    done
  fi
  args+=("${extra_args[@]}")
fi

exec /opt/bitcoin/bin/bitcoind "${args[@]}"
