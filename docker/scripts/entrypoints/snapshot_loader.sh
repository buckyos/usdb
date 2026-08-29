#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
# shellcheck source=../helpers/snapshot_marker.sh
source "${script_dir}/../helpers/snapshot_marker.sh"

snapshot_mode="${SNAPSHOT_MODE:-none}"
root_dir="${BH_ROOT_DIR:-/data/balance-history}"
config_path="${root_dir}/config.toml"
db_dir="${root_dir}/db"
marker_path="$(snapshot_marker_path "${root_dir}")"
indexer_root="${USDB_INDEXER_ROOT_DIR:-/data/usdb-indexer}"
indexer_config_path="${indexer_root}/config.json"

"${script_dir}/../helpers/render_balance_history_config.sh" "${config_path}"

case "${snapshot_mode}" in
  none)
    echo "Snapshot loader disabled (SNAPSHOT_MODE=none)"
    exit 0
    ;;
  balance-history)
    ;;
  paired-checkpoint)
    "${script_dir}/../helpers/render_usdb_indexer_config.sh" "${indexer_config_path}"
    checkpoint_manifest="${USDB_INDEXER_CHECKPOINT_MANIFEST:-}"
    snapshot_manifest="${BH_SNAPSHOT_MANIFEST:-}"
    trusted_keys="${BH_SNAPSHOT_TRUSTED_KEYS_FILE:-}"
    if [[ -z "${checkpoint_manifest}" || -z "${snapshot_manifest}" || -z "${trusted_keys}" ]]; then
      echo "SNAPSHOT_MODE=paired-checkpoint requires USDB_INDEXER_CHECKPOINT_MANIFEST, BH_SNAPSHOT_MANIFEST, and BH_SNAPSHOT_TRUSTED_KEYS_FILE" >&2
      exit 1
    fi
    args=(
      install-pair
      --checkpoint-manifest "${checkpoint_manifest}"
      --balance-history-manifest "${snapshot_manifest}"
      --trusted-keys "${trusted_keys}"
      --indexer-root "${indexer_root}"
      --balance-history-root "${root_dir}"
      --network-bundle-id "${USDB_NETWORK_BUNDLE_ID:?USDB_NETWORK_BUNDLE_ID is required}"
      --chain-id "${USDB_CHAIN_ID:?USDB_CHAIN_ID is required}"
      --index-origin-height "${USDB_GENESIS_BLOCK_HEIGHT:?USDB_GENESIS_BLOCK_HEIGHT is required}"
      --lock-timeout-secs "${USDB_CHECKPOINT_LOCK_TIMEOUT_SECS:-10}"
    )
    usdb-indexer-checkpoint-tool "${args[@]}"
    snapshot_marker_write "${marker_path}" "${snapshot_mode}" "${BH_SNAPSHOT_FILE:-}" "${snapshot_manifest}"
    echo "Paired checkpoint install completed and marker written to ${marker_path}"
    exit 0
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
