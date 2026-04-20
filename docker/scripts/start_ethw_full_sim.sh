#!/usr/bin/env bash
set -euo pipefail

ethw_data_dir="${ETHW_DATA_DIR:-/data/ethw}"
identity_dir="${ETHW_IDENTITY_DIR:-${ethw_data_dir}/bootstrap}"
identity_marker="${ETHW_IDENTITY_MARKER:-${identity_dir}/ethw-sim-identity.json}"
identity_mode="${ETHW_IDENTITY_MODE:-none}"
identity_seed="${ETHW_IDENTITY_SEED:-${WORLD_SIM_IDENTITY_SEED:-}}"
explicit_address="${ETHW_MINER_ADDRESS:-}"
explicit_private_key_hex="${ETHW_MINER_PRIVATE_KEY_HEX:-}"
auto_append_etherbase="${ETHW_AUTO_APPEND_MINER_ETHERBASE:-1}"
ethw_command="${ETHW_COMMAND:-}"
ethw_geth_bin="${ETHW_GETH_BIN:-geth}"

if [[ -z "${ethw_command}" ]]; then
  echo "ETHW_COMMAND is not set" >&2
  exit 1
fi

log() {
  printf '[ethw-full-sim] %s\n' "$*" >&2
}

sanitize_legacy_etherbase_placeholder() {
  ETHW_COMMAND_INPUT="${ethw_command}" python3 <<'PY'
import os
import shlex

command = os.environ["ETHW_COMMAND_INPUT"]
tokens = shlex.split(command)
sanitized = []
i = 0
while i < len(tokens):
    token = tokens[i]
    next_token = tokens[i + 1] if i + 1 < len(tokens) else ""

    if token in ("--miner.etherbase", "--etherbase") and "ETHW_MINER_ADDRESS" in next_token:
        i += 2
        continue
    if (token.startswith("--miner.etherbase=") or token.startswith("--etherbase=")) and "ETHW_MINER_ADDRESS" in token:
        i += 1
        continue

    sanitized.append(token)
    i += 1

print(shlex.join(sanitized))
PY
}

json_read_field() {
  local file="${1:?file is required}"
  local field="${2:?field is required}"
  python3 - "${file}" "${field}" <<'PY'
import json
import sys

path, field = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as fh:
    data = json.load(fh)
value = data.get(field)
if value is None:
    print("")
elif isinstance(value, bool):
    print("true" if value else "false")
else:
    print(str(value))
PY
}

sha256_hex() {
  local value="${1:?value is required}"
  python3 - "${value}" <<'PY'
import hashlib
import sys

print(hashlib.sha256(sys.argv[1].encode("utf-8")).hexdigest())
PY
}

derive_private_key_hex_from_seed() {
  local seed="${1:?seed is required}"
  local role="${2:?role is required}"
  local counter="${3:?counter is required}"
  python3 - "${seed}" "${role}" "${counter}" <<'PY'
import hashlib
import sys

seed, role, counter = sys.argv[1], sys.argv[2], sys.argv[3]
material = f"usdb-ethw-sim-v1:{role}:{seed}:{counter}".encode("utf-8")
digest = hashlib.sha256(material).hexdigest()
if int(digest, 16) == 0:
    raise SystemExit("derived zero private key")
print(digest)
PY
}

identity_fingerprint() {
  if [[ -n "${explicit_private_key_hex}" ]]; then
    sha256_hex "explicit-key:${explicit_private_key_hex}"
  elif [[ "${identity_mode}" == "deterministic-seed" ]]; then
    sha256_hex "deterministic-seed:${identity_seed}"
  elif [[ -n "${explicit_address}" ]]; then
    sha256_hex "explicit-address:${explicit_address}"
  else
    sha256_hex "none"
  fi
}

write_identity_marker() {
  local address="${1:?address is required}"
  local fingerprint="${2:?fingerprint is required}"
  local scheme="${3:?scheme is required}"
  mkdir -p "${identity_dir}"
  python3 - "${identity_marker}" "${identity_mode}" "${address}" "${fingerprint}" "${scheme}" <<'PY'
import json
import os
import sys

path, mode, address, fingerprint, scheme = sys.argv[1:6]
payload = {
    "identity_mode": mode,
    "identity_scheme": scheme,
    "ethw_miner_address": address,
    "identity_fingerprint": fingerprint,
}
tmp = f"{path}.tmp"
with open(tmp, "w", encoding="utf-8") as fh:
    json.dump(payload, fh, indent=2, sort_keys=True)
    fh.write("\n")
os.replace(tmp, path)
PY
}

load_identity_marker_if_matching() {
  local requested_fingerprint="${1:?fingerprint is required}"
  [[ -f "${identity_marker}" ]] || return 1

  local current_fingerprint current_address
  current_fingerprint="$(json_read_field "${identity_marker}" "identity_fingerprint")"
  current_address="$(json_read_field "${identity_marker}" "ethw_miner_address")"

  if [[ -z "${current_fingerprint}" || -z "${current_address}" ]]; then
    echo "ETHW identity marker is incomplete: ${identity_marker}" >&2
    return 1
  fi
  if [[ "${current_fingerprint}" != "${requested_fingerprint}" ]]; then
    echo "Existing ETHW identity marker does not match the current full-sim identity request: ${identity_marker}" >&2
    echo "Clear the ETHW data volume or use a matching identity configuration." >&2
    exit 1
  fi

  printf '%s\n' "${current_address}"
}

import_private_key_and_resolve_address() {
  local private_key_hex="${1:?private key is required}"
  local password_file key_file import_output address
  password_file="$(mktemp)"
  key_file="$(mktemp)"
  trap 'rm -f "${password_file}" "${key_file}"' RETURN
  : >"${password_file}"
  printf '%s\n' "${private_key_hex}" >"${key_file}"

  import_output="$("${ethw_geth_bin}" --datadir "${ethw_data_dir}" account import --password "${password_file}" "${key_file}" 2>&1)"
  address="$(printf '%s\n' "${import_output}" | sed -n 's/.*Address: {\([0-9a-fA-F]\{40\}\)}.*/0x\1/p' | tail -n 1)"
  if [[ -z "${address}" ]]; then
    echo "Failed to parse ETHW miner address from geth account import output" >&2
    printf '%s\n' "${import_output}" >&2
    exit 1
  fi

  trap - RETURN
  rm -f "${password_file}" "${key_file}"
  printf '%s\n' "${address}"
}

resolve_ethw_miner_address() {
  local requested_fingerprint address candidate_key counter
  requested_fingerprint="$(identity_fingerprint)"

  if address="$(load_identity_marker_if_matching "${requested_fingerprint}")"; then
    log "Reusing ETHW miner identity from ${identity_marker}: ${address}"
    printf '%s\n' "${address}"
    return 0
  fi

  case "${identity_mode}" in
    none)
      if [[ -n "${explicit_address}" ]]; then
        write_identity_marker "${explicit_address}" "${requested_fingerprint}" "address-only-v1"
        printf '%s\n' "${explicit_address}"
        return 0
      fi
      return 1
      ;;
    deterministic-seed)
      if [[ -z "${identity_seed}" ]]; then
        echo "ETHW_IDENTITY_MODE=deterministic-seed requires ETHW_IDENTITY_SEED or WORLD_SIM_IDENTITY_SEED" >&2
        return 2
      fi
      for counter in $(seq 0 7); do
        candidate_key="$(derive_private_key_hex_from_seed "${identity_seed}" "miner" "${counter}")"
        if address="$(import_private_key_and_resolve_address "${candidate_key}")"; then
          explicit_private_key_hex="${candidate_key}"
          write_identity_marker "${address}" "${requested_fingerprint}" "deterministic-seed-v1"
          printf '%s\n' "${address}"
          return 0
        fi
      done
      echo "Failed to derive and import a deterministic ETHW miner identity from ETHW_IDENTITY_SEED" >&2
      return 2
      ;;
    explicit-key)
      if [[ -z "${explicit_private_key_hex}" ]]; then
        echo "ETHW_IDENTITY_MODE=explicit-key requires ETHW_MINER_PRIVATE_KEY_HEX" >&2
        return 2
      fi
      address="$(import_private_key_and_resolve_address "${explicit_private_key_hex}")"
      write_identity_marker "${address}" "${requested_fingerprint}" "explicit-key-v1"
      printf '%s\n' "${address}"
      return 0
      ;;
    explicit-address)
      if [[ -z "${explicit_address}" ]]; then
        echo "ETHW_IDENTITY_MODE=explicit-address requires ETHW_MINER_ADDRESS" >&2
        return 2
      fi
      write_identity_marker "${explicit_address}" "${requested_fingerprint}" "address-only-v1"
      printf '%s\n' "${explicit_address}"
      return 0
      ;;
    *)
      echo "Unsupported ETHW_IDENTITY_MODE=${identity_mode}" >&2
      return 2
      ;;
  esac
}

if [[ "${ethw_command}" == *"ETHW_MINER_ADDRESS"* ]]; then
  ethw_command="$(sanitize_legacy_etherbase_placeholder)"
fi

set +e
miner_address="$(resolve_ethw_miner_address)"
resolve_status=$?
set -e

if [[ "${resolve_status}" -eq 0 ]]; then
  export ETHW_MINER_ADDRESS="${miner_address}"
  if [[ "${auto_append_etherbase}" == "1" && "${ethw_command}" != *"--miner.etherbase"* && "${ethw_command}" != *"--etherbase"* ]]; then
    ethw_command="${ethw_command} --miner.etherbase ${ETHW_MINER_ADDRESS}"
  fi
  log "Using ETHW miner address ${ETHW_MINER_ADDRESS}"
elif [[ "${resolve_status}" -eq 1 ]]; then
  log "No ETHW miner identity override requested; starting with provided ETHW_COMMAND"
else
  exit "${resolve_status}"
fi

exec bash -lc "${ethw_command}"
