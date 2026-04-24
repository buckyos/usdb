#!/usr/bin/env bash

ETHW_GENESIS_SIGNATURE_SCHEME_ED25519="ed25519"

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

trusted_key_public_key_base64() {
  local trusted_keys_file="${1:?trusted_keys_file is required}"
  local key_id="${2:?key_id is required}"

  [[ -f "${trusted_keys_file}" ]] || return 1

  local flattened value
  flattened="$(tr -d '\r\n ' < "${trusted_keys_file}")"
  value="$(printf '%s' "${flattened}" | sed -nE "s/.*\\{\"key_id\":\"${key_id}\",\"public_key_base64\":\"([^\"]+)\"\\}.*/\\1/p")"
  [[ -n "${value}" ]] || return 1
  printf '%s' "${value}"
}

verify_ed25519_manifest_signature() {
  local manifest_file="${1:?manifest_file is required}"
  local signature_file="${2:?signature_file is required}"
  local public_key_base64="${3:?public_key_base64 is required}"

  local tmp_dir public_key_raw public_key_der signature_raw
  tmp_dir="$(mktemp -d)"
  public_key_raw="${tmp_dir}/public-key.raw"
  public_key_der="${tmp_dir}/public-key.der"
  signature_raw="${tmp_dir}/manifest.sig.raw"

  if ! printf '%s' "${public_key_base64}" | tr -d '\r\n ' | base64 -d >"${public_key_raw}" 2>/dev/null; then
    rm -rf "${tmp_dir}"
    echo "Failed to decode trusted ETHW genesis public key from base64" >&2
    return 1
  fi
  if [[ "$(wc -c < "${public_key_raw}")" -ne 32 ]]; then
    rm -rf "${tmp_dir}"
    echo "Invalid trusted ETHW genesis public key length: expected 32 bytes" >&2
    return 1
  fi

  printf '\x30\x2a\x30\x05\x06\x03\x2b\x65\x70\x03\x21\x00' >"${public_key_der}"
  cat "${public_key_raw}" >>"${public_key_der}"

  if ! tr -d '\r\n ' <"${signature_file}" | base64 -d >"${signature_raw}" 2>/dev/null; then
    rm -rf "${tmp_dir}"
    echo "Failed to decode ETHW genesis manifest signature from base64: ${signature_file}" >&2
    return 1
  fi
  if [[ "$(wc -c < "${signature_raw}")" -ne 64 ]]; then
    rm -rf "${tmp_dir}"
    echo "Invalid ETHW genesis manifest signature length: expected 64 bytes" >&2
    return 1
  fi

  if ! openssl pkeyutl \
    -verify \
    -pubin \
    -inkey "${public_key_der}" \
    -keyform DER \
    -rawin \
    -in "${manifest_file}" \
    -sigfile "${signature_raw}" >/dev/null 2>&1; then
    rm -rf "${tmp_dir}"
    echo "ETHW genesis manifest signature verification failed for ${manifest_file}" >&2
    return 1
  fi

  rm -rf "${tmp_dir}"
}

validate_ethw_genesis_artifact() {
  local trust_mode="${1:?trust_mode is required}"
  local genesis_file="${2:?genesis_file is required}"
  local manifest_file="${3:-}"
  local signature_file="${4:-}"
  local trusted_keys_file="${5:-}"

  case "${trust_mode}" in
    none|manifest|signed)
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
    if [[ "${trust_mode}" == "manifest" || "${trust_mode}" == "signed" ]]; then
      echo "ETHW_BOOTSTRAP_TRUST_MODE=${trust_mode} requires an ETHW genesis manifest file" >&2
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

  if [[ "${trust_mode}" != "signed" ]]; then
    return 0
  fi

  if [[ -z "${signature_file}" ]]; then
    echo "ETHW_BOOTSTRAP_TRUST_MODE=signed requires an ETHW genesis manifest signature file" >&2
    return 1
  fi
  if [[ ! -f "${signature_file}" ]]; then
    echo "ETHW genesis manifest signature does not exist: ${signature_file}" >&2
    return 1
  fi
  if [[ -z "${trusted_keys_file}" ]]; then
    echo "ETHW_BOOTSTRAP_TRUST_MODE=signed requires a trusted keys file" >&2
    return 1
  fi
  if [[ ! -f "${trusted_keys_file}" ]]; then
    echo "ETHW trusted keys file does not exist: ${trusted_keys_file}" >&2
    return 1
  fi

  local signature_scheme signing_key_id public_key_base64
  signature_scheme="$(json_extract_string_field "${manifest_file}" "signature_scheme" || true)"
  if [[ "${signature_scheme}" != "${ETHW_GENESIS_SIGNATURE_SCHEME_ED25519}" ]]; then
    echo "ETHW signed genesis manifest requires signature_scheme=${ETHW_GENESIS_SIGNATURE_SCHEME_ED25519}: ${manifest_file}" >&2
    return 1
  fi

  signing_key_id="$(json_extract_string_field "${manifest_file}" "signing_key_id" || true)"
  if [[ -z "${signing_key_id}" ]]; then
    echo "ETHW signed genesis manifest requires signing_key_id: ${manifest_file}" >&2
    return 1
  fi

  public_key_base64="$(trusted_key_public_key_base64 "${trusted_keys_file}" "${signing_key_id}" || true)"
  if [[ -z "${public_key_base64}" ]]; then
    echo "ETHW genesis signer ${signing_key_id} is not trusted by ${trusted_keys_file}" >&2
    return 1
  fi

  verify_ed25519_manifest_signature "${manifest_file}" "${signature_file}" "${public_key_base64}"
}
