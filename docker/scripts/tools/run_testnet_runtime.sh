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
  managed-start-snapshot <minimum-tip-height> [anchor-height btc-block-hash]
  managed-start-data <minimum-tip-height> [anchor-height btc-block-hash]
                 Start one managed stage without waiting for a snapshot import.
  quiesce-data   Gracefully stop dependent services for a managed resource transition.
  container-ids  Print this project's container IDs for resource inspection.
  data-status    Print the current balance-history readiness response.
  wait-data-origin [height] [timeout]
                 Wait for query-ready balance-history state at the USDB origin.
  wait-data      Wait for balance-history consensus readiness; timeout is the first argument.
  up-indexer [height]
                 Start usdb-indexer after balance-history commits the USDB origin.
  install-registry
                 Start or retry the release-approved optional registry installer.
  wait-indexer   Wait for usdb-indexer consensus readiness; timeout is the first argument.
  up-chain       Recheck all final readiness gates, then start the USDB chain.
  stop-chain     Gracefully stop only chain and disable its automatic restart.
  recreate-chain Apply a checked role change to chain with --no-deps.
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
  local family
  local -a transport_files=()
  family="$(node_env_value USDB_P2P_IP_FAMILY)"
  case "${family:-ipv4}" in
    ipv4) ;;
    ipv6|dual)
      transport_files+=(-f "${docker_dir}/compose.p2p-dual.yml")
      if [[ "${family}" == "ipv6" ]]; then
        transport_files+=(-f "${docker_dir}/compose.p2p-ipv6.yml")
      fi
      ;;
    *) echo "Invalid USDB_P2P_IP_FAMILY=${family}" >&2; return 1 ;;
  esac
  docker compose \
    --project-name "${project_name}" \
    --env-file "${bundle_dir}/network.env" \
    --env-file "${node_env}" \
    -f "${docker_dir}/compose.runtime.yml" \
    -f "${bundle_dir}/compose.network.yml" \
    "${transport_files[@]}" \
    "$@"
}

check_p2p_transport() {
  if [[ "$(node_env_value USDB_P2P_IP_FAMILY)" == "ipv6" || "$(node_env_value USDB_P2P_IP_FAMILY)" == "dual" ]]; then
    python3 "${script_dir}/usdb_p2p.py" check --node-env "${node_env}"
  fi
}

restore_runtime_restart_policy() {
  local service container_id
  for service in "$@"; do
    container_id="$(compose ps --all --quiet "${service}")"
    if [[ -n "${container_id}" ]]; then
      docker update --restart=unless-stopped "${container_id}" >/dev/null
    fi
  done
}

quiesce_runtime_services() {
  local service container_id state elapsed
  # Stop consumers first. A controller interruption leaves restart disabled so
  # Docker cannot race the durable resource transition on its own.
  for service in "$@"; do
    container_id="$(compose ps --all --quiet "${service}")"
    [[ -n "${container_id}" ]] || continue
    state="$(docker inspect --format '{{.State.Status}}' "${container_id}")"
    if [[ "${state}" == "paused" ]]; then
      echo "Cannot quiesce paused service ${service}; operator intervention is required" >&2
      return 1
    fi
    if [[ "${state}" == "running" || "${state}" == "restarting" ]]; then
      docker update --restart=no "${container_id}" >/dev/null
      state="$(docker inspect --format '{{.State.Status}}' "${container_id}")"
      if [[ "${state}" == "running" ]]; then
        docker kill --signal=SIGTERM "${container_id}" >/dev/null
      fi
      elapsed=0
      while true; do
        state="$(docker inspect --format '{{.State.Status}}' "${container_id}")"
        [[ "${state}" == "running" || "${state}" == "restarting" ]] || break
        sleep 1
        elapsed=$((elapsed + 1))
        if ((elapsed % 15 == 0)); then
          echo "Resource transition: waiting for ${service} to flush and stop (${elapsed}s)" >&2
        fi
      done
    fi
  done
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
    if [[ "$(node_env_value USDB_RESOURCE_MODE)" == "auto" ]]; then
      echo "Automatic resources require usdb-node up to coordinate the data-start transition" >&2
      exit 1
    fi
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
    compose up -d snapshot-loader balance-history script-registry-installer
    ;;
  managed-start-snapshot|managed-start-data)
    require_node_env
    validate_bundle --node-env "${node_env}" --require-runtime --require-bitcoin-runtime
    if [[ "$(node_env_value USDB_RESOURCE_MODE)" != "auto" || "$(node_env_value USDB_RESOURCE_PHASE)" == "bitcoin" ]]; then
      echo "Managed data startup requires a committed overlap or steady resource plan" >&2
      exit 1
    fi
    USDB_TESTNET_BUNDLE_DIR="${bundle_dir}" USDB_TESTNET_NODE_ENV="${node_env}" \
      BTC_READY_WAIT_TIMEOUT_SECS=0 "${bitcoin_runner}" wait-data "$@"
    if [[ "${action}" == "managed-start-snapshot" ]]; then
      compose up -d --no-deps snapshot-loader
    else
      loader_id="$(compose ps --all --quiet snapshot-loader)"
      if [[ -z "${loader_id}" || "$(docker inspect --format '{{.State.Status}}:{{.State.ExitCode}}' "${loader_id}")" != "exited:0" ]]; then
        echo "Snapshot loader must finish successfully before balance-history starts" >&2
        exit 1
      fi
      compose up -d --no-deps balance-history script-registry-installer
      restore_runtime_restart_policy balance-history
    fi
    ;;
  quiesce-data)
    require_node_env
    quiesce_runtime_services usdb-control-plane usdb-chain usdb-indexer balance-history
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
    compose up -d --no-deps usdb-indexer
    restore_runtime_restart_policy usdb-indexer
    ;;
  install-registry)
    require_node_env
    command -v docker >/dev/null 2>&1 || {
      echo "docker is required" >&2
      exit 1
    }
    validate_bundle --node-env "${node_env}" --require-runtime --require-bitcoin-runtime
    compose config --quiet
    compose up -d --force-recreate script-registry-installer
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
    check_p2p_transport
    command -v docker >/dev/null 2>&1 || {
      echo "docker is required" >&2
      exit 1
    }
    validate_bundle --node-env "${node_env}" --require-runtime --require-bitcoin-runtime
    if [[ "$(node_env_value USDB_NODE_ROLE)" == "miner" ]]; then
      python3 "${script_dir}/usdb_node.py" --node-env "${node_env}" mining validate-start
    fi
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
    if [[ "$(node_env_value USDB_RESOURCE_MODE)" == "auto" ]]; then
      # A retry may find only one chain service running. Release the chain's
      # allocation and database before chain-init reuses the same budget slot.
      quiesce_runtime_services usdb-control-plane usdb-chain
      # Explicitly run both existing gates without asking Compose to recreate
      # a completed snapshot-loader whose old resource allocation has changed.
      for job in usdb-chain-init paired-checkpoint-recovery; do
        compose up -d --no-deps "${job}"
        job_id="$(compose ps --all --quiet "${job}")"
        if [[ -z "${job_id}" || "$(docker wait "${job_id}")" != "0" ]]; then
          echo "Managed chain startup gate failed: ${job}" >&2
          exit 1
        fi
      done
      USDB_TESTNET_BUNDLE_DIR="${bundle_dir}" USDB_TESTNET_NODE_ENV="${node_env}" \
        "${bitcoin_runner}" wait
      check_readiness "$(host_rpc_url BH_BIND_PORT 28010)" balance-history --require-consensus-ready
      check_readiness "$(host_rpc_url USDB_INDEXER_BIND_PORT 28020)" usdb-indexer --require-consensus-ready
      compose up -d --no-deps usdb-chain usdb-control-plane
    else
      compose up -d usdb-chain-init usdb-chain usdb-control-plane
    fi
    restore_runtime_restart_policy usdb-chain usdb-control-plane
    ;;
  stop-chain)
    require_node_env
    quiesce_runtime_services usdb-chain
    ;;
  recreate-chain)
    require_node_env
    check_p2p_transport
    validate_bundle --node-env "${node_env}" --require-runtime --require-bitcoin-runtime
    if [[ "$(node_env_value USDB_NODE_ROLE)" == "miner" ]]; then
      python3 "${script_dir}/usdb_node.py" --node-env "${node_env}" mining validate-start
    fi
    # Wait for the old writer to exit before Compose can create its replacement.
    quiesce_runtime_services usdb-chain
    compose up -d --no-deps --force-recreate usdb-chain
    restore_runtime_restart_policy usdb-chain
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
  container-ids)
    require_node_env
    compose ps --all --quiet
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
