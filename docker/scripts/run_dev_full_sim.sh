#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/.." && pwd)"

env_file="${docker_dir}/local/dev-full-sim/env/dev-full-sim.env"
example_env_file="${docker_dir}/env/dev-full-sim.env.example"

source "${script_dir}/bootstrap_local_inputs_common.sh"

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/run_dev_full_sim.sh [prepare|build-images|up|down|reset|ps|logs|state]

This helper starts the current development "full-sim" profile on top of:

  - compose.base.yml
  - compose.dev-sim.yml
  - compose.ord.yml
  - compose.bootstrap.yml
  - compose.world-sim.yml

It combines:

  - dev-sim BTC runtime
  - ord-server
  - ETHW node
  - SourceDAO bootstrap
  - BTC world-sim

If docker/local/dev-full-sim/env/dev-full-sim.env does not exist, this helper
initializes it from docker/env/dev-full-sim.env.example once. It will also
scaffold ETHW chain bootstrap config, SourceDAO bootstrap config, and ETHW genesis inputs under:

  docker/local/dev-full-sim/bootstrap/manifests/

Actions:
  prepare       Scaffold local env and bootstrap inputs.
  build-images  Build packaged world-sim images and the local ETHW image.
  up            Prepare inputs and start the full development simulation stack.
  down          Stop the stack but keep Docker volumes.
  reset         Stop the stack and remove Docker volumes.
  ps            Show compose service status.
  logs          Follow compose logs.
  state         Print SourceDAO bootstrap state files from the current stack.
EOF
}

for cmd in docker; do
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "Required command not found: ${cmd}" >&2
    exit 1
  }
done

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"
project_name="${USDB_DEV_FULL_SIM_PROJECT_NAME:-usdb-dev-full-sim}"
export USDB_DOCKER_NETWORK="${USDB_DOCKER_NETWORK:-${project_name}-net}"
export SOURCE_DAO_BOOTSTRAP_MODE="${SOURCE_DAO_BOOTSTRAP_MODE:-dev-workspace}"
export SOURCE_DAO_BOOTSTRAP_SCOPE="${SOURCE_DAO_BOOTSTRAP_SCOPE:-full}"
export SOURCE_DAO_BOOTSTRAP_PREPARE="${SOURCE_DAO_BOOTSTRAP_PREPARE:-auto}"
export ETHW_SIM_PROTOCOL_ALIGNMENT="${ETHW_SIM_PROTOCOL_ALIGNMENT:-1}"

action="${1:-up}"
shift || true

case "${action}" in
  -h|--help|help)
    usage
    exit 0
    ;;
esac

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.ord.yml" \
    -f "${docker_dir}/compose.bootstrap.yml" \
    -f "${docker_dir}/compose.world-sim.yml" \
    "$@"
}

ensure_image_exists() {
  local image="${1:?image is required}"
  docker image inspect "${image}" >/dev/null 2>&1 || {
    cat <<EOF >&2
Missing image ${image}

Build the packaged dependency images first:
  docker/scripts/run_dev_full_sim.sh build-images
EOF
    exit 1
  }
}

ensure_dev_full_sim_images() {
  ensure_image_exists "$(env_get WORLD_SIM_BITCOIN_IMAGE usdb-bitcoin28-regtest:local)"
  ensure_image_exists "$(env_get WORLD_SIM_TOOLS_IMAGE usdb-world-sim-tools:local)"
  ensure_image_exists "$(env_get ORD_IMAGE usdb-world-sim-tools:local)"
  ensure_image_exists "$(env_get ETHW_IMAGE usdb-ethw:local)"
}

state_mode() {
  env_get WORLD_SIM_STATE_MODE persistent
}

identity_seed() {
  env_get WORLD_SIM_IDENTITY_SEED ""
}

ethw_identity_mode() {
  env_get ETHW_IDENTITY_MODE deterministic-seed
}

ensure_ethw_identity_defaults() {
  if [[ "$(ethw_identity_mode)" != "deterministic-seed" ]]; then
    return
  fi

  export WORLD_SIM_IDENTITY_SEED="${WORLD_SIM_IDENTITY_SEED:-$(env_get WORLD_SIM_IDENTITY_SEED alpha-seed-1)}"
  export ETHW_IDENTITY_SEED="${ETHW_IDENTITY_SEED:-$(env_get ETHW_IDENTITY_SEED "${WORLD_SIM_IDENTITY_SEED}")}"
}

prepare_state_mode() {
  local mode
  mode="$(state_mode)"
  case "${mode}" in
    persistent)
      ;;
    reset)
      echo "WORLD_SIM_STATE_MODE=reset -> clearing Docker volumes before startup"
      compose down -v --remove-orphans >/dev/null 2>&1 || true
      ;;
    seeded-reset)
      if [[ -z "$(identity_seed)" ]]; then
        echo "WORLD_SIM_STATE_MODE=seeded-reset requires WORLD_SIM_IDENTITY_SEED" >&2
        exit 1
      fi
      echo "WORLD_SIM_STATE_MODE=seeded-reset -> clearing Docker volumes before startup"
      echo "Current implementation will deterministically recreate ord wallet identities and ETHW miner identity from WORLD_SIM_IDENTITY_SEED."
      compose down -v --remove-orphans >/dev/null 2>&1 || true
      ;;
    *)
      echo "Unsupported WORLD_SIM_STATE_MODE=${mode}" >&2
      exit 1
      ;;
  esac
}

build_images() {
  WORLD_SIM_BITCOIN_IMAGE="$(env_get WORLD_SIM_BITCOIN_IMAGE usdb-bitcoin28-regtest:local)" \
  WORLD_SIM_TOOLS_IMAGE="$(env_get WORLD_SIM_TOOLS_IMAGE usdb-world-sim-tools:local)" \
  ORD_IMAGE="$(env_get ORD_IMAGE usdb-world-sim-tools:local)" \
  WORLD_SIM_RELEASE_ORD_SOURCE="$(env_get WORLD_SIM_RELEASE_ORD_SOURCE git-tag)" \
  WORLD_SIM_RELEASE_ORD_VERSION="$(env_get WORLD_SIM_RELEASE_ORD_VERSION 0.23.3)" \
  "${docker_dir}/scripts/build_world_sim_release_images.sh"

  build_ethw_image
}

show_state() {
  local container_id

  container_id="$(compose ps -q sourcedao-bootstrap | tail -n 1)"
  if [[ -z "${container_id}" ]]; then
    echo "No sourcedao-bootstrap container found for project ${project_name}" >&2
    exit 1
  fi

  echo "--- sourcedao-bootstrap-state.json ---"
  docker cp "${container_id}:/bootstrap/sourcedao-bootstrap-state.json" - 2>/dev/null || true
  echo
  echo "--- sourcedao-bootstrap.done.json ---"
  docker cp "${container_id}:/bootstrap/sourcedao-bootstrap.done.json" - 2>/dev/null || true
}

case "${action}" in
  prepare)
    prepare_local_inputs
    ;;
  build-images)
    init_env_file
    build_images
    ;;
  up)
    prepare_local_inputs
    ensure_dev_full_sim_images
    ensure_ethw_identity_defaults
    prepare_state_mode
    compose up --build "$@"
    ;;
  down)
    compose down --remove-orphans "$@"
    ;;
  reset)
    compose down -v --remove-orphans "$@"
    ;;
  ps)
    compose ps "$@"
    ;;
  logs)
    compose logs -f "$@"
    ;;
  state)
    show_state
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
