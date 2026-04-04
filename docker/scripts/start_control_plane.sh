#!/usr/bin/env bash
set -euo pipefail

root_dir="${CONTROL_PLANE_ROOT_DIR:-/data/usdb-control-plane}"
config_path="${root_dir}/config.toml"

/opt/usdb/docker/scripts/render_control_plane_config.sh "${config_path}"

if [[ -n "${CONTROL_PLANE_EXTRA_ARGS:-}" ]]; then
  # shellcheck disable=SC2086
  exec usdb-control-plane --root-dir "${root_dir}" --skip-process-lock ${CONTROL_PLANE_EXTRA_ARGS}
fi

exec usdb-control-plane --root-dir "${root_dir}" --skip-process-lock
