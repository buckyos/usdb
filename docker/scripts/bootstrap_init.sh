#!/usr/bin/env bash
set -euo pipefail

bootstrap_dir="${BOOTSTRAP_DIR:-/bootstrap}"
input_dir="/bootstrap-input"

mkdir -p "${bootstrap_dir}"

json_escape() {
  local value="${1:-}"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  printf '%s' "${value}"
}

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
ethw_config_copied="false"
sourcedao_config_copied="false"

if copy_optional_file "${ETHW_BOOTSTRAP_GENESIS_FILE:-}" "ethw-genesis.json"; then
  ethw_genesis_copied="true"
elif [[ "${BOOTSTRAP_REQUIRE_ETHW_GENESIS:-true}" == "true" ]]; then
  echo "Cold-start bootstrap requires ETHW_BOOTSTRAP_GENESIS_FILE" >&2
  exit 1
fi

if copy_optional_file "${ETHW_BOOTSTRAP_CONFIG_FILE:-}" "ethw-bootstrap-config.json"; then
  ethw_config_copied="true"
fi

if copy_optional_file "${SOURCE_DAO_CONFIG_FILE:-}" "sourcedao-bootstrap-config.json"; then
  sourcedao_config_copied="true"
fi

manifest_path="${bootstrap_dir}/bootstrap-manifest.json"
generated_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"

cat >"${manifest_path}" <<EOF
{
  "generated_at": "$(json_escape "${generated_at}")",
  "btc_network": "$(json_escape "${BTC_NETWORK:-bitcoin}")",
  "ethw_genesis_required": ${BOOTSTRAP_REQUIRE_ETHW_GENESIS:-true},
  "ethw_genesis_copied": ${ethw_genesis_copied},
  "ethw_genesis_path": $(if [[ "${ethw_genesis_copied}" == "true" ]]; then printf '"%s"' "$(json_escape "${bootstrap_dir}/ethw-genesis.json")"; else printf 'null'; fi),
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
