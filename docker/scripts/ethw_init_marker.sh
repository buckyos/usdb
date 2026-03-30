#!/usr/bin/env bash

ethw_init_marker_path() {
  local data_dir="${1:?data_dir is required}"
  echo "${data_dir}/bootstrap/ethw-init.done.json"
}

ethw_marker_escape() {
  local value="${1:-}"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '%s' "${value}"
}

ethw_marker_extract_field() {
  local marker_path="${1:?marker_path is required}"
  local field_name="${2:?field_name is required}"

  [[ -f "${marker_path}" ]] || return 1

  local line value
  line="$(grep -m1 "\"${field_name}\"" "${marker_path}" || true)"
  [[ -n "${line}" ]] || return 1

  value="$(printf '%s\n' "${line}" | sed -E 's/^[[:space:]]*"[^"]+"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*$/\1/')"
  value="${value//\\\"/\"}"
  value="${value//\\\\/\\}"
  printf '%s' "${value}"
}

ethw_init_marker_matches() {
  local marker_path="${1:?marker_path is required}"
  local expected_genesis_file="${2:?expected_genesis_file is required}"
  local expected_manifest_file="${3:-}"
  local expected_genesis_sha256="${4:?expected_genesis_sha256 is required}"

  [[ -f "${marker_path}" ]] || return 1

  local actual_genesis_file actual_manifest_file actual_genesis_sha256
  actual_genesis_file="$(ethw_marker_extract_field "${marker_path}" "genesis_file" || true)"
  actual_manifest_file="$(ethw_marker_extract_field "${marker_path}" "genesis_manifest_file" || true)"
  actual_genesis_sha256="$(ethw_marker_extract_field "${marker_path}" "genesis_sha256" || true)"

  [[ "${actual_genesis_file}" == "${expected_genesis_file}" ]] || return 1
  [[ "${actual_manifest_file}" == "${expected_manifest_file}" ]] || return 1
  [[ "${actual_genesis_sha256}" == "${expected_genesis_sha256}" ]] || return 1
}

ethw_init_marker_write() {
  local marker_path="${1:?marker_path is required}"
  local genesis_file="${2:?genesis_file is required}"
  local genesis_manifest_file="${3:-}"
  local genesis_sha256="${4:?genesis_sha256 is required}"

  mkdir -p "$(dirname "${marker_path}")"

  local tmp_path="${marker_path}.tmp"
  cat >"${tmp_path}" <<EOF
{
  "genesis_file": "$(ethw_marker_escape "${genesis_file}")",
  "genesis_manifest_file": "$(ethw_marker_escape "${genesis_manifest_file}")",
  "genesis_sha256": "$(ethw_marker_escape "${genesis_sha256}")",
  "initialized_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
  mv "${tmp_path}" "${marker_path}"
}
