#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"
bundle_dir="${USDB_TESTNET_BUNDLE_DIR:-${docker_dir}/networks/testnet-v0}"
node_env="${USDB_TESTNET_NODE_ENV:-${bundle_dir}/node.env}"
project_name="${USDB_TESTNET_PROJECT_NAME:-usdb-testnet-v0}"
validator="${script_dir}/validate_network_bundle.py"

usage() {
  cat <<EOF
Usage:
  docker/scripts/tools/run_testnet_runtime.sh <action> [args]

Actions:
  init-env       Create the private per-node node.env from the example.
  validate       Validate only the checked-in immutable network bundle.
  validate-node  Validate the bundle and this machine's node.env.
  up             Require images, BTC credentials and signed snapshot; start detached.
  down           Stop containers without deleting named volumes.
  ps             Show service state.
  logs           Follow service logs.
  pull           Pull the explicitly configured release images.

The helper composes docker/compose.runtime.yml with the testnet-v0 network
overlay. It never builds images and never deletes node data.
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
  up)
    require_node_env
    command -v docker >/dev/null 2>&1 || {
      echo "docker is required" >&2
      exit 1
    }
    validate_bundle --node-env "${node_env}" --require-runtime
    compose config --quiet
    compose up -d "$@"
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
