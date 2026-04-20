#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/.." && pwd)"

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/run_sourcedao_bootstrap.sh [prepare|build-images|up|down|reset|ps|logs|state]

This helper wraps the full local SourceDAO bootstrap path on top of:

  - compose.base.yml
  - compose.dev-sim.yml
  - compose.bootstrap.yml

It will:
  1. scaffold docker/local/bootstrap/env/bootstrap.env if missing
  2. scaffold docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json if missing
  3. generate docker/local/bootstrap/manifests/ethw-genesis.json if missing
  4. start the local BTC + ETHW + SourceDAO bootstrap stack

By default, this helper also overrides the bootstrap flow to:
  SOURCE_DAO_BOOTSTRAP_MODE=dev-workspace
  SOURCE_DAO_BOOTSTRAP_SCOPE=full
  SOURCE_DAO_BOOTSTRAP_PREPARE=auto

Actions:
  prepare       Scaffold local files and generate ETHW genesis if needed.
  build-images  Build usdb-services:local and the local ETHW image.
  up            Prepare local files and start the full bootstrap stack in detached mode.
  down          Stop the stack but keep volumes.
  reset         Stop the stack and remove volumes.
  ps            Show compose service status.
  logs          Follow compose logs.
  state         Print sourcedao-bootstrap state and marker files from the last run.
EOF
}

for cmd in docker; do
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "Required command not found: ${cmd}" >&2
    exit 1
  }
done

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"
project_name="${USDB_SOURCE_DAO_BOOTSTRAP_PROJECT_NAME:-usdb-sourcedao-bootstrap}"
export USDB_DOCKER_NETWORK="${USDB_DOCKER_NETWORK:-${project_name}-net}"
export SOURCE_DAO_BOOTSTRAP_MODE="${SOURCE_DAO_BOOTSTRAP_MODE:-dev-workspace}"
export SOURCE_DAO_BOOTSTRAP_SCOPE="${SOURCE_DAO_BOOTSTRAP_SCOPE:-full}"
export SOURCE_DAO_BOOTSTRAP_PREPARE="${SOURCE_DAO_BOOTSTRAP_PREPARE:-auto}"

env_file="${docker_dir}/local/bootstrap/env/bootstrap.env"
example_env_file="${docker_dir}/env/bootstrap.env.example"
env_dir="$(dirname "${env_file}")"

env_get() {
  local key="${1:?key is required}"
  local fallback="${2:-}"
  local value
  if [[ "${!key+x}" == "x" ]]; then
    printf '%s\n' "${!key}"
    return
  fi
  if [[ -f "${env_file}" ]]; then
    value="$(awk -F= -v key="${key}" '$1 == key { sub(/^[^=]+=*/, "", $0); print $0 }' "${env_file}" | tail -n 1)"
  else
    value=""
  fi
  if [[ -n "${value}" ]]; then
    printf '%s\n' "${value}"
  else
    printf '%s\n' "${fallback}"
  fi
}

host_path_from_docker_dir() {
  local path="${1:?path is required}"
  if [[ "${path}" = /* ]]; then
    printf '%s\n' "${path}"
  else
    printf '%s\n' "${docker_dir}/${path#./}"
  fi
}

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.bootstrap.yml" \
    "$@"
}

ensure_ethw_image_exists() {
  local image
  image="$(env_get ETHW_IMAGE usdb-ethw:local)"
  docker image inspect "${image}" >/dev/null 2>&1 || {
    cat <<EOF >&2
Missing ETHW image ${image}

Build it first with:
  docker/scripts/run_sourcedao_bootstrap.sh build-images
EOF
    exit 1
  }
}

init_env_file() {
  if [[ ! -f "${env_file}" ]]; then
    mkdir -p "${env_dir}"
    cp "${example_env_file}" "${env_file}"
    echo "Initialized ${env_file} from ${example_env_file}"
  fi
}

bootstrap_manifests_dir() {
  host_path_from_docker_dir "$(env_get BOOTSTRAP_HOST_DIR ./local/bootstrap/manifests)"
}

source_dao_repo_dir() {
  host_path_from_docker_dir "$(env_get SOURCE_DAO_REPO_HOST_DIR ../../SourceDAO)"
}

go_ethereum_repo_dir() {
  host_path_from_docker_dir "${GO_ETHEREUM_REPO_HOST_DIR:-../../go-ethereum}"
}

ensure_source_dao_config() {
  local manifests_dir
  local source_dao_repo
  local config_file
  local source_template

  manifests_dir="$(bootstrap_manifests_dir)"
  source_dao_repo="$(source_dao_repo_dir)"
  config_file="${manifests_dir}/sourcedao-bootstrap-config.json"
  source_template="${source_dao_repo}/tools/config/usdb-bootstrap-full.example.json"

  mkdir -p "${manifests_dir}"

  if [[ ! -f "${config_file}" ]]; then
    [[ -f "${source_template}" ]] || {
      echo "Missing SourceDAO bootstrap template: ${source_template}" >&2
      exit 1
    }
    cp "${source_template}" "${config_file}"
    echo "Initialized ${config_file} from ${source_template}"
  fi
}

ensure_ethw_genesis() {
  local manifests_dir
  local genesis_file
  local config_file
  local ethw_image

  manifests_dir="$(bootstrap_manifests_dir)"
  genesis_file="${manifests_dir}/ethw-genesis.json"
  config_file="${manifests_dir}/sourcedao-bootstrap-config.json"
  ethw_image="$(env_get ETHW_IMAGE usdb-ethw:local)"

  if [[ -f "${genesis_file}" ]]; then
    return
  fi

  ensure_ethw_image_exists
  [[ -f "${config_file}" ]] || {
    echo "Missing SourceDAO bootstrap config: ${config_file}" >&2
    exit 1
  }

  echo "Generating ${genesis_file} from ${config_file}"
  docker run --rm \
    -v "${manifests_dir}:/workspace/bootstrap:ro" \
    "${ethw_image}" \
    dumpgenesis \
    --usdb \
    --usdb.bootstrap.config /workspace/bootstrap/sourcedao-bootstrap-config.json \
    > "${genesis_file}"
}

prepare_local_inputs() {
  init_env_file
  ensure_source_dao_config
  ensure_ethw_genesis
}

build_images() {
  local ethw_image
  local go_ethereum_repo

  ethw_image="$(env_get ETHW_IMAGE usdb-ethw:local)"
  go_ethereum_repo="$(go_ethereum_repo_dir)"

  [[ -d "${go_ethereum_repo}" ]] || {
    echo "Missing go-ethereum repo: ${go_ethereum_repo}" >&2
    exit 1
  }

  echo "Building ${ethw_image} from ${go_ethereum_repo}"
  docker build -t "${ethw_image}" "${go_ethereum_repo}"

  echo "Building usdb bootstrap stack images"
  compose build
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

action="${1:-up}"
shift || true

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
    compose up -d --build "$@"
    echo
    echo "Stack started with project ${project_name}"
    echo "Suggested next commands:"
    echo "  docker/scripts/run_sourcedao_bootstrap.sh ps"
    echo "  docker/scripts/run_sourcedao_bootstrap.sh logs"
    echo "  docker/scripts/run_sourcedao_bootstrap.sh state"
    echo
    echo "Control console:"
    echo "  http://127.0.0.1:$(env_get CONTROL_PLANE_BIND_PORT 28040)/"
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
  -h|--help|help)
    usage
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
