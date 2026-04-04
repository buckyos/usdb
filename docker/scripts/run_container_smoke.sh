#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
docker_dir="$(cd "${script_dir}/.." && pwd)"
repo_root="$(cd "${docker_dir}/.." && pwd)"

require_command() {
  local cmd="${1:?command is required}"
  command -v "${cmd}" >/dev/null 2>&1 || {
    echo "Required command not found: ${cmd}" >&2
    exit 1
  }
}

for cmd in docker curl python3 openssl sha256sum base64 mktemp; do
  require_command "${cmd}"
done

export DOCKER_API_VERSION="${DOCKER_API_VERSION:-1.41}"

compose() {
  docker compose \
    --project-name "${compose_project}" \
    --env-file "${env_file}" \
    -f "${docker_dir}/compose.base.yml" \
    -f "${docker_dir}/compose.dev-sim.yml" \
    -f "${docker_dir}/compose.bootstrap.yml" \
    "$@"
}

log() {
  printf '[smoke] %s\n' "$*"
}

cleanup() {
  if [[ "${KEEP_RUNNING:-0}" == "1" ]]; then
    log "KEEP_RUNNING=1 set; leaving compose project ${compose_project} running"
    log "Temp work dir: ${work_dir}"
    return
  fi

  compose down -v --remove-orphans >/dev/null 2>&1 || true
  rm -rf "${work_dir}"
}

dump_diagnostics() {
  echo
  log "Container smoke failed; dumping compose state"
  compose ps -a || true
  echo
  compose logs --no-color || true
}

trap cleanup EXIT
trap dump_diagnostics ERR

work_dir="$(mktemp -d "/tmp/usdb-docker-smoke.XXXXXX")"
compose_project="usdb-smoke-$(date +%s)"
env_file="${work_dir}/smoke.env"

bootstrap_manifest_dir="${work_dir}/bootstrap/manifests"
bootstrap_keys_dir="${work_dir}/bootstrap/keys"
bootstrap_snapshot_dir="${work_dir}/bootstrap/snapshots"

mkdir -p \
  "${bootstrap_manifest_dir}" \
  "${bootstrap_keys_dir}" \
  "${bootstrap_snapshot_dir}"

genesis_file="${bootstrap_manifest_dir}/ethw-genesis.json"
genesis_manifest_file="${bootstrap_manifest_dir}/ethw-genesis.manifest.json"
genesis_sig_file="${bootstrap_manifest_dir}/ethw-genesis.manifest.sig"
trusted_keys_file="${bootstrap_keys_dir}/trusted_ethw_genesis_keys.json"
signer_private_key="${work_dir}/ethw-genesis-signer.pem"
signer_public_key_der="${work_dir}/ethw-genesis-signer-public.der"
signer_key_id="ethw-genesis-signer-1"

cat >"${genesis_file}" <<'EOF'
{
  "config": {
    "chainId": 20260323
  },
  "difficulty": "0x2000",
  "gasLimit": "0x1c9c380",
  "alloc": {},
  "nonce": "0x0",
  "timestamp": "0x0",
  "extraData": "0x",
  "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
  "coinbase": "0x0000000000000000000000000000000000000000",
  "number": "0x0",
  "gasUsed": "0x0",
  "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000"
}
EOF

genesis_sha256="$(sha256sum "${genesis_file}" | awk '{print $1}')"

cat >"${genesis_manifest_file}" <<EOF
{
  "file_sha256": "${genesis_sha256}",
  "signature_scheme": "ed25519",
  "signing_key_id": "${signer_key_id}"
}
EOF

openssl genpkey -algorithm Ed25519 -out "${signer_private_key}" >/dev/null 2>&1
openssl pkey -in "${signer_private_key}" -pubout -outform DER >"${signer_public_key_der}" 2>/dev/null

public_key_base64="$(
  python3 - "${signer_public_key_der}" <<'PY'
import base64
import pathlib
import sys

path = pathlib.Path(sys.argv[1])
data = path.read_bytes()
if len(data) < 32:
    raise SystemExit("DER public key output shorter than 32 bytes")
print(base64.b64encode(data[-32:]).decode("ascii"))
PY
)"

cat >"${trusted_keys_file}" <<EOF
{
  "keys": [
    {
      "key_id": "${signer_key_id}",
      "public_key_base64": "${public_key_base64}"
    }
  ]
}
EOF

signature_raw="${work_dir}/ethw-genesis.manifest.sig.raw"
openssl pkeyutl \
  -sign \
  -inkey "${signer_private_key}" \
  -rawin \
  -in "${genesis_manifest_file}" \
  -out "${signature_raw}" >/dev/null 2>&1
base64 -w0 "${signature_raw}" >"${genesis_sig_file}"

cat >"${env_file}" <<EOF
USDB_SERVICES_IMAGE=usdb-services:local
ETHW_IMAGE=usdb-services:local
USDB_DOCKER_SCRIPTS_HOST_DIR=${docker_dir}/scripts
USDB_DOCKER_NETWORK=${compose_project}-net

ETHW_COMMAND=trap : TERM INT; while true; do sleep 3600; done
ETHW_INIT_COMMAND=mkdir -p /data/ethw/geth && cp /bootstrap/ethw-genesis.json /data/ethw/geth/bootstrapped-genesis.json
ETHW_DATA_DIR=/data/ethw

BTC_NETWORK=regtest
BTC_RPC_URL=http://btc-node:28132
BTC_RPC_PORT=28132
BTC_NODE_DATA_DIR=/home/bitcoin/.bitcoin
BTC_DATA_DIR=/data/bitcoind
BTC_AUTH_MODE=cookie
BTC_COOKIE_FILE=/data/bitcoind/regtest/.cookie
BTC_RPC_BIND_PORT=0
BTC_P2P_PORT=28133
BTC_P2P_BIND_PORT=0

BH_ROOT_DIR=/data/balance-history
BH_RPC_PORT=28110
BH_BIND_PORT=0
BH_SNAPSHOT_TRUST_MODE=dev
WAIT_FOR_BTC_TIMEOUT_SECS=120

USDB_INDEXER_ROOT_DIR=/data/usdb-indexer
USDB_INDEXER_RPC_PORT=28120
USDB_INDEXER_BIND_PORT=0
USDB_GENESIS_BLOCK_HEIGHT=1
INSCRIPTION_SOURCE=bitcoind

BOOTSTRAP_DIR=/bootstrap
BOOTSTRAP_REQUIRE_ETHW_GENESIS=true
ETHW_BOOTSTRAP_TRUST_MODE=signed
BOOTSTRAP_HOST_DIR=${bootstrap_manifest_dir}
BOOTSTRAP_KEYS_HOST_DIR=${bootstrap_keys_dir}
ETHW_BOOTSTRAP_GENESIS_INPUT_FILE=/bootstrap-input/ethw-genesis.json
ETHW_BOOTSTRAP_GENESIS_MANIFEST_INPUT_FILE=/bootstrap-input/ethw-genesis.manifest.json
ETHW_BOOTSTRAP_GENESIS_SIG_INPUT_FILE=/bootstrap-input/ethw-genesis.manifest.sig
ETHW_BOOTSTRAP_TRUSTED_KEYS_INPUT_FILE=/bootstrap-keys/trusted_ethw_genesis_keys.json
ETHW_CANONICAL_GENESIS_FILE=/bootstrap/ethw-genesis.json
ETHW_CANONICAL_GENESIS_MANIFEST_FILE=/bootstrap/ethw-genesis.manifest.json
ETHW_CANONICAL_GENESIS_SIG_FILE=/bootstrap/ethw-genesis.manifest.sig
ETHW_CANONICAL_TRUSTED_KEYS_FILE=/bootstrap/trusted-ethw-genesis-keys.json
ETHW_HTTP_BIND_PORT=0
ETHW_WS_BIND_PORT=0
ETHW_P2P_BIND_PORT=0
ETHW_RPC_URL=http://ethw-node:8545

CONTROL_PLANE_ROOT_DIR=/data/usdb-control-plane
CONTROL_PLANE_PORT=28140
CONTROL_PLANE_BIND_PORT=0

SNAPSHOT_MODE=none
SNAPSHOT_HOST_DIR=${bootstrap_snapshot_dir}
SNAPSHOT_KEYS_HOST_DIR=${bootstrap_keys_dir}
EOF

json_rpc_call() {
  local url="${1:?url is required}"
  local method="${2:?method is required}"

  curl -fsS \
    -H 'content-type: application/json' \
    --data "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"${method}\",\"params\":[]}" \
    "${url}"
}

json_field() {
  local payload="${1:?payload is required}"
  local path="${2:?path is required}"

  python3 -c '
import json
import sys

path = sys.argv[1].split(".")
data = json.load(sys.stdin)
value = data
for part in path:
    value = value[part]
if isinstance(value, bool):
    print("true" if value else "false")
elif value is None:
    print("null")
else:
    print(value)
' "${path}" <<<"${payload}"
}

wait_until() {
  local description="${1:?description is required}"
  local timeout_secs="${2:?timeout_secs is required}"
  shift 2

  local deadline=$((SECONDS + timeout_secs))
  until "$@"; do
    if (( SECONDS >= deadline )); then
      echo "Timed out waiting for ${description}" >&2
      return 1
    fi
    sleep 2
  done
}

service_exit_code() {
  local service="${1:?service is required}"
  local cid
  cid="$(compose ps -a -q "${service}")"
  [[ -n "${cid}" ]] || {
    echo "No container found for service ${service}" >&2
    return 1
  }
  docker inspect --format '{{.State.ExitCode}}' "${cid}"
}

service_status() {
  local service="${1:?service is required}"
  local cid
  cid="$(compose ps -a -q "${service}")"
  [[ -n "${cid}" ]] || {
    echo "No container found for service ${service}" >&2
    return 1
  }
  docker inspect --format '{{.State.Status}}' "${cid}"
}

published_host_port() {
  local service="${1:?service is required}"
  local container_port="${2:?container_port is required}"
  local mapping
  mapping="$(compose port "${service}" "${container_port}" | head -n1 | tr -d '\r')"
  [[ -n "${mapping}" ]] || {
    echo "No published host port found for ${service}:${container_port}" >&2
    return 1
  }
  printf '%s' "${mapping##*:}"
}

wait_for_published_host_port() {
  local service="${1:?service is required}"
  local container_port="${2:?container_port is required}"
  local timeout_secs="${3:?timeout_secs is required}"
  local deadline=$((SECONDS + timeout_secs))

  while true; do
    if port="$(published_host_port "${service}" "${container_port}" 2>/dev/null)"; then
      printf '%s' "${port}"
      return 0
    fi
    if (( SECONDS >= deadline )); then
      echo "Timed out waiting for a published host port for ${service}:${container_port}" >&2
      return 1
    fi
    sleep 2
  done
}

wait_for_btc_rpc() {
  compose exec -T -u bitcoin btc-node bitcoin-cli \
    "-datadir=${BTC_NODE_DATA_DIR:-/home/bitcoin/.bitcoin}" \
    -regtest \
    "-rpcport=${BTC_RPC_PORT:-28132}" \
    -rpcwait \
    getblockcount >/dev/null 2>&1
}

mine_regtest_blocks() {
  local blocks="${1:?blocks is required}"
  compose exec -T -u bitcoin btc-node bitcoin-cli \
    "-datadir=${BTC_NODE_DATA_DIR:-/home/bitcoin/.bitcoin}" \
    -regtest \
    "-rpcport=${BTC_RPC_PORT:-28132}" \
    -rpcwait \
    createwallet smoke >/dev/null 2>&1 || true
  local address
  address="$(
    compose exec -T -u bitcoin btc-node bitcoin-cli \
      "-datadir=${BTC_NODE_DATA_DIR:-/home/bitcoin/.bitcoin}" \
      -regtest \
      "-rpcport=${BTC_RPC_PORT:-28132}" \
      -rpcwallet=smoke \
      getnewaddress | tr -d '\r'
  )"
  [[ -n "${address}" ]] || {
    echo "Failed to obtain a regtest mining address" >&2
    return 1
  }
  compose exec -T -u bitcoin btc-node bitcoin-cli \
    "-datadir=${BTC_NODE_DATA_DIR:-/home/bitcoin/.bitcoin}" \
    -regtest \
    "-rpcport=${BTC_RPC_PORT:-28132}" \
    -rpcwallet=smoke \
    generatetoaddress "${blocks}" "${address}" >/dev/null
}

balance_history_consensus_ready() {
  local payload
  payload="$(json_rpc_call "http://127.0.0.1:${bh_host_port}" "get_readiness")" || return 1
  [[ "$(json_field "${payload}" "result.consensus_ready")" == "true" ]]
}

usdb_indexer_consensus_ready() {
  local payload
  payload="$(json_rpc_call "http://127.0.0.1:${usdb_indexer_host_port}" "get_readiness")" || return 1
  [[ "$(json_field "${payload}" "result.consensus_ready")" == "true" ]]
}

control_plane_overview_ready() {
  local payload
  payload="$(curl -fsS "http://127.0.0.1:${control_plane_host_port}/api/system/overview")" || return 1
  [[ "$(json_field "${payload}" "service")" == "usdb-control-plane" ]]
}

log "Using temp work dir ${work_dir}"
compose up -d --build

bh_host_port="$(wait_for_published_host_port balance-history 28110 120)"
usdb_indexer_host_port="$(wait_for_published_host_port usdb-indexer 28120 120)"
control_plane_host_port="$(wait_for_published_host_port usdb-control-plane 28140 120)"
log "balance-history published on http://127.0.0.1:${bh_host_port}"
log "usdb-indexer published on http://127.0.0.1:${usdb_indexer_host_port}"
log "usdb-control-plane published on http://127.0.0.1:${control_plane_host_port}"

wait_until "btc-node RPC" 120 wait_for_btc_rpc
mine_regtest_blocks "${SMOKE_REGTEST_BLOCKS:-3}"

wait_until "balance-history consensus readiness" 180 balance_history_consensus_ready
wait_until "usdb-indexer consensus readiness" 180 usdb_indexer_consensus_ready
wait_until "control-plane overview readiness" 180 control_plane_overview_ready

[[ "$(service_exit_code bootstrap-init)" == "0" ]] || {
  echo "bootstrap-init did not exit cleanly" >&2
  exit 1
}
[[ "$(service_exit_code ethw-init)" == "0" ]] || {
  echo "ethw-init did not exit cleanly" >&2
  exit 1
}
[[ "$(service_status ethw-node)" == "running" ]] || {
  echo "ethw-node is not running" >&2
  exit 1
}

compose exec -T ethw-node test -f /bootstrap/bootstrap-manifest.json
compose exec -T ethw-node test -f /data/ethw/bootstrap/ethw-init.done.json

bh_readiness="$(json_rpc_call "http://127.0.0.1:${bh_host_port}" "get_readiness")"
usdb_readiness="$(json_rpc_call "http://127.0.0.1:${usdb_indexer_host_port}" "get_readiness")"
control_plane_overview="$(curl -fsS "http://127.0.0.1:${control_plane_host_port}/api/system/overview")"

log "balance-history readiness: ${bh_readiness}"
log "usdb-indexer readiness: ${usdb_readiness}"
log "control-plane overview: ${control_plane_overview}"
log "Container-level bootstrap smoke succeeded"
