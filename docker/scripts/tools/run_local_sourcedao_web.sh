#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd "${script_dir}/../../.." && pwd)"
tool_cmd="docker/scripts/tools/run_local_sourcedao_web.sh"

control_plane_url="${CONTROL_PLANE_URL:-http://127.0.0.1:28140}"
sourcedao_web_dir="${SOURCEDAO_WEB_DIR:-${repo_dir}/../buckydaowww/src}"
sourcedao_web_port="${SOURCEDAO_WEB_PORT:-3050}"
ethw_public_rpc_url="${ETHW_PUBLIC_RPC_URL:-http://127.0.0.1:8545}"
sourcedao_backend_url="${SOURCEDAO_BACKEND_URL:-http://127.0.0.1:3333}"

usage() {
  cat <<EOF
Usage:
  ${tool_cmd} [up|env|check]

Starts the standalone SourceDAO web frontend against the currently running
USDB local control-plane runtime.

Environment:
  CONTROL_PLANE_URL        control-plane base URL, default ${control_plane_url}
  SOURCEDAO_WEB_DIR        buckydaowww/src directory, default ${sourcedao_web_dir}
  SOURCEDAO_WEB_PORT       local Next.js port, default ${sourcedao_web_port}
  ETHW_PUBLIC_RPC_URL      browser-facing ETHW RPC URL, default ${ethw_public_rpc_url}
  SOURCEDAO_BACKEND_URL    browser-facing SourceDAO backend URL, default ${sourcedao_backend_url}

Actions:
  up     Generate runtime env and run Next.js dev server.
  env    Print the generated env exports without starting the server.
  check  Validate that control-plane and SourceDAO bootstrap data are available.
EOF
}

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "Required command not found: $1" >&2
    exit 1
  }
}

emit_sourcedao_env() {
  python3 - "${control_plane_url}" "${ethw_public_rpc_url}" "${sourcedao_backend_url}" <<'PY'
import json
import shlex
import sys
import urllib.request

control_plane_url, ethw_public_rpc_url, sourcedao_backend_url = sys.argv[1:4]
overview_url = control_plane_url.rstrip("/") + "/api/system/overview"

try:
    with urllib.request.urlopen(overview_url, timeout=10) as response:
        overview = json.load(response)
except Exception as exc:
    raise SystemExit(f"Failed to load control-plane overview from {overview_url}: {exc}")

bootstrap_state = (
    overview.get("bootstrap", {})
    .get("sourcedao_bootstrap_state", {})
    .get("data")
)
if not isinstance(bootstrap_state, dict):
    raise SystemExit("SourceDAO bootstrap state is not available in control-plane overview.")

if bootstrap_state.get("status") != "completed":
    raise SystemExit(
        f"SourceDAO bootstrap is not completed yet: {bootstrap_state.get('status')}"
    )

final_wiring = bootstrap_state.get("final_wiring") or {}
required = {
    "NEXT_PUBLIC_MAIN": bootstrap_state.get("dao_address"),
    "NEXT_PUBLIC_DIVIDEND": bootstrap_state.get("dividend_address") or final_wiring.get("dividend"),
    "NEXT_PUBLIC_COMMITTEE": final_wiring.get("committee"),
    "NEXT_PUBLIC_DEV_TOKEN": final_wiring.get("dev_token"),
    "NEXT_PUBLIC_NORMAL_TOKEN": final_wiring.get("normal_token"),
    "NEXT_PUBLIC_LOCKUP": final_wiring.get("token_lockup"),
    "NEXT_PUBLIC_PROJECT": final_wiring.get("project"),
    "NEXT_PUBLIC_ACQUIRED": final_wiring.get("acquired"),
}
missing = [key for key, value in required.items() if not value]
if missing:
    raise SystemExit("SourceDAO bootstrap state is missing fields: " + ", ".join(missing))

env = {
    "NEXT_PUBLIC_CHAIN": "USDB Local ETHW",
    "NEXT_PUBLIC_NETWORK_ID": str(bootstrap_state.get("chain_id") or ""),
    "NEXT_PUBLIC_LOCAL_AUTH_MODE": "wallet",
    "NEXT_PUBLIC_RPC_URL": ethw_public_rpc_url,
    "NEXT_PUBLIC_SERVER": sourcedao_backend_url,
    "NEXT_PUBLIC_TOKEN_ADDRESS_LINK": "",
    "NEXT_PUBLIC_ADDRESS_LINK": "",
    **required,
}

for key, value in env.items():
    print(f"export {key}={shlex.quote(str(value))}")
PY
}

action="${1:-up}"
shift || true

case "${action}" in
  -h|--help|help)
    usage
    exit 0
    ;;
esac

require_cmd python3

case "${action}" in
  env)
    emit_sourcedao_env
    ;;
  check)
    emit_sourcedao_env >/dev/null
    [[ -d "${sourcedao_web_dir}" ]] || {
      echo "SourceDAO web directory not found: ${sourcedao_web_dir}" >&2
      exit 1
    }
    echo "SourceDAO web runtime env is available."
    echo "SourceDAO web dir: ${sourcedao_web_dir}"
    echo "Target URL: http://127.0.0.1:${sourcedao_web_port}"
    ;;
  up)
    require_cmd npm
    [[ -d "${sourcedao_web_dir}" ]] || {
      echo "SourceDAO web directory not found: ${sourcedao_web_dir}" >&2
      exit 1
    }
    eval "$(emit_sourcedao_env)"
    cd "${sourcedao_web_dir}"
    if [[ ! -d node_modules ]]; then
      npm install
    fi
    echo "Starting SourceDAO web at http://127.0.0.1:${sourcedao_web_port}"
    echo "Using ETHW RPC ${NEXT_PUBLIC_RPC_URL} and backend ${NEXT_PUBLIC_SERVER}"
    exec npm run dev -- -H 0.0.0.0 -p "${sourcedao_web_port}" "$@"
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
