#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/run_console_preview.sh [up|down|logs|ps]

This helper keeps the standard dev-sim environment file but only starts the
minimum service subset needed for the local control console:

  - btc-node
  - snapshot-loader
  - balance-history
  - usdb-indexer
  - usdb-control-plane

The control console is then available on:

  http://127.0.0.1:28140/

If docker/local/dev-sim/env/dev-sim.env does not exist yet, this helper will
create the parent directory and copy it from docker/env/dev-sim.env.example
once. Existing files are never overwritten.
EOF
}

for cmd in docker; do
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "Required command not found: ${cmd}" >&2
    exit 1
  }
done

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"
project_name="${USDB_CONSOLE_PREVIEW_PROJECT_NAME:-usdb-console-preview}"

action="${1:-up}"
env_file="${docker_dir}/local/dev-sim/env/dev-sim.env"
example_env_file="${docker_dir}/env/dev-sim.env.example"
env_dir="$(dirname "${env_file}")"

if [[ ! -f "${env_file}" ]]; then
  mkdir -p "${env_dir}"
  cp "${example_env_file}" "${env_file}"
  echo "Initialized ${env_file} from ${example_env_file}" >&2
  echo "Review ETHW_IMAGE / ETHW_COMMAND later if you want the full dev-sim stack." >&2
fi

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    "$@"
}

services=(
  btc-node
  snapshot-loader
  balance-history
  usdb-indexer
  usdb-control-plane
)

case "${action}" in
  up)
    compose up --build "${services[@]}"
    ;;
  down)
    compose down --remove-orphans
    ;;
  logs)
    compose logs -f "${services[@]}"
    ;;
  ps)
    compose ps "${services[@]}"
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
