#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
core_tool="${script_dir}/run_local_world_sim.sh"
tool_cmd="docker/scripts/tools/run_local_world_sim_ethw.sh"

usage() {
  cat <<EOF
Usage:
  ${tool_cmd} up
  ${tool_cmd} build-images
  ${tool_cmd} doctor
  ${tool_cmd} ps
  ${tool_cmd} logs
  ${tool_cmd} down
  ${tool_cmd} reset

This helper starts the ETHW-aligned local world-sim profile. It shares the same
compose project and env file as run_local_world_sim.sh, but includes ethw-node
and enables ETHW protocol alignment by default.

Options for up:
  --foreground  Follow compose logs after startup diagnostics succeed.
  -d, --detach  Explicitly keep detached mode (the default for this helper).
EOF
}

action="${1:-up}"
shift || true

case "${action}" in
  up)
    exec "${core_tool}" up-ethw "$@"
    ;;
  build-images|doctor|ps|logs|down|reset)
    exec "${core_tool}" "${action}" "$@"
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
