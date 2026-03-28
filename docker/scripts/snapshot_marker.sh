#!/usr/bin/env bash

snapshot_marker_path() {
  local root_dir="${1:?root_dir is required}"
  echo "${root_dir}/bootstrap/snapshot-loader.done.json"
}

snapshot_marker_escape() {
  local value="${1:-}"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '%s' "${value}"
}

snapshot_marker_extract_field() {
  local marker_path="${1:?marker_path is required}"
  local field_name="${2:?field_name is required}"

  if [[ ! -f "${marker_path}" ]]; then
    return 1
  fi

  local line
  line="$(grep -m1 "\"${field_name}\"" "${marker_path}" || true)"
  if [[ -z "${line}" ]]; then
    return 1
  fi

  local value
  value="$(printf '%s\n' "${line}" | sed -E 's/^[[:space:]]*"[^"]+"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*$/\1/')"
  value="${value//\\\"/\"}"
  value="${value//\\\\/\\}"
  printf '%s' "${value}"
}

snapshot_marker_matches() {
  local marker_path="${1:?marker_path is required}"
  local expected_mode="${2:?expected_mode is required}"
  local expected_file="${3:-}"
  local expected_manifest="${4:-}"

  [[ -f "${marker_path}" ]] || return 1

  local actual_mode actual_file actual_manifest
  actual_mode="$(snapshot_marker_extract_field "${marker_path}" "snapshot_mode" || true)"
  actual_file="$(snapshot_marker_extract_field "${marker_path}" "snapshot_file" || true)"
  actual_manifest="$(snapshot_marker_extract_field "${marker_path}" "snapshot_manifest" || true)"

  [[ "${actual_mode}" == "${expected_mode}" ]] || return 1
  [[ "${actual_file}" == "${expected_file}" ]] || return 1
  [[ "${actual_manifest}" == "${expected_manifest}" ]] || return 1
}

snapshot_marker_write() {
  local marker_path="${1:?marker_path is required}"
  local snapshot_mode="${2:?snapshot_mode is required}"
  local snapshot_file="${3:-}"
  local snapshot_manifest="${4:-}"

  mkdir -p "$(dirname "${marker_path}")"

  local tmp_path="${marker_path}.tmp"
  cat >"${tmp_path}" <<EOF
{
  "snapshot_mode": "$(snapshot_marker_escape "${snapshot_mode}")",
  "snapshot_file": "$(snapshot_marker_escape "${snapshot_file}")",
  "snapshot_manifest": "$(snapshot_marker_escape "${snapshot_manifest}")",
  "installed_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
}
EOF
  mv "${tmp_path}" "${marker_path}"
}
