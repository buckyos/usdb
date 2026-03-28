#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${script_dir}/snapshot_marker.sh"

snapshot_mode="${SNAPSHOT_MODE:-none}"
root_dir="${BH_ROOT_DIR:-/data/balance-history}"
config_path="${root_dir}/config.toml"
db_dir="${root_dir}/db"
marker_path="$(snapshot_marker_path "${root_dir}")"

"${script_dir}/render_balance_history_config.sh" "${config_path}"

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
  snapshot_file="${BH_SNAPSHOT_FILE:-}"
  snapshot_manifest="${BH_SNAPSHOT_MANIFEST:-}"
  if snapshot_marker_matches "${marker_path}" "${snapshot_mode}" "${snapshot_file}" "${snapshot_manifest}"; then
    echo "Existing balance-history DB and matching snapshot marker detected under ${root_dir}; skipping snapshot install"
    exit 0
  fi
  echo "Existing balance-history DB detected under ${db_dir}, but snapshot marker is missing or does not match current snapshot inputs" >&2
  exit 1
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

snapshot_manifest="${BH_SNAPSHOT_MANIFEST:-}"
if [[ -f "${marker_path}" ]]; then
  echo "Removing stale snapshot marker at ${marker_path}" >&2
  rm -f "${marker_path}"
fi

args=(--root-dir "${root_dir}" install-snapshot --file "${snapshot_file}")
if [[ -n "${snapshot_manifest}" ]]; then
  args+=(--manifest "${snapshot_manifest}")
fi

balance-history "${args[@]}"
snapshot_marker_write "${marker_path}" "${snapshot_mode}" "${snapshot_file}" "${snapshot_manifest}"
echo "Snapshot install completed and marker written to ${marker_path}"
