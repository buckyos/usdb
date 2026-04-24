#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/../.." && pwd)"

env_dir="${docker_dir}/local/world-sim/env"
env_file="${env_dir}/world-sim.env"
env_example="${docker_dir}/env/world-sim.env.example"

mkdir -p "${env_dir}" "${docker_dir}/local/world-sim/runtime"

if [[ ! -f "${env_file}" ]]; then
  cp "${env_example}" "${env_file}"
  cat <<EOF
Initialized ${env_file} from ${env_example}
Build the packaged images before the first run:
  docker/scripts/run_world_sim.sh build-images
EOF
fi

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"
project_name="${USDB_WORLD_SIM_PROJECT_NAME:-usdb-world-sim}"
startup_timeout_secs="${WORLD_SIM_STARTUP_TIMEOUT_SECS:-180}"

log() {
  printf '[run-world-sim] %s\n' "$*"
}

warn() {
  printf '[run-world-sim] %s\n' "$*" >&2
}

compose() {
  docker compose \
    --project-name "${project_name}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.ord.yml" \
    -f "${docker_dir}/compose.world-sim.yml" \
    "$@"
}

env_get() {
  local key="${1:?key is required}"
  local fallback="${2:-}"
  local value
  if [[ "${!key+x}" == "x" ]]; then
    printf '%s\n' "${!key}"
    return
  fi
  value="$(awk -F= -v key="${key}" '$1 == key { sub(/^[^=]+=*/, "", $0); print $0 }' "${env_file}" | tail -n 1)"
  if [[ -n "${value}" ]]; then
    printf '%s\n' "${value}"
  else
    printf '%s\n' "${fallback}"
  fi
}

ensure_image_exists() {
  local image="${1:?image is required}"
  docker image inspect "${image}" >/dev/null 2>&1 || {
    cat <<EOF >&2
Missing image ${image}

Build the packaged world-sim release images first:
  docker/scripts/run_world_sim.sh build-images
EOF
    exit 1
  }
}

ensure_world_sim_images() {
  ensure_image_exists "$(env_get WORLD_SIM_BITCOIN_IMAGE usdb-bitcoin28-regtest:local)"
  ensure_image_exists "$(env_get WORLD_SIM_TOOLS_IMAGE usdb-world-sim-tools:local)"
  ensure_image_exists "$(env_get ORD_IMAGE usdb-world-sim-tools:local)"
}

state_mode() {
  env_get WORLD_SIM_STATE_MODE persistent
}

identity_seed() {
  env_get WORLD_SIM_IDENTITY_SEED ""
}

host_ord_url() {
  printf 'http://127.0.0.1:%s\n' "$(env_get ORD_SERVER_BIND_PORT 28130)"
}

host_balance_history_url() {
  printf 'http://127.0.0.1:%s\n' "$(env_get BH_BIND_PORT 28110)"
}

host_usdb_indexer_url() {
  printf 'http://127.0.0.1:%s\n' "$(env_get USDB_INDEXER_BIND_PORT 28120)"
}

ethw_identity_mode() {
  env_get ETHW_IDENTITY_MODE deterministic-seed
}

ensure_ethw_identity_defaults() {
  if [[ "$(ethw_identity_mode)" != "deterministic-seed" ]]; then
    return
  fi

  export WORLD_SIM_IDENTITY_SEED="${WORLD_SIM_IDENTITY_SEED:-$(env_get WORLD_SIM_IDENTITY_SEED alpha-seed-1)}"
  export ETHW_IDENTITY_SEED="${ETHW_IDENTITY_SEED:-$(env_get ETHW_IDENTITY_SEED "${WORLD_SIM_IDENTITY_SEED}")}"
}

prepare_state_mode() {
  local mode
  mode="$(state_mode)"
  case "${mode}" in
    persistent)
      ;;
    reset)
      echo "WORLD_SIM_STATE_MODE=reset -> clearing Docker volumes before startup"
      compose down -v --remove-orphans >/dev/null 2>&1 || true
      ;;
    seeded-reset)
      if [[ -z "$(identity_seed)" ]]; then
        echo "WORLD_SIM_STATE_MODE=seeded-reset requires WORLD_SIM_IDENTITY_SEED" >&2
        exit 1
      fi
      echo "WORLD_SIM_STATE_MODE=seeded-reset -> clearing Docker volumes before startup"
      echo "Current implementation will deterministically recreate ord wallet identities from WORLD_SIM_IDENTITY_SEED."
      compose down -v --remove-orphans >/dev/null 2>&1 || true
      ;;
    *)
      echo "Unsupported WORLD_SIM_STATE_MODE=${mode}" >&2
      exit 1
      ;;
  esac
}

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/run_world_sim.sh up
  docker/scripts/run_world_sim.sh up-full
  docker/scripts/run_world_sim.sh build-images
  docker/scripts/run_world_sim.sh doctor
  docker/scripts/run_world_sim.sh ps
  docker/scripts/run_world_sim.sh logs
  docker/scripts/run_world_sim.sh down
  docker/scripts/run_world_sim.sh reset

Modes:
  up       Start the BTC-side local stack plus world-sim, without ethw-node.
           The helper starts in detached mode by default, then waits for core
           readiness and surfaces common persistent-state failures explicitly.
  up-full  Start the same stack and include ethw-node as part of full dev-sim.
  down     Stop the stack but keep Docker volumes and world state.
  reset    Stop the stack and remove Docker volumes to reset world state.
  doctor   Print current startup/readiness diagnostics for the running stack.

Options for up / up-full:
  --foreground  Follow compose logs after startup diagnostics succeed.
  -d, --detach  Explicitly keep detached mode (the default for this helper).
EOF
}

json_rpc_call() {
  local url="${1:?url is required}"
  local method="${2:?method is required}"
  local params="${3:-[]}"

  curl -fsS \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":${params}}" \
    "${url}"
}

readiness_snapshot() {
  local payload="${1:?payload is required}"
  python3 - "${payload}" <<'PY'
import json
import sys

try:
    data = json.loads(sys.argv[1])
except Exception:
    raise SystemExit(1)

result = data.get("result") or {}
print(result.get("service") or "")
print("1" if result.get("rpc_alive") else "0")
print("1" if result.get("query_ready") else "0")
print("1" if result.get("consensus_ready") else "0")
print(",".join(result.get("blockers") or []))
print(result.get("phase") or "")
print(str(result.get("stable_height") if result.get("stable_height") is not None else ""))
print(str(result.get("synced_block_height") if result.get("synced_block_height") is not None else ""))
print(result.get("message") or "")
PY
}

runner_is_running() {
  docker ps --format '{{.Names}}' | grep -qx "${project_name}-world-sim-runner-1"
}

sanitize_up_args() {
  foreground_logs=0
  up_args=()

  while (($#)); do
    case "$1" in
      --foreground)
        foreground_logs=1
        ;;
      --build)
        ;;
      -d|--detach)
        ;;
      --attach|--attach=*|--attach-dependencies|--abort-on-container-exit|--abort-on-container-failure|--menu)
        echo "Unsupported docker compose attach-style option for this helper: $1" >&2
        echo "Use 'docker/scripts/run_world_sim.sh logs' or '--foreground' instead." >&2
        exit 1
        ;;
      *)
        up_args+=("$1")
        ;;
    esac
    shift
  done
}

print_world_sim_doctor() {
  local ord_height=""
  local bh_payload=""
  local usdb_payload=""
  local bh_state=()
  local usdb_state=()

  log "Compose status:"
  compose ps || true
  echo

  ord_height="$(curl -fsS "$(host_ord_url)/blockcount" 2>/dev/null || true)"
  if [[ -n "${ord_height}" ]]; then
    log "ord-server blockcount: ${ord_height}"
  else
    warn "ord-server is not reachable at $(host_ord_url)"
  fi

  bh_payload="$(json_rpc_call "$(host_balance_history_url)" "get_readiness" 2>/dev/null || true)"
  if [[ -n "${bh_payload}" ]]; then
    mapfile -t bh_state < <(readiness_snapshot "${bh_payload}")
    log "balance-history readiness: query=${bh_state[2]:-0} consensus=${bh_state[3]:-0} blockers=${bh_state[4]:-(none)} stable_height=${bh_state[6]:-}"
    if [[ -n "${bh_state[8]:-}" ]]; then
      warn "balance-history message: ${bh_state[8]}"
    fi
  else
    warn "balance-history readiness RPC is not reachable at $(host_balance_history_url)"
  fi

  usdb_payload="$(json_rpc_call "$(host_usdb_indexer_url)" "get_readiness" 2>/dev/null || true)"
  if [[ -n "${usdb_payload}" ]]; then
    mapfile -t usdb_state < <(readiness_snapshot "${usdb_payload}")
    log "usdb-indexer readiness: query=${usdb_state[2]:-0} consensus=${usdb_state[3]:-0} blockers=${usdb_state[4]:-(none)} synced_height=${usdb_state[7]:-}"
    if [[ -n "${usdb_state[8]:-}" ]]; then
      warn "usdb-indexer message: ${usdb_state[8]}"
    fi
  else
    warn "usdb-indexer readiness RPC is not reachable at $(host_usdb_indexer_url)"
  fi

  if runner_is_running; then
    log "world-sim-runner: running"
  else
    warn "world-sim-runner is not running"
  fi
}

print_common_state_recovery_hint() {
  local mode
  mode="$(state_mode)"
  warn "Detected a persistent world-sim state recovery failure."
  if [[ "${mode}" == "persistent" ]]; then
    warn "Current WORLD_SIM_STATE_MODE=persistent; the local volumes appear inconsistent with the BTC / ord state."
    warn "Most direct recovery:"
    warn "  docker/scripts/run_world_sim.sh reset"
    warn "  docker/scripts/run_world_sim.sh up"
    warn "If you want deterministic identities after reset, set WORLD_SIM_STATE_MODE=seeded-reset and WORLD_SIM_IDENTITY_SEED in ${env_file}."
  else
    warn "Current WORLD_SIM_STATE_MODE=${mode}; consider docker/scripts/run_world_sim.sh reset before starting again."
  fi
}

wait_for_world_sim_readiness() {
  local deadline now ord_height bh_payload usdb_payload
  local bh_state=()
  local usdb_state=()
  local bh_blockers="" bh_message="" usdb_blockers=""

  deadline="$(( $(date +%s) + startup_timeout_secs ))"
  while true; do
    ord_height="$(curl -fsS "$(host_ord_url)/blockcount" 2>/dev/null || true)"

    bh_payload="$(json_rpc_call "$(host_balance_history_url)" "get_readiness" 2>/dev/null || true)"
    if [[ -n "${bh_payload}" ]]; then
      mapfile -t bh_state < <(readiness_snapshot "${bh_payload}")
      bh_blockers="${bh_state[4]:-}"
      bh_message="${bh_state[8]:-}"
    else
      bh_state=()
      bh_blockers=""
      bh_message=""
    fi

    usdb_payload="$(json_rpc_call "$(host_usdb_indexer_url)" "get_readiness" 2>/dev/null || true)"
    if [[ -n "${usdb_payload}" ]]; then
      mapfile -t usdb_state < <(readiness_snapshot "${usdb_payload}")
      usdb_blockers="${usdb_state[4]:-}"
    else
      usdb_state=()
      usdb_blockers=""
    fi

    if [[ "${ord_height}" =~ ^[0-9]+$ ]] \
      && [[ "${bh_state[3]:-0}" == "1" ]] \
      && [[ "${usdb_state[3]:-0}" == "1" ]] \
      && runner_is_running; then
      log "world-sim stack is ready: ord=${ord_height}, balance-history=${bh_state[6]:-unknown}, usdb-indexer=${usdb_state[7]:-unknown}"
      return
    fi

    if [[ "${bh_message}" == *"Missing block undo bundle"* ]] || [[ "${bh_blockers}" == *"RollbackInProgress"* ]]; then
      print_world_sim_doctor
      print_common_state_recovery_hint
      echo
      compose logs --tail 30 balance-history world-sim-runner || true
      return 1
    fi

    now="$(date +%s)"
    if (( now >= deadline )); then
      warn "Timed out waiting for world-sim readiness after ${startup_timeout_secs}s."
      print_world_sim_doctor
      if [[ "${bh_blockers}" == *"RollbackInProgress"* ]] || [[ "${usdb_blockers}" == *"UpstreamConsensusNotReady"* ]]; then
        print_common_state_recovery_hint
      fi
      echo
      compose logs --tail 30 balance-history world-sim-runner || true
      return 1
    fi
    sleep 2
  done
}

start_stack() {
  local include_ethw="${1:?include_ethw flag is required}"
  shift

  sanitize_up_args "$@"

  if [[ "${include_ethw}" == "1" ]]; then
    ensure_ethw_identity_defaults
    export ETHW_SIM_PROTOCOL_ALIGNMENT="${ETHW_SIM_PROTOCOL_ALIGNMENT:-1}"
    compose up --build --detach "${up_args[@]}"
  else
    export ETHW_SIM_PROTOCOL_ALIGNMENT="${ETHW_SIM_PROTOCOL_ALIGNMENT:-0}"
    compose up --build --detach btc-node snapshot-loader balance-history usdb-indexer usdb-control-plane ord-server world-sim-runner "${up_args[@]}"
  fi

  wait_for_world_sim_readiness

  if [[ "${foreground_logs}" == "1" ]]; then
    compose logs -f
  else
    log "Use 'docker/scripts/run_world_sim.sh logs' to follow the stack logs."
  fi
}

action="${1:-up}"
shift || true

case "${action}" in
  up)
    ensure_world_sim_images
    prepare_state_mode
    start_stack 0 "$@"
    ;;
  up-full)
    ensure_world_sim_images
    prepare_state_mode
    start_stack 1 "$@"
    ;;
  build-images)
    "${docker_dir}/scripts/build_world_sim_release_images.sh" "$@"
    ;;
  doctor)
    print_world_sim_doctor
    ;;
  ps)
    compose ps "$@"
    ;;
  logs)
    compose logs -f "$@"
    ;;
  down)
    compose down "$@"
    ;;
  reset)
    compose down -v "$@"
    ;;
  *)
    usage
    exit 1
    ;;
esac
