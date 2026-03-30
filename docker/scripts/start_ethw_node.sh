#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${script_dir}/ethw_bootstrap_artifact.sh"
source "${script_dir}/ethw_init_marker.sh"

bootstrap_dir="${BOOTSTRAP_DIR:-/bootstrap}"
ethw_data_dir="${ETHW_DATA_DIR:-/data/ethw}"
trust_mode="${ETHW_BOOTSTRAP_TRUST_MODE:-none}"
genesis_file="${ETHW_CANONICAL_GENESIS_FILE:-${bootstrap_dir}/ethw-genesis.json}"
genesis_manifest_file="${ETHW_CANONICAL_GENESIS_MANIFEST_FILE:-}"
genesis_signature_file="${ETHW_CANONICAL_GENESIS_SIG_FILE:-}"
trusted_keys_file="${ETHW_CANONICAL_TRUSTED_KEYS_FILE:-}"
marker_path="$(ethw_init_marker_path "${ethw_data_dir}")"

if [[ -z "${genesis_manifest_file}" && -f "${bootstrap_dir}/ethw-genesis.manifest.json" ]]; then
  genesis_manifest_file="${bootstrap_dir}/ethw-genesis.manifest.json"
fi
if [[ -z "${genesis_signature_file}" && -f "${bootstrap_dir}/ethw-genesis.manifest.sig" ]]; then
  genesis_signature_file="${bootstrap_dir}/ethw-genesis.manifest.sig"
fi
if [[ -z "${trusted_keys_file}" && -f "${bootstrap_dir}/trusted-ethw-genesis-keys.json" ]]; then
  trusted_keys_file="${bootstrap_dir}/trusted-ethw-genesis-keys.json"
fi

validate_ethw_genesis_artifact "${trust_mode}" "${genesis_file}" "${genesis_manifest_file}" "${genesis_signature_file}" "${trusted_keys_file}"
genesis_sha256="$(sha256_file "${genesis_file}")"

if ! ethw_init_marker_matches "${marker_path}" "${genesis_file}" "${genesis_manifest_file}" "${genesis_sha256}"; then
  echo "ETHW bootstrap requires a matching init marker at ${marker_path}" >&2
  exit 1
fi

if [[ -z "${ETHW_COMMAND:-}" ]]; then
  echo "ETHW_COMMAND is not set" >&2
  exit 1
fi

exec bash -lc "${ETHW_COMMAND}"
