#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
source "${script_dir}/ethw_bootstrap_artifact.sh"

bootstrap_dir="${BOOTSTRAP_DIR:-/bootstrap}"

mkdir -p "${bootstrap_dir}"

copy_optional_file() {
  local src="$1"
  local dst_name="$2"
  if [[ -z "${src}" ]]; then
    return 1
  fi
  if [[ ! -f "${src}" ]]; then
    echo "Bootstrap input file does not exist: ${src}" >&2
    exit 1
  fi
  cp "${src}" "${bootstrap_dir}/${dst_name}"
  return 0
}

ethw_genesis_copied="false"
ethw_genesis_manifest_copied="false"
ethw_genesis_manifest_verified="false"
ethw_genesis_sha256="null"
ethw_config_copied="false"
sourcedao_config_copied="false"
ethw_bootstrap_trust_mode="${ETHW_BOOTSTRAP_TRUST_MODE:-none}"

if copy_optional_file "${ETHW_BOOTSTRAP_GENESIS_INPUT_FILE:-}" "ethw-genesis.json"; then
  ethw_genesis_copied="true"
elif [[ "${BOOTSTRAP_REQUIRE_ETHW_GENESIS:-true}" == "true" ]]; then
  echo "Cold-start bootstrap requires ETHW_BOOTSTRAP_GENESIS_INPUT_FILE" >&2
  exit 1
fi

if [[ "${ethw_genesis_copied}" == "true" ]]; then
  if copy_optional_file "${ETHW_BOOTSTRAP_GENESIS_MANIFEST_INPUT_FILE:-}" "ethw-genesis.manifest.json"; then
    ethw_genesis_manifest_copied="true"
  fi
  copied_genesis="${bootstrap_dir}/ethw-genesis.json"
  copied_genesis_manifest=""
  if [[ "${ethw_genesis_manifest_copied}" == "true" ]]; then
    copied_genesis_manifest="${bootstrap_dir}/ethw-genesis.manifest.json"
  fi
  validate_ethw_genesis_artifact "${ethw_bootstrap_trust_mode}" "${copied_genesis}" "${copied_genesis_manifest}"
  ethw_genesis_sha256="\"$(json_escape "$(sha256_file "${copied_genesis}")")\""
  if [[ -n "${copied_genesis_manifest}" ]]; then
    ethw_genesis_manifest_verified="true"
  fi
fi

if copy_optional_file "${ETHW_BOOTSTRAP_CONFIG_INPUT_FILE:-}" "ethw-bootstrap-config.json"; then
  ethw_config_copied="true"
fi

if copy_optional_file "${SOURCE_DAO_CONFIG_INPUT_FILE:-}" "sourcedao-bootstrap-config.json"; then
  sourcedao_config_copied="true"
fi

manifest_path="${bootstrap_dir}/bootstrap-manifest.json"
generated_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

cat >"${manifest_path}" <<EOF
{
  "generated_at": "$(json_escape "${generated_at}")",
  "btc_network": "$(json_escape "${BTC_NETWORK:-bitcoin}")",
  "ethw_genesis_required": ${BOOTSTRAP_REQUIRE_ETHW_GENESIS:-true},
  "ethw_bootstrap_trust_mode": "$(json_escape "${ethw_bootstrap_trust_mode}")",
  "ethw_genesis_copied": ${ethw_genesis_copied},
  "ethw_genesis_path": $(if [[ "${ethw_genesis_copied}" == "true" ]]; then printf '"%s"' "$(json_escape "${bootstrap_dir}/ethw-genesis.json")"; else printf 'null'; fi),
  "ethw_genesis_manifest_copied": ${ethw_genesis_manifest_copied},
  "ethw_genesis_manifest_path": $(if [[ "${ethw_genesis_manifest_copied}" == "true" ]]; then printf '"%s"' "$(json_escape "${bootstrap_dir}/ethw-genesis.manifest.json")"; else printf 'null'; fi),
  "ethw_genesis_manifest_verified": ${ethw_genesis_manifest_verified},
  "ethw_genesis_sha256": ${ethw_genesis_sha256},
  "ethw_bootstrap_config_copied": ${ethw_config_copied},
  "ethw_bootstrap_config_path": $(if [[ "${ethw_config_copied}" == "true" ]]; then printf '"%s"' "$(json_escape "${bootstrap_dir}/ethw-bootstrap-config.json")"; else printf 'null'; fi),
  "sourcedao_config_copied": ${sourcedao_config_copied},
  "sourcedao_config_path": $(if [[ "${sourcedao_config_copied}" == "true" ]]; then printf '"%s"' "$(json_escape "${bootstrap_dir}/sourcedao-bootstrap-config.json")"; else printf 'null'; fi),
  "balance_history_snapshot_mode": "$(json_escape "${SNAPSHOT_MODE:-none}")",
  "balance_history_snapshot_file": $(if [[ -n "${BH_SNAPSHOT_FILE:-}" ]]; then printf '"%s"' "$(json_escape "${BH_SNAPSHOT_FILE}")"; else printf 'null'; fi),
  "balance_history_snapshot_manifest": $(if [[ -n "${BH_SNAPSHOT_MANIFEST:-}" ]]; then printf '"%s"' "$(json_escape "${BH_SNAPSHOT_MANIFEST}")"; else printf 'null'; fi),
  "balance_history_snapshot_trust_mode": "$(json_escape "${BH_SNAPSHOT_TRUST_MODE:-dev}")"
}
EOF

echo "Bootstrap artifacts prepared under ${bootstrap_dir}"
echo "Bootstrap manifest written to ${manifest_path}"
