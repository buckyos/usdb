#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="${CONTROL_PLANE_ROOT_DIR:-/data/usdb-control-plane}"
config_path="${root_dir}/config.toml"

"${script_dir}/../helpers/render_control_plane_config.sh" "${config_path}"

if [[ -n "${CONTROL_PLANE_EXTRA_ARGS:-}" ]]; then
  # shellcheck disable=SC2086
  exec usdb-control-plane --root-dir "${root_dir}" --skip-process-lock ${CONTROL_PLANE_EXTRA_ARGS}
fi

exec usdb-control-plane --root-dir "${root_dir}" --skip-process-lock
