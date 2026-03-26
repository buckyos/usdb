#!/usr/bin/env bash
set -euo pipefail

snapshot_mode="${SNAPSHOT_MODE:-none}"
root_dir="${BH_ROOT_DIR:-/data/balance-history}"
config_path="${root_dir}/config.toml"
db_dir="${root_dir}/db"

/opt/usdb/docker/scripts/render_balance_history_config.sh "${config_path}"

case "${snapshot_mode}" in
  none)
    echo "Snapshot loader disabled (SNAPSHOT_MODE=none)"
    exit 0
    ;;
  balance-history)
    ;;
  *)
    echo "Unsupported SNAPSHOT_MODE=${snapshot_mode}" >&2
    exit 1
    ;;
esac

if [[ -d "${db_dir}" ]] && find "${db_dir}" -mindepth 1 -print -quit | grep -q .; then
  echo "Existing balance-history DB detected under ${db_dir}; skipping snapshot install"
  exit 0
fi

snapshot_file="${BH_SNAPSHOT_FILE:-}"
if [[ -z "${snapshot_file}" ]]; then
  echo "SNAPSHOT_MODE=balance-history requires BH_SNAPSHOT_FILE" >&2
  exit 1
fi

if [[ ! -f "${snapshot_file}" ]]; then
  echo "Snapshot file does not exist: ${snapshot_file}" >&2
  exit 1
fi

args=(--root-dir "${root_dir}" install-snapshot --file "${snapshot_file}")
if [[ -n "${BH_SNAPSHOT_MANIFEST:-}" ]]; then
  args+=(--manifest "${BH_SNAPSHOT_MANIFEST}")
fi

exec balance-history "${args[@]}"
