#!/usr/bin/env bash
set -euo pipefail

bootstrap_dir="${BOOTSTRAP_DIR:-/bootstrap}"
mode="${SOURCE_DAO_BOOTSTRAP_MODE:-disabled}"
scope="${SOURCE_DAO_BOOTSTRAP_SCOPE:-dao-dividend-only}"
state_file="${SOURCE_DAO_BOOTSTRAP_STATE_FILE:-${bootstrap_dir}/sourcedao-bootstrap-state.json}"
marker_file="${SOURCE_DAO_BOOTSTRAP_MARKER_FILE:-${bootstrap_dir}/sourcedao-bootstrap.done.json}"
log_file="${SOURCE_DAO_BOOTSTRAP_LOG_FILE:-${bootstrap_dir}/sourcedao-bootstrap.log}"
config_file="${SOURCE_DAO_BOOTSTRAP_CONFIG_FILE:-${bootstrap_dir}/sourcedao-bootstrap-config.json}"
runtime_config_file="${SOURCE_DAO_BOOTSTRAP_RUNTIME_CONFIG_FILE:-${bootstrap_dir}/sourcedao-bootstrap.runtime.json}"
repo_dir="${SOURCE_DAO_REPO_DIR:-/workspace/SourceDAO}"
artifacts_dir="${SOURCE_DAO_ARTIFACTS_DIR:-${repo_dir}/artifacts-usdb}"
prepare_mode="${SOURCE_DAO_BOOTSTRAP_PREPARE:-validate}"
rpc_url="${ETHW_RPC_URL:-http://ethw-node:8545}"
wait_seconds="${SOURCE_DAO_RPC_WAIT_SECONDS:-300}"
tsx_bin="${SOURCE_DAO_TSX_BIN:-${repo_dir}/node_modules/.bin/tsx}"

mkdir -p "${bootstrap_dir}" "$(dirname "${state_file}")" "$(dirname "${marker_file}")"

json_string() {
  node -e 'process.stdout.write(JSON.stringify(process.argv[1] ?? ""));' "${1:-}"
}

read_config_value() {
  local field="$1"
  node -e '
    const fs = require("node:fs");
    const file = process.argv[1];
    const field = process.argv[2];
    const value = JSON.parse(fs.readFileSync(file, "utf8"))[field];
    process.stdout.write(value === undefined || value === null ? "" : String(value));
  ' "${config_file}" "${field}"
}

write_state() {
  local status="$1"
  local message="$2"
  local chain_id="${3:-}"
  local dao_address="${4:-}"
  local dividend_address="${5:-}"
  local completed_at="${6:-}"

  cat >"${state_file}" <<EOF
{
  "generated_at": $(json_string "$(date -u +"%Y-%m-%dT%H:%M:%SZ")"),
  "mode": $(json_string "${mode}"),
  "scope": $(json_string "${scope}"),
  "status": $(json_string "${status}"),
  "message": $(json_string "${message}"),
  "rpc_url": $(json_string "${rpc_url}"),
  "repo_dir": $(json_string "${repo_dir}"),
  "artifacts_dir": $(json_string "${artifacts_dir}"),
  "config_path": $(json_string "${config_file}"),
  "runtime_config_path": $(json_string "${runtime_config_file}"),
  "log_path": $(json_string "${log_file}"),
  "chain_id": $(if [[ -n "${chain_id}" ]]; then json_string "${chain_id}"; else printf 'null'; fi),
  "dao_address": $(if [[ -n "${dao_address}" ]]; then json_string "${dao_address}"; else printf 'null'; fi),
  "dividend_address": $(if [[ -n "${dividend_address}" ]]; then json_string "${dividend_address}"; else printf 'null'; fi),
  "completed_at": $(if [[ -n "${completed_at}" ]]; then json_string "${completed_at}"; else printf 'null'; fi)
}
EOF
}

write_marker() {
  local chain_id="$1"
  local dao_address="$2"
  local dividend_address="$3"
  local completed_at="$4"

  cat >"${marker_file}" <<EOF
{
  "completed": true,
  "completed_at": $(json_string "${completed_at}"),
  "mode": $(json_string "${mode}"),
  "scope": $(json_string "${scope}"),
  "chain_id": $(json_string "${chain_id}"),
  "dao_address": $(json_string "${dao_address}"),
  "dividend_address": $(json_string "${dividend_address}")
}
EOF
}

fail_with_state() {
  local message="$1"
  echo "${message}" >&2
  write_state "error" "${message}"
  exit 1
}

state_has_error_status() {
  [[ -s "${state_file}" ]] || return 1
  SOURCE_DAO_STATE_FILE="${state_file}" \
  node <<'NODE'
const fs = require("node:fs");
const stateFile = process.env.SOURCE_DAO_STATE_FILE;
try {
  const data = JSON.parse(fs.readFileSync(stateFile, "utf8"));
  process.exit(data?.status === "error" ? 0 : 1);
} catch {
  process.exit(1);
}
NODE
}

wait_for_ethw_rpc() {
  local deadline=$((SECONDS + wait_seconds))
  while (( SECONDS < deadline )); do
    if ETHW_RPC_URL="${rpc_url}" node <<'NODE' >/dev/null 2>&1
const rpcUrl = process.env.ETHW_RPC_URL;
async function main() {
  const response = await fetch(rpcUrl, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: 1,
      method: "eth_chainId",
      params: [],
    }),
  });
  if (!response.ok) {
    process.exit(1);
  }
  const payload = await response.json();
  if (payload.error || typeof payload.result !== "string" || payload.result.length === 0) {
    process.exit(1);
  }
}
main().catch(() => process.exit(1));
NODE
    then
      return 0
    fi
    sleep 2
  done
  return 1
}

prepare_workspace_auto() {
  if [[ ! -d "${repo_dir}/node_modules" ]]; then
    echo "Installing SourceDAO node_modules with npm ci"
    (cd "${repo_dir}" && npm ci)
  fi
  if [[ ! -d "${artifacts_dir}" ]]; then
    echo "Building SourceDAO USDB artifacts"
    (
      cd "${repo_dir}" && \
      SOURCE_DAO_ARTIFACTS_DIR="${artifacts_dir}" \
      SOURCE_DAO_CACHE_DIR="${repo_dir}/cache-usdb" \
      npm run build:usdb
    )
  fi
}

write_runtime_config() {
  SOURCE_DAO_SOURCE_CONFIG="${config_file}" \
  SOURCE_DAO_RUNTIME_CONFIG="${runtime_config_file}" \
  SOURCE_DAO_RUNTIME_ARTIFACTS="${artifacts_dir}" \
  SOURCE_DAO_RUNTIME_RPC_URL="${rpc_url}" \
  node <<'NODE'
const fs = require("node:fs");
const sourceConfig = process.env.SOURCE_DAO_SOURCE_CONFIG;
const runtimeConfig = process.env.SOURCE_DAO_RUNTIME_CONFIG;
const artifactsDir = process.env.SOURCE_DAO_RUNTIME_ARTIFACTS;
const rpcUrl = process.env.SOURCE_DAO_RUNTIME_RPC_URL;

const data = JSON.parse(fs.readFileSync(sourceConfig, "utf8"));
data.artifactsDir = artifactsDir;
data.rpcUrl = rpcUrl;
fs.writeFileSync(runtimeConfig, `${JSON.stringify(data, null, 2)}\n`);
NODE
}

run_bootstrap_worker() {
  case "${scope}" in
    dao-dividend-only)
      (
        cd "${repo_dir}" && \
        "${tsx_bin}" scripts/usdb_bootstrap_smoke.ts \
          --config "${runtime_config_file}" \
          --rpc-url "${rpc_url}"
      )
      ;;
    full)
      (
        cd "${repo_dir}" && \
        "${tsx_bin}" scripts/usdb_bootstrap_full.ts \
          --config "${runtime_config_file}" \
          --rpc-url "${rpc_url}" \
          --state-file "${state_file}" \
          --repo-dir "${repo_dir}"
      )
      ;;
    *)
      fail_with_state "Unsupported SOURCE_DAO_BOOTSTRAP_SCOPE=${scope}"
      ;;
  esac
}

validate_workspace() {
  [[ -d "${repo_dir}" ]] || fail_with_state "SourceDAO repo directory does not exist: ${repo_dir}"
  [[ -f "${config_file}" ]] || fail_with_state "Missing SourceDAO bootstrap config: ${config_file}"

  case "${prepare_mode}" in
    auto)
      prepare_workspace_auto
      ;;
    validate)
      ;;
    *)
      fail_with_state "Unsupported SOURCE_DAO_BOOTSTRAP_PREPARE=${prepare_mode}"
      ;;
  esac

  [[ -x "${tsx_bin}" ]] || fail_with_state "Missing tsx runtime under SourceDAO repo: ${tsx_bin}"
  [[ -d "${artifacts_dir}" ]] || fail_with_state "Missing SourceDAO artifacts directory: ${artifacts_dir}"
}

case "${mode}" in
  disabled)
    write_state "disabled" "SourceDAO bootstrap mode disabled"
    echo "SourceDAO bootstrap disabled; leaving without completion marker."
    exit 0
    ;;
  dev-workspace)
    ;;
  *)
    fail_with_state "Unsupported SOURCE_DAO_BOOTSTRAP_MODE=${mode}"
    ;;
esac

validate_workspace

if ! wait_for_ethw_rpc; then
  fail_with_state "ETHW RPC did not become ready within ${wait_seconds} seconds at ${rpc_url}"
fi

dao_address="$(read_config_value "daoAddress")"
dividend_address="$(read_config_value "dividendAddress")"
chain_id="$(read_config_value "chainId")"
write_runtime_config

: >"${log_file}"
echo "Running SourceDAO bootstrap (${scope}) from ${repo_dir}" | tee -a "${log_file}"
echo "Using config ${config_file}" | tee -a "${log_file}"
echo "Using runtime config ${runtime_config_file}" | tee -a "${log_file}"
echo "Using artifacts ${artifacts_dir}" | tee -a "${log_file}"

if ! run_bootstrap_worker 2>&1 | tee -a "${log_file}"; then
  if state_has_error_status; then
    echo "SourceDAO bootstrap failed. See ${log_file}" >&2
    exit 1
  fi
  fail_with_state "SourceDAO bootstrap failed. See ${log_file}"
fi

completed_at="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
if [[ "${scope}" == "dao-dividend-only" ]]; then
  write_state \
    "completed" \
    "SourceDAO bootstrap smoke completed successfully" \
    "${chain_id}" \
    "${dao_address}" \
    "${dividend_address}" \
    "${completed_at}"
fi
write_marker "${chain_id}" "${dao_address}" "${dividend_address}" "${completed_at}"

echo "SourceDAO bootstrap completed successfully."
