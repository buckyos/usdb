#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"
bundle_dir="${USDB_TESTNET_BUNDLE_DIR:-${docker_dir}/networks/testnet-v0}"
node_env="${USDB_TESTNET_NODE_ENV:-${bundle_dir}/node.env}"
project_name="${USDB_TESTNET_PROJECT_NAME:-usdb-testnet-v0}"
validator="${script_dir}/validate_network_bundle.py"
bitcoin_runner="${script_dir}/run_testnet_bitcoin.sh"
readiness_checker="${script_dir}/check_json_rpc_readiness.py"

usage() {
  cat <<EOF
Usage:
  docker/scripts/tools/run_testnet_runtime.sh <action> [args]

Actions:
  init-env       Create the private per-node node.env from the example.
  validate       Validate only the checked-in immutable network bundle.
  validate-node  Validate the bundle and this machine's node.env.
  up-data <minimum-tip-height> [anchor-height btc-block-hash]
                 Wait for the BTC data-start anchor, then start snapshot-loader
                 and balance-history without waiting for full Bitcoin readiness.
  data-status    Print the current balance-history readiness response.
  wait-data-origin [height] [timeout]
                 Wait for query-ready balance-history state at the USDB origin.
  wait-data      Wait for balance-history consensus readiness; timeout is the first argument.
  up-indexer [height]
                 Start usdb-indexer after balance-history commits the USDB origin.
  wait-indexer   Wait for usdb-indexer consensus readiness; timeout is the first argument.
  up-chain       Recheck all final readiness gates, then start the USDB chain.
  up             Complete all final gates and start indexer/chain for compatibility.
  indexer-status Print the current usdb-indexer readiness response.
  down           Stop containers without deleting bind-mounted node data.
  ps             Show service state.
  logs           Follow service logs.
  pull           Pull the explicitly configured release images.

The helper composes docker/compose.runtime.yml with the testnet-v0 network
overlay. Bitcoin Core has an independent lifecycle managed by
run_testnet_bitcoin.sh. This helper never builds images or deletes node data.
EOF
}

action="${1:-}"
if [[ -z "${action}" || "${action}" == "help" || "${action}" == "--help" || "${action}" == "-h" ]]; then
  usage
  exit 0
fi
shift || true

init_node_env() {
  if [[ -f "${node_env}" ]]; then
    echo "Node env already exists: ${node_env}" >&2
    return
  fi
  mkdir -p "$(dirname "${node_env}")"
  cp "${bundle_dir}/node.env.example" "${node_env}"
  chmod 600 "${node_env}"
  echo "Created ${node_env}; fill image references, BTC credentials and node role before startup."
}

require_node_env() {
  if [[ ! -f "${node_env}" ]]; then
    echo "Missing node env: ${node_env}" >&2
    echo "Run docker/scripts/tools/run_testnet_runtime.sh init-env first." >&2
    exit 1
  fi
}

validate_bundle() {
  python3 "${validator}" --bundle-dir "${bundle_dir}" "$@"
}

node_env_value() {
  local key="$1"
  awk -F= -v key="${key}" '$1 == key { print substr($0, length($1) + 2) }' "${node_env}"
}

host_rpc_url() {
  local port_key="$1"
  local default_port="$2"
  local port
  port="$(node_env_value "${port_key}")"
  printf 'http://127.0.0.1:%s\n' "${port:-${default_port}}"
}

check_readiness() {
  local url="$1"
  local service="$2"
  shift 2
  python3 "${readiness_checker}" \
    --url "${url}" \
    --expected-service "${service}" \
    --progress-interval-secs "${USDB_READINESS_PROGRESS_INTERVAL_SECS:-30}" \
    "$@"
}

compose() {
  export USDB_NETWORK_ARTIFACTS_DIR="${bundle_dir}/artifacts"
  export BH_SNAPSHOT_TRUST_HOST_DIR="${bundle_dir}/trust"
  docker compose \
    --project-name "${project_name}" \
    --env-file "${bundle_dir}/network.env" \
    --env-file "${node_env}" \
    -f "${docker_dir}/compose.runtime.yml" \
    -f "${bundle_dir}/compose.network.yml" \
    "$@"
}

case "${action}" in
  init-env)
    init_node_env
    ;;
  validate)
    validate_bundle
    ;;
  validate-node)
    require_node_env
    validate_bundle --node-env "${node_env}"
    ;;
  up-data)
    require_node_env
    command -v docker >/dev/null 2>&1 || {
      echo "docker is required" >&2
      exit 1
    }
    validate_bundle --node-env "${node_env}" --require-runtime --require-bitcoin-runtime
    btc_minimum_tip_height="${1:-}"
    btc_anchor_height="${2:-}"
    btc_data_hash="${3:-}"
    if [[ ! "${btc_minimum_tip_height}" =~ ^[0-9]+$ ]]; then
      echo "up-data requires the minimum BTC tip height" >&2
      exit 2
    fi
    USDB_TESTNET_BUNDLE_DIR="${bundle_dir}" \
      USDB_TESTNET_NODE_ENV="${node_env}" \
      "${bitcoin_runner}" wait-data \
        "${btc_minimum_tip_height}" "${btc_anchor_height}" "${btc_data_hash}"
    compose config --quiet
    compose up -d snapshot-loader balance-history
    ;;
  data-status)
    require_node_env
    check_readiness "$(host_rpc_url BH_BIND_PORT 28010)" "balance-history"
    ;;
  wait-data)
    require_node_env
    timeout_secs="${1:-86400}"
    check_readiness \
      "$(host_rpc_url BH_BIND_PORT 28010)" \
      "balance-history" \
      --require-consensus-ready \
      --wait-timeout-secs "${timeout_secs}"
    ;;
  wait-data-origin)
    require_node_env
    origin_height="${1:-$(node_env_value USDB_GENESIS_BLOCK_HEIGHT)}"
    timeout_secs="${2:-86400}"
    if [[ ! "${origin_height}" =~ ^[0-9]+$ ]]; then
      echo "wait-data-origin requires a non-negative origin height" >&2
      exit 2
    fi
    check_readiness \
      "$(host_rpc_url BH_BIND_PORT 28010)" \
      "balance-history" \
      --minimum-stable-height "${origin_height}" \
      --wait-timeout-secs "${timeout_secs}"
    ;;
  up-indexer)
    require_node_env
    command -v docker >/dev/null 2>&1 || {
      echo "docker is required" >&2
      exit 1
    }
    validate_bundle --node-env "${node_env}" --require-runtime --require-bitcoin-runtime
    compose config --quiet
    origin_height="${1:-$(node_env_value USDB_GENESIS_BLOCK_HEIGHT)}"
    if [[ ! "${origin_height}" =~ ^[0-9]+$ ]]; then
      echo "up-indexer requires a non-negative origin height" >&2
      exit 2
    fi
    check_readiness \
      "$(host_rpc_url BH_BIND_PORT 28010)" \
      "balance-history" \
      --minimum-stable-height "${origin_height}"
    compose up -d usdb-indexer
    ;;
  wait-indexer)
    require_node_env
    timeout_secs="${1:-${WAIT_FOR_USDB_INDEXER_READY_TIMEOUT_SECS:-1800}}"
    check_readiness \
      "$(host_rpc_url USDB_INDEXER_BIND_PORT 28020)" \
      "usdb-indexer" \
      --require-consensus-ready \
      --wait-timeout-secs "${timeout_secs}"
    ;;
  up-chain)
    require_node_env
    command -v docker >/dev/null 2>&1 || {
      echo "docker is required" >&2
      exit 1
    }
    validate_bundle --node-env "${node_env}" --require-runtime --require-bitcoin-runtime
    USDB_TESTNET_BUNDLE_DIR="${bundle_dir}" \
      USDB_TESTNET_NODE_ENV="${node_env}" \
      "${bitcoin_runner}" wait
    check_readiness \
      "$(host_rpc_url BH_BIND_PORT 28010)" \
      "balance-history" \
      --require-consensus-ready
    check_readiness \
      "$(host_rpc_url USDB_INDEXER_BIND_PORT 28020)" \
      "usdb-indexer" \
      --require-consensus-ready
    compose up -d usdb-chain-init usdb-chain usdb-control-plane
    ;;
  up)
    require_node_env
    origin_height="$(node_env_value USDB_GENESIS_BLOCK_HEIGHT)"
    "${BASH_SOURCE[0]}" up-indexer "${origin_height}"
    "${BASH_SOURCE[0]}" up-chain
    ;;
  indexer-status)
    require_node_env
    check_readiness "$(host_rpc_url USDB_INDEXER_BIND_PORT 28020)" "usdb-indexer"
    ;;
  down)
    require_node_env
    compose down --remove-orphans "$@"
    ;;
  ps)
    require_node_env
    compose ps "$@"
    ;;
  logs)
    require_node_env
    compose logs -f "$@"
    ;;
  pull)
    require_node_env
    validate_bundle --node-env "${node_env}"
    compose pull "$@"
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
