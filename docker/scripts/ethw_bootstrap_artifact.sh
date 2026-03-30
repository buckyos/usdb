#!/usr/bin/env bash

json_escape() {
  local value="${1:-}"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  value="${value//$'\n'/\\n}"
  printf '%s' "${value}"
}

json_extract_string_field() {
  local json_path="${1:?json_path is required}"
  local field_name="${2:?field_name is required}"

  [[ -f "${json_path}" ]] || return 1

  local line value
  line="$(grep -m1 "\"${field_name}\"" "${json_path}" || true)"
  [[ -n "${line}" ]] || return 1

  value="$(printf '%s\n' "${line}" | sed -E 's/^[[:space:]]*"[^"]+"[[:space:]]*:[[:space:]]*"(([^"\\]|\\.)*)".*$/\1/')"
  value="${value//\\\"/\"}"
  value="${value//\\\\/\\}"
  printf '%s' "${value}"
}

sha256_file() {
  local path="${1:?path is required}"
  sha256sum "${path}" | awk '{print $1}'
}

validate_ethw_genesis_artifact() {
  local trust_mode="${1:?trust_mode is required}"
  local genesis_file="${2:?genesis_file is required}"
  local manifest_file="${3:-}"

  case "${trust_mode}" in
    none|manifest)
      ;;
    *)
      echo "Unsupported ETHW_BOOTSTRAP_TRUST_MODE=${trust_mode}" >&2
      return 1
      ;;
  esac

  if [[ ! -f "${genesis_file}" ]]; then
    echo "ETHW genesis file does not exist: ${genesis_file}" >&2
    return 1
  fi

  if [[ -z "${manifest_file}" ]]; then
    if [[ "${trust_mode}" == "manifest" ]]; then
      echo "ETHW_BOOTSTRAP_TRUST_MODE=manifest requires ETHW_BOOTSTRAP_GENESIS_MANIFEST_INPUT_FILE" >&2
      return 1
    fi
    return 0
  fi

  if [[ ! -f "${manifest_file}" ]]; then
    echo "ETHW genesis manifest does not exist: ${manifest_file}" >&2
    return 1
  fi

  local expected_sha actual_sha
  expected_sha="$(json_extract_string_field "${manifest_file}" "file_sha256" || true)"
  if [[ -z "${expected_sha}" ]]; then
    echo "ETHW genesis manifest missing file_sha256: ${manifest_file}" >&2
    return 1
  fi

  actual_sha="$(sha256_file "${genesis_file}")"
  if [[ "${actual_sha}" != "${expected_sha}" ]]; then
    echo "ETHW genesis file_sha256 mismatch for ${genesis_file}: expected ${expected_sha}, got ${actual_sha}" >&2
    return 1
  fi
}
