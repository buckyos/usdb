#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"
tool_cmd="docker/scripts/tools/run_local_runtime.sh"
build_images_cmd="docker/scripts/tools/build_world_sim_images.sh"

usage() {
  cat <<EOF
Usage:
  ${tool_cmd} [up|build-images|down|logs|ps]

This helper starts the current "dev-full" BTC runtime profile:

  - btc-node
  - snapshot-loader
  - balance-history
  - usdb-indexer
  - usdb-control-plane
  - ethw-node
  - ord-server

It reuses docker/local/dev-full/env/dev-full.env and composes:

  - docker/compose.base.yml
  - docker/compose.dev-sim.yml
  - docker/compose.ord.yml

If docker/local/dev-full/env/dev-full.env does not exist yet, this helper will
initialize it from docker/env/dev-full.env.example once. Existing files are
never overwritten.

This is the local "dev-full" runtime profile:

  - not joiner
  - not world-sim
  - not SourceDAO bootstrap

The control console is then available on:

  http://127.0.0.1:28140/
EOF
}

for cmd in docker; do
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "Required command not found: ${cmd}" >&2
    exit 1
  }
done

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"
project_name="${USDB_DEV_FULL_RUNTIME_PROJECT_NAME:-usdb-dev-full-runtime}"

action="${1:-up}"
shift || true

case "${action}" in
  -h|--help|help)
    usage
    exit 0
    ;;
esac

env_file="${docker_dir}/local/dev-full/env/dev-full.env"
example_env_file="${docker_dir}/env/dev-full.env.example"
env_dir="$(dirname "${env_file}")"

if [[ ! -f "${env_file}" ]]; then
  mkdir -p "${env_dir}"
  cp "${example_env_file}" "${env_file}"
  echo "Initialized ${env_file} from ${example_env_file}" >&2
  echo "Review ETHW_IMAGE / ETHW_COMMAND for your local environment before first full-runtime startup." >&2
fi

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.ord.yml" \
    "$@"
}

env_get() {
  local key="${1:?key is required}"
  local fallback="${2:-}"
  local value
  if [[ "${!key+x}" == "x" ]]; then
    printf '%s\n' "${!key}"
    return
  fi
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

Build the packaged ord/runtime helper images first:
  ${tool_cmd} build-images
EOF
    exit 1
  }
}

ensure_image_executable() {
  local image="${1:?image is required}"
  local path="${2:?path is required}"
  ensure_image_exists "${image}"
  docker run --rm --entrypoint /usr/bin/test "${image}" -x "${path}" >/dev/null 2>&1 || {
    cat <<EOF >&2
Image ${image} is stale or incompatible: missing executable ${path}

Rebuild the packaged ord/runtime helper image:
  ${tool_cmd} build-images
EOF
    exit 1
  }
}

ensure_full_runtime_images() {
  local ord_image
  ord_image="$(env_get ORD_IMAGE usdb-world-sim-tools:local)"
  ensure_image_executable "${ord_image}" "/opt/usdb/docker/scripts/entrypoints/start_ord_server.sh"
  ensure_image_executable "${ord_image}" "/opt/ord/bin/ord"
}

case "${action}" in
  up)
    ensure_full_runtime_images
    compose up --build "$@"
    ;;
  build-images)
    "${docker_dir}/scripts/tools/build_world_sim_images.sh" "$@"
    ;;
  ps)
    compose ps "$@"
    ;;
  logs)
    compose logs -f "$@"
    ;;
  down)
    compose down --remove-orphans "$@"
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
