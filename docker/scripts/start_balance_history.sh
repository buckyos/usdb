#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${script_dir}/snapshot_marker.sh"

root_dir="${BH_ROOT_DIR:-/data/balance-history}"
config_path="${root_dir}/config.toml"
snapshot_mode="${SNAPSHOT_MODE:-none}"
marker_path="$(snapshot_marker_path "${root_dir}")"

"${script_dir}/render_balance_history_config.sh" "${config_path}"

btc_url="${BTC_RPC_URL:-http://btc-node:8332}"
btc_target="${btc_url#*://}"
btc_host="${btc_target%%:*}"
btc_port="${btc_target##*:}"

"${script_dir}/wait_for_tcp.sh" \
  "${btc_host}" \
  "${btc_port}" \
  "${WAIT_FOR_BTC_TIMEOUT_SECS:-120}"

if [[ "${snapshot_mode}" == "balance-history" ]]; then
  snapshot_file="${BH_SNAPSHOT_FILE:-}"
  snapshot_manifest="${BH_SNAPSHOT_MANIFEST:-}"
  if ! snapshot_marker_matches "${marker_path}" "${snapshot_mode}" "${snapshot_file}" "${snapshot_manifest}"; then
    echo "SNAPSHOT_MODE=balance-history requires a matching snapshot install marker at ${marker_path}" >&2
    exit 1
  fi
elif [[ "${snapshot_mode}" != "none" ]]; then
  echo "Unsupported SNAPSHOT_MODE=${snapshot_mode}" >&2
  exit 1
fi

if [[ -n "${BH_EXTRA_ARGS:-}" ]]; then
  # shellcheck disable=SC2086
  exec balance-history --root-dir "${root_dir}" ${BH_EXTRA_ARGS}
fi

exec balance-history --root-dir "${root_dir}"
