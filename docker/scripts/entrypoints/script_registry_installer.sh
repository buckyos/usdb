#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root_dir="${BH_ROOT_DIR:-/data/balance-history}"
sidecar_root="${root_dir}/auxiliary/script-registry"
bases_root="${sidecar_root}/bases"
attempt_path="${sidecar_root}/attempt.json"
artifact_id="${BH_SCRIPT_REGISTRY_ARTIFACT_ID:-}"

write_attempt() {
  local state="$1"
  local error_text="${2:-}"
  mkdir -p "${sidecar_root}"
  python3 - "${attempt_path}" "${state}" "${artifact_id}" "${error_text}" <<'PY'
import json
import os
import sys
import time

path, state, artifact_id, error_text = sys.argv[1:]
value = {
    "schema_version": "balance-history-script-registry-attempt:v1",
    "state": state,
    "artifact_id": artifact_id or None,
    "last_error": error_text or None,
    "updated_at": int(time.time()),
}
temporary = f"{path}.tmp.{os.getpid()}"
with open(temporary, "x", encoding="utf-8") as output:
    json.dump(value, output, indent=2, sort_keys=True)
    output.write("\n")
    output.flush()
    os.fsync(output.fileno())
os.replace(temporary, path)
directory = os.open(os.path.dirname(path), os.O_RDONLY)
try:
    os.fsync(directory)
finally:
    os.close(directory)
PY
}

fail() {
  local exit_code=$?
  local line_number="${BASH_LINENO[0]:-unknown}"
  write_attempt "failed" "installer exited with status ${exit_code} at line ${line_number}"
  exit "${exit_code}"
}
trap fail ERR

if [[ "${BH_SCRIPT_REGISTRY_ENABLED:-0}" != "1" ]]; then
  write_attempt "disabled"
  echo "Script-registry sidecar acquisition is disabled"
  exit 0
fi
if [[ -z "${BH_SCRIPT_REGISTRY_RECORD_URL:-}" || -z "${artifact_id}" ]]; then
  echo "Enabled script-registry acquisition requires record URL and artifact ID" >&2
  exit 1
fi
if [[ -z "${BH_SNAPSHOT_MANIFEST:-}" || -z "${BH_SNAPSHOT_TRUSTED_KEYS_FILE:-}" ]]; then
  echo "Script-registry activation requires the installed core manifest and trusted keys" >&2
  exit 1
fi

write_attempt "downloading"
install_output="$({
  python3 "${script_dir}/../tools/snapshot_distribution.py" install \
    --record-url "${BH_SCRIPT_REGISTRY_RECORD_URL}" \
    --destination-root "${bases_root}" \
    --trusted-keys "${BH_SNAPSHOT_TRUSTED_KEYS_FILE}" \
    --component script_registry \
    --expected-network "${BTC_NETWORK:?BTC_NETWORK is required}" \
    --max-height "${BH_SYNC_MAX_SYNC_BLOCK_HEIGHT:-4294967295}" \
    --download-concurrency "${BH_SCRIPT_REGISTRY_DOWNLOAD_CONCURRENCY:-8}" \
    --download-chunk-size-mib "${BH_SCRIPT_REGISTRY_DOWNLOAD_CHUNK_SIZE_MIB:-64}"
})"
manifest_path="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["manifest_file"])' <<<"${install_output}")"
installed_artifact_id="$(python3 -c 'import json,sys; print(json.load(sys.stdin)["artifact_id"])' <<<"${install_output}")"
if [[ "${installed_artifact_id}" != "${artifact_id}" ]]; then
  echo "Installed registry artifact ID differs from release selection" >&2
  exit 1
fi

write_attempt "verifying"
balance-history \
  --root-dir "${root_dir}" \
  activate-script-registry \
  --manifest "${manifest_path}" \
  --core-manifest "${BH_SNAPSHOT_MANIFEST}" \
  --trusted-keys "${BH_SNAPSHOT_TRUSTED_KEYS_FILE}"
write_attempt "ready"
trap - ERR
echo "Script-registry sidecar is verified and active: ${artifact_id}"
