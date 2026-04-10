#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/.." && pwd)"

env_dir="${docker_dir}/local/world-sim/env"
env_file="${env_dir}/world-sim.env"
env_example="${docker_dir}/env/world-sim.env.example"

mkdir -p "${env_dir}" "${docker_dir}/local/world-sim/runtime"

if [[ ! -f "${env_file}" ]]; then
  cp "${env_example}" "${env_file}"
  cat <<EOF
Initialized ${env_file} from ${env_example}
Build the packaged images before the first run:
  docker/scripts/run_world_sim.sh build-images
EOF
fi

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"
project_name="${USDB_WORLD_SIM_PROJECT_NAME:-usdb-world-sim}"

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.world-sim.yml" \
    "$@"
}

env_get() {
  local key="${1:?key is required}"
  local fallback="${2:-}"
  local value
  value="$(awk -F= -v key="${key}" '$1 == key { sub(/^[^=]+=*/, "", $0); print $0 }' "${env_file}" | tail -n 1)"
  if [[ -n "${value}" ]]; then
    printf '%s\n' "${value}"
  else
    printf '%s\n' "${fallback}"
  fi
}

ensure_image_exists() {
  local image="${1:?image is required}"
  docker image inspect "${image}" >/dev/null 2>&1 || {
    cat <<EOF >&2
Missing image ${image}

Build the packaged world-sim release images first:
  docker/scripts/run_world_sim.sh build-images
EOF
    exit 1
  }
}

ensure_world_sim_images() {
  ensure_image_exists "$(env_get WORLD_SIM_BITCOIN_IMAGE usdb-bitcoin28-regtest:local)"
  ensure_image_exists "$(env_get WORLD_SIM_TOOLS_IMAGE usdb-world-sim-tools:local)"
}

state_mode() {
  env_get WORLD_SIM_STATE_MODE persistent
}

identity_seed() {
  env_get WORLD_SIM_IDENTITY_SEED ""
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
      echo "Current implementation will deterministically recreate ord wallet identities from WORLD_SIM_IDENTITY_SEED."
      compose down -v --remove-orphans >/dev/null 2>&1 || true
      ;;
    *)
      echo "Unsupported WORLD_SIM_STATE_MODE=${mode}" >&2
      exit 1
      ;;
  esac
}

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/run_world_sim.sh up
  docker/scripts/run_world_sim.sh up-full
  docker/scripts/run_world_sim.sh build-images
  docker/scripts/run_world_sim.sh ps
  docker/scripts/run_world_sim.sh logs
  docker/scripts/run_world_sim.sh down
  docker/scripts/run_world_sim.sh reset

Modes:
  up       Start the BTC-side local stack plus world-sim, without ethw-node.
  up-full  Start the same stack and include ethw-node as part of full dev-sim.
  down     Stop the stack but keep Docker volumes and world state.
  reset    Stop the stack and remove Docker volumes to reset world state.
EOF
}

action="${1:-up}"
shift || true

case "${action}" in
  up)
    ensure_world_sim_images
    prepare_state_mode
    compose up --build btc-node snapshot-loader balance-history usdb-indexer usdb-control-plane ord-server world-sim-runner "$@"
    ;;
  up-full)
    ensure_world_sim_images
    prepare_state_mode
    compose up --build "$@"
    ;;
  build-images)
    "${docker_dir}/scripts/build_world_sim_release_images.sh" "$@"
    ;;
  ps)
    compose ps "$@"
    ;;
  logs)
    compose logs -f "$@"
    ;;
  down)
    compose down "$@"
    ;;
  reset)
    compose down -v "$@"
    ;;
  *)
    usage
    exit 1
    ;;
esac
