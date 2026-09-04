#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"
bundle_dir="${USDB_TESTNET_BUNDLE_DIR:-${docker_dir}/networks/testnet-v0}"
node_env="${USDB_TESTNET_NODE_ENV:-${bundle_dir}/node.env}"
project_name="${USDB_TESTNET_BITCOIN_PROJECT_NAME:-usdb-testnet-v0-bitcoin}"
validator="${script_dir}/validate_network_bundle.py"

usage() {
  cat <<EOF
Usage:
  docker/scripts/tools/run_testnet_bitcoin.sh <action> [args]

Actions:
  init-rpc-auth [username]
            Create the private rpcauth file and atomically update the private node.env.
  validate  Validate the bundle and Bitcoin node configuration.
  start     Create the shared network and start Bitcoin Core without waiting for full sync.
  up        Start Bitcoin Core, then wait for full consensus-source readiness.
  wait-data <minimum-tip-height> [anchor-height block-hash]
            Wait only for the historical data-start boundary. The optional
            block-hash must match Bitcoin's active chain at anchor-height.
  wait      Wait for mainnet, full sync and txindex readiness.
  progress  Print one machine-readable Bitcoin sync/readiness observation.
  status    Show service state and perform a single readiness check.
  ps        Show service state.
  logs      Follow Bitcoin Core logs.
  pull      Pull the digest-pinned Bitcoin Core image.
  down      Stop Bitcoin Core without deleting its bind-mounted data directory.

Bitcoin Core is a separate Compose project. USDB runtime stop/reset operations do
not stop it and never remove BTC_NODE_DATA_HOST_DIR.
EOF
}

action="${1:-}"
if [[ -z "${action}" || "${action}" == "help" || "${action}" == "--help" || "${action}" == "-h" ]]; then
  usage
  exit 0
fi
shift || true

require_node_env() {
  if [[ ! -f "${node_env}" ]]; then
    echo "Missing node env: ${node_env}" >&2
    echo "Run docker/scripts/tools/run_testnet_runtime.sh init-env first." >&2
    exit 1
  fi
}

env_value() {
  local key="$1"
  local file="$2"
  local line
  line="$(grep -m 1 -E "^${key}=" "${file}" || true)"
  printf '%s' "${line#*=}"
}

network_name() {
  env_value USDB_DOCKER_NETWORK "${bundle_dir}/network.env"
}

ensure_network() {
  local name
  name="$(network_name)"
  if [[ -z "${name}" ]]; then
    echo "USDB_DOCKER_NETWORK is missing from network.env" >&2
    exit 1
  fi
  if ! docker network inspect "${name}" >/dev/null 2>&1; then
    docker network create \
      --label org.usdb.network-bundle=usdb-testnet-v0 \
      "${name}" >/dev/null
    echo "Created shared Docker network ${name}"
  fi
}

validate_node() {
  python3 "${validator}" \
    --bundle-dir "${bundle_dir}" \
    --node-env "${node_env}"
}

validate_bitcoin_runtime() {
  python3 "${validator}" \
    --bundle-dir "${bundle_dir}" \
    --node-env "${node_env}" \
    --require-bitcoin-runtime
}

prepare_data_dir() {
  local data_dir
  data_dir="$(env_value BTC_NODE_DATA_HOST_DIR "${node_env}")"
  if [[ -z "${data_dir}" || "${data_dir}" != /* ]]; then
    echo "BTC_NODE_DATA_HOST_DIR must be an absolute path" >&2
    exit 1
  fi
  install -d -m 0700 "${data_dir}"
}

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${bundle_dir}/network.env" \
    --env-file "${node_env}" \
    -f "${docker_dir}/compose.bitcoin.yml" \
    "$@"
}

timestamp_utc() {
  date -u +'%Y-%m-%dT%H:%M:%SZ'
}

duration_text() {
  local elapsed="${1}"
  printf '%02d:%02d:%02d' \
    "$((elapsed / 3600))" \
    "$(((elapsed % 3600) / 60))" \
    "$((elapsed % 60))"
}

stop_bitcoin() {
  local container_id
  local initial_state
  local current_state
  local started_at
  local finished_at
  local elapsed
  local next_heartbeat=15
  local shutdown_phase
  local final_state
  local exit_code
  local oom_killed
  local unsafe_stop=0

  container_id="$(compose ps --all --quiet btc-node)"
  if [[ -z "${container_id}" ]]; then
    printf '[%s] [usdb-node] Bitcoin Core has no container to stop\n' "$(timestamp_utc)" >&2
    compose down --remove-orphans "$@"
    return
  fi

  initial_state="$(docker inspect --format '{{.State.Status}}' "${container_id}")"
  started_at="$(date +%s)"
  printf '[%s] [usdb-node] Bitcoin Core shutdown started: state=%s, force_kill=disabled\n' \
    "$(timestamp_utc)" \
    "${initial_state}" >&2

  if [[ "${initial_state}" == "running" || "${initial_state}" == "restarting" ]]; then
    # An RPC stop is invisible to Docker's unless-stopped policy. Disable that
    # policy before requesting shutdown so a clean bitcoind exit cannot race a
    # container restart. The next Compose up restores the declared policy.
    if ! docker update --restart=no "${container_id}" >/dev/null; then
      printf '[%s] [usdb-node] ERROR: failed to disable the Bitcoin container restart policy; shutdown was not requested\n' \
        "$(timestamp_utc)" >&2
      return 1
    fi

    if compose exec -T btc-node \
      /opt/bitcoin/bin/bitcoin-cli \
        -chain=main \
        -datadir=/data/bitcoin \
        -rpcclienttimeout=10 \
        stop; then
      printf '[%s] [usdb-node] Bitcoin Core accepted the authenticated RPC stop request\n' \
        "$(timestamp_utc)" >&2
    else
      current_state="$(docker inspect --format '{{.State.Status}}' "${container_id}" 2>/dev/null || true)"
      if [[ "${current_state}" == "running" || "${current_state}" == "restarting" ]]; then
        printf '[%s] [usdb-node] WARNING: Bitcoin RPC stop is unavailable; sending SIGTERM without a forced-kill deadline\n' \
          "$(timestamp_utc)" >&2
        if ! docker kill --signal=SIGTERM "${container_id}" >/dev/null; then
          printf '[%s] [usdb-node] ERROR: failed to signal Bitcoin Core; preserving the running container for diagnosis\n' \
            "$(timestamp_utc)" >&2
          docker update --restart=unless-stopped "${container_id}" >/dev/null 2>&1 || true
          return 1
        fi
      fi
    fi

    while true; do
      current_state="$(docker inspect --format '{{.State.Status}}' "${container_id}" 2>/dev/null || true)"
      if [[ "${current_state}" != "running" && "${current_state}" != "restarting" ]]; then
        break
      fi
      sleep 1
      elapsed="$(( $(date +%s) - started_at ))"
      if ((elapsed >= next_heartbeat)); then
        shutdown_phase="$(
          data_dir="$(env_value BTC_NODE_DATA_HOST_DIR "${node_env}")"
          if [[ -r "${data_dir}/debug.log" ]]; then
            tail -n 256 "${data_dir}/debug.log" 2>/dev/null \
              | grep -E 'Shutdown:|Dumped mempool|Flushed fee estimates|thread exit' \
              | tail -n 1 || true
          fi
        )"
        printf '[%s] [usdb-node] Bitcoin Core shutdown in progress: elapsed=%s, phase=%s\n' \
          "$(timestamp_utc)" \
          "$(duration_text "${elapsed}")" \
          "${shutdown_phase:-waiting for database flush}" >&2
        next_heartbeat="$((next_heartbeat + 15))"
      fi
    done
  fi
  finished_at="$(date +%s)"
  elapsed="$((finished_at - started_at))"

  final_state="$(docker inspect --format '{{.State.Status}}' "${container_id}")"
  exit_code="$(docker inspect --format '{{.State.ExitCode}}' "${container_id}")"
  oom_killed="$(docker inspect --format '{{.State.OOMKilled}}' "${container_id}")"
  printf '[%s] [usdb-node] Bitcoin Core shutdown completed: elapsed=%s, state=%s, exit_code=%s, oom_killed=%s\n' \
    "$(timestamp_utc)" \
    "$(duration_text "${elapsed}")" \
    "${final_state}" \
    "${exit_code}" \
    "${oom_killed}" >&2

  if [[ "${exit_code}" == "137" && "${oom_killed}" != "true" ]]; then
    unsafe_stop=1
    printf '[%s] [usdb-node] ERROR: Bitcoin Core retained evidence of an external forced kill; inspect debug.log before restart\n' \
      "$(timestamp_utc)" >&2
  elif [[ "${exit_code}" != "0" ]]; then
    printf '[%s] [usdb-node] WARNING: Bitcoin Core retained a non-zero exit code; inspect its persistent debug.log before restart\n' \
      "$(timestamp_utc)" >&2
  fi

  compose down --remove-orphans "$@"
  if ((unsafe_stop != 0)); then
    return 1
  fi
}

wait_ready() {
  local timeout_secs
  timeout_secs="${BTC_READY_WAIT_TIMEOUT_SECS:-86400}"
  compose exec -T btc-node \
    python3 /opt/usdb/docker/scripts/tools/check_bitcoin_readiness.py \
      --wait-timeout-secs "${timeout_secs}" \
      --poll-interval-secs "${BTC_READY_POLL_INTERVAL_SECS:-15}" \
      --progress-interval-secs "${BTC_READY_PROGRESS_INTERVAL_SECS:-60}"
}

start_bitcoin() {
  local container_id
  command -v docker >/dev/null 2>&1 || {
    echo "docker is required" >&2
    exit 1
  }
  prepare_data_dir
  validate_bitcoin_runtime
  ensure_network
  compose config --quiet
  compose up -d "$@" btc-node
  container_id="$(compose ps --all --quiet btc-node)"
  if [[ -z "${container_id}" ]]; then
    echo "Bitcoin Core container is missing after Compose up" >&2
    exit 1
  fi
  # This also repairs restart=no left by an interrupted managed shutdown.
  docker update --restart=unless-stopped "${container_id}" >/dev/null
}

wait_data_start() {
  local minimum_tip_height="${1:-}"
  local anchor_height="${2:-}"
  local expected_hash="${3:-}"
  if [[ ! "${minimum_tip_height}" =~ ^[0-9]+$ ]]; then
    echo "wait-data requires a non-negative minimum BTC tip height" >&2
    exit 2
  fi
  if [[ -n "${anchor_height}" || -n "${expected_hash}" ]]; then
    if [[ ! "${anchor_height}" =~ ^[0-9]+$ || -z "${expected_hash}" ]]; then
      echo "wait-data requires anchor-height and block-hash together" >&2
      exit 2
    fi
  fi
  local args=(
    --data-start
    --minimum-height "${minimum_tip_height}"
    --wait-timeout-secs "${BTC_READY_WAIT_TIMEOUT_SECS:-86400}"
    --poll-interval-secs "${BTC_READY_POLL_INTERVAL_SECS:-15}"
    --progress-interval-secs "${BTC_READY_PROGRESS_INTERVAL_SECS:-60}"
  )
  if [[ -n "${expected_hash}" ]]; then
    args+=(--anchor-height "${anchor_height}" --expected-block-hash "${expected_hash}")
  fi
  compose exec -T btc-node \
    python3 /opt/usdb/docker/scripts/tools/check_bitcoin_readiness.py "${args[@]}"
}

case "${action}" in
  init-rpc-auth)
    require_node_env
    rpcauth_file="$(env_value BTC_RPCAUTH_HOST_FILE "${node_env}")"
    if [[ -z "${rpcauth_file}" || "${rpcauth_file}" != /* ]]; then
      echo "BTC_RPCAUTH_HOST_FILE must be an absolute path" >&2
      exit 1
    fi
    python3 "${script_dir}/generate_bitcoin_rpcauth.py" \
      --username "${1:-usdb-testnet}" \
      --output "${rpcauth_file}" \
      --node-env "${node_env}"
    echo "Bitcoin RPC credentials were written directly to the private node env; no password was printed." >&2
    ;;
  validate)
    require_node_env
    validate_node
    ;;
  start)
    require_node_env
    start_bitcoin "$@"
    ;;
  up)
    require_node_env
    start_bitcoin "$@"
    wait_ready
    ;;
  wait-data)
    require_node_env
    validate_bitcoin_runtime
    wait_data_start "${1:-}" "${2:-}" "${3:-}"
    ;;
  wait)
    require_node_env
    validate_bitcoin_runtime
    wait_ready
    ;;
  progress)
    require_node_env
    compose exec -T btc-node \
      python3 /opt/usdb/docker/scripts/tools/check_bitcoin_readiness.py \
        --status-json
    ;;
  status)
    require_node_env
    compose ps
    compose exec -T btc-node \
      python3 /opt/usdb/docker/scripts/tools/check_bitcoin_readiness.py
    ;;
  ps)
    require_node_env
    compose ps "$@"
    ;;
  logs)
    require_node_env
    compose logs -f "$@" btc-node
    ;;
  pull)
    require_node_env
    validate_node
    compose pull "$@" btc-node
    ;;
  down)
    require_node_env
    stop_bitcoin "$@"
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
