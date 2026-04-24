#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"
tool_cmd="docker/scripts/tools/run_local_bootstrap.sh"
source "${script_dir}/../helpers/bootstrap_local_inputs_common.sh"

usage() {
  cat <<EOF
Usage:
  ${tool_cmd} [prepare|build-images|up|down|reset|ps|logs|state]

This helper wraps the full local SourceDAO bootstrap path on top of:

  - compose.base.yml
  - compose.dev-sim.yml
  - compose.bootstrap.yml

It will:
  1. scaffold docker/local/bootstrap/env/bootstrap.env if missing
  2. scaffold docker/local/bootstrap/manifests/ethw-bootstrap-config.json if missing
  3. scaffold docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json if missing
  4. generate docker/local/bootstrap/manifests/ethw-genesis.json if needed
  5. start the local BTC + ETHW + SourceDAO bootstrap stack

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

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.bootstrap.yml" \
    "$@"
}

build_images() {
  build_ethw_image

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
    echo "  ${tool_cmd} ps"
    echo "  ${tool_cmd} logs"
    echo "  ${tool_cmd} state"
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
