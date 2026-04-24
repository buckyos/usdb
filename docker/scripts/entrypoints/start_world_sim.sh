#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ord_bin="${ORD_BIN:-/opt/ord/bin/ord}"
bitcoin_bin_dir="${BITCOIN_BIN_DIR:-/opt/bitcoin/bin}"
bitcoin_cli="${bitcoin_bin_dir}/bitcoin-cli"
btc_rpc_url="${BTC_RPC_URL:-http://btc-node:28132}"
btc_target="${btc_rpc_url#*://}"
btc_host="${btc_target%%:*}"
btc_port="${btc_target##*:}"
btc_data_dir="${BTC_DATA_DIR:-/data/bitcoind}"
btc_auth_mode="${BTC_AUTH_MODE:-cookie}"
btc_rpc_user="${BTC_RPC_USER:-}"
btc_rpc_password="${BTC_RPC_PASSWORD:-}"
ord_data_dir="${ORD_DATA_DIR:-/data/ord}"
ord_server_url="${ORD_SERVER_URL:-http://ord-server:28130}"
balance_history_rpc_url="${BALANCE_HISTORY_RPC_URL:-http://balance-history:28110}"
usdb_rpc_url="${USDB_RPC_URL:-http://usdb-indexer:28120}"
world_sim_work_dir="${WORLD_SIM_WORK_DIR:-/data/world-sim}"
world_simulator="${WORLD_SIMULATOR:-/opt/usdb/world-sim/regtest_world_simulator.py}"
world_sim_bip39_wordlist="${WORLD_SIM_BIP39_WORDLIST:-/opt/usdb/world-sim/bip39-english.txt}"
cookie_file="${BTC_COOKIE_FILE:-${btc_data_dir}/regtest/.cookie}"
sync_timeout_sec="${SYNC_TIMEOUT_SEC:-300}"
world_sim_mode="${WORLD_SIM_MODE:-all}"
world_sim_state_mode="${WORLD_SIM_STATE_MODE:-persistent}"
world_sim_identity_seed="${WORLD_SIM_IDENTITY_SEED:-}"
world_sim_bootstrap_dir="${WORLD_SIM_BOOTSTRAP_DIR:-${world_sim_work_dir}/bootstrap}"
world_sim_bootstrap_marker="${WORLD_SIM_BOOTSTRAP_MARKER:-${world_sim_bootstrap_dir}/world-sim-bootstrap.done.json}"
world_sim_loop_state_file="${WORLD_SIM_LOOP_STATE_FILE:-${world_sim_bootstrap_dir}/world-sim-loop-state.json}"
world_sim_recovery_state_file="${WORLD_SIM_RECOVERY_STATE_FILE:-${world_sim_bootstrap_dir}/world-sim-recovery-state.json}"
ethw_sim_protocol_alignment="${ETHW_SIM_PROTOCOL_ALIGNMENT:-0}"
ethw_data_dir="${ETHW_DATA_DIR:-/data/ethw}"
ethw_identity_marker="${ETHW_IDENTITY_MARKER:-${ethw_data_dir}/bootstrap/ethw-sim-identity.json}"
ethw_identity_marker_wait_secs="${ETHW_IDENTITY_MARKER_WAIT_SECS:-60}"
ethw_identity_mode="${ETHW_IDENTITY_MODE:-none}"
ethw_identity_seed="${ETHW_IDENTITY_SEED:-${world_sim_identity_seed}}"
ethw_miner_address_override="${ETHW_MINER_ADDRESS:-}"
ethw_miner_agent_id="${ETHW_MINER_AGENT_ID:-0}"
resolved_ethw_miner_address=""

miner_wallet_name="${MINER_WALLET_NAME:-usdb-world-miner}"
wallet_prefix="${ORD_WALLET_PREFIX:-usdb-world-agent}"
agent_count="${AGENT_COUNT:-5}"
premine_blocks="${PREMINE_BLOCKS:-140}"
fund_agent_amount_btc="${FUND_AGENT_AMOUNT_BTC:-4.0}"
fund_confirm_blocks="${FUND_CONFIRM_BLOCKS:-2}"
sim_blocks="${SIM_BLOCKS:-300}"
sim_base_seed="${SIM_SEED:-42}"
sim_loop_batch_blocks="${SIM_LOOP_BATCH_BLOCKS:-25}"
ord_stability_probes="${WORLD_SIM_ORD_STABILITY_PROBES:-2}"
ord_stability_sleep_secs="${WORLD_SIM_ORD_STABILITY_SLEEP_SECS:-1}"

identity_scheme="legacy-random-v1"
if [[ -n "${world_sim_identity_seed}" ]]; then
  identity_scheme="ord-mnemonic-v1"
fi

log() {
  printf '[docker-world-sim] %s\n' "$*"
}

require_file() {
  local path="${1:?path is required}"
  local label="${2:?label is required}"
  [[ -e "${path}" ]] || {
    echo "Missing ${label}: ${path}" >&2
    exit 1
  }
}

require_executable() {
  local path="${1:?path is required}"
  local label="${2:?label is required}"
  [[ -x "${path}" ]] || {
    echo "Missing executable ${label}: ${path}" >&2
    exit 1
  }
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

json_result_u32() {
  python3 -c 'import json,sys; print(int(json.load(sys.stdin).get("result", 0)))'
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

rpc_consensus_ready() {
  local url="${1:?url is required}"
  json_rpc_call "${url}" "get_readiness" \
    | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit(0)
r = d.get("result") or {}
print(1 if r.get("consensus_ready") else 0)'
}

wait_rpc_ready() {
  local service_name="${1:?service is required}"
  local url="${2:?url is required}"
  local method="${3:?method is required}"
  local params="${4:-[]}"
  local attempts="${5:-240}"

  log "Waiting for ${service_name} RPC at ${url}"
  for _ in $(seq 1 "${attempts}"); do
    if json_rpc_call "${url}" "${method}" "${params}" >/dev/null 2>&1; then
      return
    fi
    sleep 0.5
  done
  echo "${service_name} RPC is not ready at ${url}" >&2
  exit 1
}

wait_http_ready() {
  local service_name="${1:?service is required}"
  local url="${2:?url is required}"
  local attempts="${3:-240}"

  log "Waiting for ${service_name} HTTP readiness at ${url}"
  for _ in $(seq 1 "${attempts}"); do
    if curl -fsS "${url}" >/dev/null 2>&1; then
      return
    fi
    sleep 0.5
  done
  echo "${service_name} is not ready at ${url}" >&2
  exit 1
}

btc_cli() {
  local args=(
    -regtest
    -datadir="${btc_data_dir}"
    -rpcconnect="${btc_host}"
    -rpcport="${btc_port}"
  )
  case "${btc_auth_mode}" in
    cookie)
      args+=(-rpccookiefile="${cookie_file}")
      ;;
    userpass)
      : "${btc_rpc_user:?BTC_RPC_USER is required when BTC_AUTH_MODE=userpass}"
      : "${btc_rpc_password:?BTC_RPC_PASSWORD is required when BTC_AUTH_MODE=userpass}"
      args+=(-rpcuser="${btc_rpc_user}" -rpcpassword="${btc_rpc_password}")
      ;;
    *)
      echo "Unsupported BTC_AUTH_MODE=${btc_auth_mode}" >&2
      exit 1
      ;;
  esac
  "${bitcoin_cli}" "${args[@]}" "$@"
}

wait_for_bitcoin_rpc() {
  local timeout_secs="${WAIT_FOR_BTC_TIMEOUT_SECS:-120}"
  local start_ts now

  log "Waiting for authenticated bitcoind RPC at ${btc_rpc_url}"
  start_ts="$(date +%s)"
  while true; do
    if btc_cli getblockcount >/dev/null 2>&1; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > timeout_secs )); then
      echo "Timed out waiting for authenticated bitcoind RPC at ${btc_rpc_url}" >&2
      exit 1
    fi
    sleep 1
  done
}

wait_until_ord_server_synced_to_bitcoind() {
  local start_ts now ord_height btc_height
  start_ts="$(date +%s)"
  while true; do
    btc_height="$(btc_cli getblockcount 2>/dev/null || echo 0)"
    ord_height="$(curl -fsS "${ord_server_url}/blockcount" 2>/dev/null | tr -d '\n\r ' || echo 0)"
    if [[ "${ord_height}" =~ ^[0-9]+$ ]] && [[ "${btc_height}" =~ ^[0-9]+$ ]] && [[ "${ord_height}" -ge "${btc_height}" ]]; then
      return
    fi
    now="$(date +%s)"
    if (( now - start_ts > sync_timeout_sec )); then
      echo "ord server sync timeout: ord_height=${ord_height:-unknown}, btc_height=${btc_height:-unknown}" >&2
      exit 1
    fi
    sleep 1
  done
}

wait_until_balance_history_synced() {
  local target_height="${1:?target height is required}"
  local start_ts now resp synced consensus_ready
  start_ts="$(date +%s)"
  while true; do
    resp="$(json_rpc_call "${balance_history_rpc_url}" "get_block_height" || true)"
    synced="$(echo "${resp}" | json_result_u32 2>/dev/null || true)"
    synced="${synced:-0}"
    consensus_ready="$(rpc_consensus_ready "${balance_history_rpc_url}" 2>/dev/null || echo 0)"
    if [[ "${synced}" -ge "${target_height}" ]] && [[ "${consensus_ready}" == "1" ]]; then
      return
    fi
    now="$(date +%s)"
    if (( now - start_ts > sync_timeout_sec )); then
      echo "balance-history sync timeout, target=${target_height}, last=${resp}" >&2
      exit 1
    fi
    sleep 1
  done
}

wait_until_usdb_synced() {
  local target_height="${1:?target height is required}"
  local start_ts now resp synced consensus_ready
  start_ts="$(date +%s)"
  while true; do
    resp="$(json_rpc_call "${usdb_rpc_url}" "get_synced_block_height" || true)"
    synced="$(echo "${resp}" | python3 -c 'import json,sys
try:
    d = json.load(sys.stdin)
except Exception:
    print(0)
    raise SystemExit(0)
res = d.get("result")
print(int(res) if res is not None else 0)
' 2>/dev/null || true)"
    synced="${synced:-0}"
    consensus_ready="$(rpc_consensus_ready "${usdb_rpc_url}" 2>/dev/null || echo 0)"
    if [[ "${synced}" -ge "${target_height}" ]] && [[ "${consensus_ready}" == "1" ]]; then
      return
    fi
    now="$(date +%s)"
    if (( now - start_ts > sync_timeout_sec )); then
      echo "usdb-indexer sync timeout, target=${target_height}, last=${resp}" >&2
      exit 1
    fi
    sleep 1
  done
}

run_ord() {
  local args=(
    --regtest
    --bitcoin-rpc-url "${btc_rpc_url}"
    --bitcoin-data-dir "${btc_data_dir}"
    --data-dir "${ord_data_dir}"
  )
  case "${btc_auth_mode}" in
    cookie)
      args+=(--cookie-file "${cookie_file}")
      ;;
    userpass)
      args+=(--bitcoin-rpc-username "${btc_rpc_user}" --bitcoin-rpc-password "${btc_rpc_password}")
      ;;
  esac
  "${ord_bin}" "${args[@]}" "$@"
}

run_ord_wallet_named() {
  local wallet_name="${1:?wallet name is required}"
  shift
  run_ord wallet \
    --no-sync \
    --server-url "${ord_server_url}" \
    --name "${wallet_name}" \
    "$@"
}

extract_bech32_address() {
  local raw="${1:?raw output is required}"
  python3 - "${raw}" <<'PY'
import re
import sys

raw = sys.argv[1]
m = re.search(r"(bc1|tb1|bcrt1)[ac-hj-np-z02-9]{20,}", raw)
if m:
    print(m.group(0))
PY
}

ensure_wallet_loaded() {
  local wallet_name="${1:?wallet name is required}"
  local timeout_secs="${WALLET_READY_TIMEOUT_SECS:-60}"
  local start_ts now

  log "Ensuring BTC wallet is loaded: ${wallet_name}"
  start_ts="$(date +%s)"
  while true; do
    if btc_cli -rpcwallet="${wallet_name}" getwalletinfo >/dev/null 2>&1; then
      return
    fi

    btc_cli -named createwallet wallet_name="${wallet_name}" load_on_startup=true >/dev/null 2>&1 || true
    btc_cli loadwallet "${wallet_name}" >/dev/null 2>&1 || true

    if btc_cli -rpcwallet="${wallet_name}" getwalletinfo >/dev/null 2>&1; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > timeout_secs )); then
      echo "Timed out waiting for wallet to load: ${wallet_name}" >&2
      exit 1
    fi
    sleep 1
  done
}

ord_wallet_descriptor_state() {
  local wallet_name="${1:?wallet name is required}"
  local output

  output="$(btc_cli -rpcwallet="${wallet_name}" listdescriptors true 2>/dev/null || true)"
  python3 - "${output}" <<'PY'
import json
import sys

try:
    payload = json.loads(sys.argv[1] or "{}")
except Exception:
    print("invalid")
    raise SystemExit(0)

descriptors = payload.get("descriptors") or []
tr_count = sum(1 for item in descriptors if (item.get("desc") or "").startswith("tr("))
rawtr_count = sum(1 for item in descriptors if (item.get("desc") or "").startswith("rawtr("))

if tr_count == 2 and len(descriptors) == 2 + rawtr_count:
    print("ord")
elif descriptors:
    print("unexpected")
else:
    print("empty")
PY
}

reset_wallet_identity_artifacts() {
  local wallet_name="${1:?wallet name is required}"

  btc_cli unloadwallet "${wallet_name}" >/dev/null 2>&1 || true
  rm -rf \
    "${btc_data_dir}/regtest/wallets/${wallet_name}" \
    "${btc_data_dir}/wallets/${wallet_name}" \
    "${ord_data_dir}/wallets/${wallet_name}.redb"
}

join_by_comma() {
  local IFS=","
  echo "$*"
}

retry_until_success() {
  local label="${1:?label is required}"
  local timeout_secs="${2:?timeout is required}"
  shift 2

  local start_ts now
  start_ts="$(date +%s)"
  while true; do
    if "$@" >/dev/null 2>&1; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > timeout_secs )); then
      echo "Timed out waiting for ${label}" >&2
      exit 1
    fi
    sleep 1
  done
}

retry_capture_output() {
  local label="${1:?label is required}"
  local timeout_secs="${2:?timeout is required}"
  shift 2

  local start_ts now output
  start_ts="$(date +%s)"
  while true; do
    if output="$("$@" 2>&1)"; then
      printf '%s\n' "${output}"
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > timeout_secs )); then
      echo "Timed out waiting for ${label}: ${output}" >&2
      exit 1
    fi
    sleep 1
  done
}

wallet_mnemonic_for() {
  local wallet_name="${1:?wallet name is required}"
  python3 - "${world_sim_bip39_wordlist}" "${world_sim_identity_seed}" "${wallet_name}" <<'PY'
import hashlib
import sys

wordlist_path, seed, wallet = sys.argv[1:]
with open(wordlist_path, "r", encoding="utf-8") as fp:
    words = [line.strip() for line in fp if line.strip()]

if len(words) != 2048:
    raise SystemExit(f"expected 2048 BIP39 words, got {len(words)}")

entropy = hashlib.sha256(f"usdb-world-sim:{seed}:{wallet}".encode("utf-8")).digest()[:16]
checksum_bits = len(entropy) * 8 // 32
checksum_byte = hashlib.sha256(entropy).digest()[0]
bitstr = "".join(f"{b:08b}" for b in entropy) + f"{checksum_byte:08b}"[:checksum_bits]
indices = [int(bitstr[i : i + 11], 2) for i in range(0, len(bitstr), 11)]
print(" ".join(words[index] for index in indices))
PY
}

ensure_ord_wallet_ready() {
  local wallet_name="${1:?wallet name is required}"
  local timeout_secs="${2:?timeout is required}"
  local passphrase="${3:-}"
  local start_ts now output output_lower

  start_ts="$(date +%s)"
  while true; do
    if [[ -n "${passphrase}" ]]; then
      if output="$(run_ord_wallet_named "${wallet_name}" create --passphrase "${passphrase}" 2>&1)"; then
        return
      fi
    else
      if output="$(run_ord_wallet_named "${wallet_name}" create 2>&1)"; then
        return
      fi
    fi

    output_lower="$(printf '%s' "${output}" | tr '[:upper:]' '[:lower:]')"
    if [[ "${output_lower}" == *"already exists"* ]]; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > timeout_secs )); then
      echo "Timed out waiting for ord wallet ${wallet_name}: ${output}" >&2
      exit 1
    fi
    sleep 1
  done
}

restore_ord_wallet_from_mnemonic() {
  local wallet_name="${1:?wallet name is required}"
  local timeout_secs="${2:?timeout is required}"
  local mnemonic="${3:?mnemonic is required}"
  local start_ts now output output_lower

  start_ts="$(date +%s)"
  while true; do
    if output="$(printf '%s\n' "${mnemonic}" | run_ord_wallet_named "${wallet_name}" restore --from mnemonic 2>&1)"; then
      return
    fi

    output_lower="$(printf '%s' "${output}" | tr '[:upper:]' '[:lower:]')"
    if [[ "${output_lower}" == *"already exists"* ]]; then
      return
    fi

    now="$(date +%s)"
    if (( now - start_ts > timeout_secs )); then
      echo "Timed out waiting to restore ord wallet ${wallet_name}: ${output}" >&2
      exit 1
    fi
    sleep 1
  done
}

prepare_runtime_environment() {
  require_executable "${ord_bin}" "ord binary"
  require_executable "${bitcoin_cli}" "bitcoin-cli"
  if [[ "${btc_auth_mode}" == "cookie" ]]; then
    require_file "${cookie_file}" "Bitcoin cookie file"
  fi
  require_file "${world_simulator}" "world simulator"
  if [[ -n "${world_sim_identity_seed}" ]]; then
    require_file "${world_sim_bip39_wordlist}" "BIP39 wordlist"
  fi
  mkdir -p "${world_sim_work_dir}" "${ord_data_dir}" "${world_sim_bootstrap_dir}"
  case "${world_sim_state_mode}" in
    persistent|reset|seeded-reset)
      ;;
    *)
      echo "Unsupported WORLD_SIM_STATE_MODE=${world_sim_state_mode}" >&2
      exit 1
      ;;
  esac
  if [[ "${world_sim_state_mode}" == "seeded-reset" && -z "${world_sim_identity_seed}" ]]; then
    echo "WORLD_SIM_STATE_MODE=seeded-reset requires WORLD_SIM_IDENTITY_SEED" >&2
    exit 1
  fi
  case "${ethw_sim_protocol_alignment}" in
    0|1)
      ;;
    *)
      echo "ETHW_SIM_PROTOCOL_ALIGNMENT must be 0 or 1" >&2
      exit 1
      ;;
  esac
  if ! [[ "${ethw_miner_agent_id}" =~ ^[0-9]+$ ]]; then
    echo "ETHW_MINER_AGENT_ID must be a non-negative integer" >&2
    exit 1
  fi
}

wait_core_services() {
  "${script_dir}/../helpers/wait_for_tcp.sh" "${btc_host}" "${btc_port}" "${WAIT_FOR_BTC_TIMEOUT_SECS:-120}"
  wait_for_bitcoin_rpc
  wait_http_ready "ord-server" "${ord_server_url}/blockcount"
  wait_rpc_ready "balance-history" "${balance_history_rpc_url}" "get_network_type"
  wait_rpc_ready "usdb-indexer" "${usdb_rpc_url}" "get_network_type"
}

bootstrap_marker_matches() {
  [[ -f "${world_sim_bootstrap_marker}" ]] || return 1
  python3 - "${world_sim_bootstrap_marker}" \
    "${miner_wallet_name}" \
    "${wallet_prefix}" \
    "${agent_count}" \
    "${premine_blocks}" \
    "${fund_agent_amount_btc}" \
    "${fund_confirm_blocks}" \
    "${world_sim_state_mode}" \
    "${world_sim_identity_seed}" <<'PY'
import json
import sys

(
    marker_path,
    miner_wallet_name,
    wallet_prefix,
    agent_count,
    premine_blocks,
    fund_agent_amount_btc,
    fund_confirm_blocks,
    world_sim_state_mode,
    world_sim_identity_seed,
) = sys.argv[1:]
with open(marker_path, "r", encoding="utf-8") as fp:
    data = json.load(fp)

expected = {
    "miner_wallet_name": miner_wallet_name,
    "wallet_prefix": wallet_prefix,
    "agent_count": int(agent_count),
    "premine_blocks": int(premine_blocks),
    "fund_agent_amount_btc": str(fund_agent_amount_btc),
    "fund_confirm_blocks": int(fund_confirm_blocks),
    "state_mode": world_sim_state_mode,
    "identity_seed": world_sim_identity_seed,
    "identity_scheme": "ord-mnemonic-v1" if world_sim_identity_seed else "legacy-random-v1",
}

for key, value in expected.items():
    if data.get(key) != value:
        raise SystemExit(1)
PY
  python3 - "${world_sim_bootstrap_marker}" \
    "${ethw_sim_protocol_alignment}" \
    "${resolved_ethw_miner_address}" \
    "${ethw_miner_agent_id}" <<'PY'
import json
import sys

marker_path, ethw_sim_protocol_alignment, resolved_ethw_miner_address, ethw_miner_agent_id = sys.argv[1:]
with open(marker_path, "r", encoding="utf-8") as fp:
    data = json.load(fp)

expected = {
    "ethw_protocol_alignment": ethw_sim_protocol_alignment == "1",
    "ethw_miner_address": resolved_ethw_miner_address,
    "ethw_miner_agent_id": int(ethw_miner_agent_id),
}

for key, value in expected.items():
    actual = data.get(
        key,
        False if key == "ethw_protocol_alignment" else (0 if key == "ethw_miner_agent_id" else ""),
    )
    if actual != value:
        raise SystemExit(1)
PY
}

load_bootstrap_state() {
  [[ -f "${world_sim_bootstrap_marker}" ]] || {
    echo "Missing world-sim bootstrap marker: ${world_sim_bootstrap_marker}" >&2
    exit 1
  }

  mapfile -t bootstrap_state < <(
    python3 - "${world_sim_bootstrap_marker}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fp:
    data = json.load(fp)

print(data.get("mining_address", ""))
print(",".join(data.get("agent_wallets", [])))
print(",".join(data.get("agent_addresses", [])))
print(str(data.get("bootstrap_height", 0)))
PY
  )

  mining_address="${bootstrap_state[0]:-}"
  agent_wallets_csv="${bootstrap_state[1]:-}"
  agent_addresses_csv="${bootstrap_state[2]:-}"
  bootstrap_height="${bootstrap_state[3]:-0}"

  [[ -n "${mining_address}" ]] || {
    echo "Bootstrap marker missing mining_address: ${world_sim_bootstrap_marker}" >&2
    exit 1
  }
  [[ -n "${agent_wallets_csv}" ]] || {
    echo "Bootstrap marker missing agent_wallets: ${world_sim_bootstrap_marker}" >&2
    exit 1
  }
  [[ -n "${agent_addresses_csv}" ]] || {
    echo "Bootstrap marker missing agent_addresses: ${world_sim_bootstrap_marker}" >&2
    exit 1
  }
}

write_bootstrap_marker() {
  local current_height="${1:?current height is required}"
  local agent_wallets_csv="${2:?agent wallets are required}"
  local agent_addresses_csv="${3:?agent addresses are required}"

  python3 - "${world_sim_bootstrap_marker}" \
    "${mining_address}" \
    "${miner_wallet_name}" \
    "${wallet_prefix}" \
    "${agent_count}" \
    "${premine_blocks}" \
    "${fund_agent_amount_btc}" \
    "${fund_confirm_blocks}" \
    "${world_sim_state_mode}" \
    "${world_sim_identity_seed}" \
    "${identity_scheme}" \
    "${ethw_sim_protocol_alignment}" \
    "${resolved_ethw_miner_address}" \
    "${ethw_miner_agent_id}" \
    "${current_height}" \
    "${agent_wallets_csv}" \
    "${agent_addresses_csv}" <<'PY'
import json
import os
import sys
import time

(
    marker_path,
    mining_address,
    miner_wallet_name,
    wallet_prefix,
    agent_count,
    premine_blocks,
    fund_agent_amount_btc,
    fund_confirm_blocks,
    world_sim_state_mode,
    world_sim_identity_seed,
    identity_scheme,
    ethw_sim_protocol_alignment,
    resolved_ethw_miner_address,
    ethw_miner_agent_id,
    current_height,
    agent_wallets_csv,
    agent_addresses_csv,
) = sys.argv[1:]

payload = {
    "version": 1,
    "created_at": int(time.time()),
    "mining_address": mining_address,
    "miner_wallet_name": miner_wallet_name,
    "wallet_prefix": wallet_prefix,
    "agent_count": int(agent_count),
    "premine_blocks": int(premine_blocks),
    "fund_agent_amount_btc": str(fund_agent_amount_btc),
    "fund_confirm_blocks": int(fund_confirm_blocks),
    "state_mode": world_sim_state_mode,
    "identity_seed": world_sim_identity_seed,
    "identity_scheme": identity_scheme,
    "ethw_protocol_alignment": ethw_sim_protocol_alignment == "1",
    "ethw_miner_address": resolved_ethw_miner_address,
    "ethw_miner_agent_id": int(ethw_miner_agent_id),
    "bootstrap_height": int(current_height),
    "agent_wallets": [v for v in agent_wallets_csv.split(",") if v],
    "agent_addresses": [v for v in agent_addresses_csv.split(",") if v],
}

os.makedirs(os.path.dirname(marker_path), exist_ok=True)
with open(marker_path, "w", encoding="utf-8") as fp:
    json.dump(payload, fp, indent=2, sort_keys=True)
    fp.write("\n")
PY
}

wait_until_ord_wallet_stable() {
  local wallet_name="${1:?wallet name is required}"
  local probe="1"
  local timeout_secs="${ORD_WALLET_READY_TIMEOUT_SECS:-60}"

  for probe in $(seq 1 "${ord_stability_probes}"); do
    retry_until_success "ord wallet balance ${wallet_name} (probe ${probe}/${ord_stability_probes})" "${timeout_secs}" \
      run_ord_wallet_named "${wallet_name}" balance
    if [[ "${probe}" -lt "${ord_stability_probes}" ]]; then
      sleep "${ord_stability_sleep_secs}"
    fi
  done
}

validate_eth_address() {
  local address="${1:?address is required}"
  [[ "${address}" =~ ^0x[0-9a-fA-F]{40}$ ]] || {
    echo "Invalid ETH address format: ${address}" >&2
    exit 1
  }
}

resolve_ethw_protocol_alignment() {
  local address="" deadline now

  resolved_ethw_miner_address=""
  if [[ "${ethw_sim_protocol_alignment}" != "1" ]]; then
    return 0
  fi

  if [[ -n "${ethw_miner_address_override}" ]]; then
    validate_eth_address "${ethw_miner_address_override}"
    resolved_ethw_miner_address="${ethw_miner_address_override}"
    log "Using ETHW miner alignment address from environment: ${resolved_ethw_miner_address}"
    return 0
  fi

  deadline="$(( $(date +%s) + ethw_identity_marker_wait_secs ))"
  while true; do
    if [[ -f "${ethw_identity_marker}" ]]; then
      address="$(json_read_field "${ethw_identity_marker}" "ethw_miner_address")"
      if [[ -n "${address}" ]]; then
        validate_eth_address "${address}"
        resolved_ethw_miner_address="${address}"
        log "Using ETHW miner alignment address from marker ${ethw_identity_marker}: ${resolved_ethw_miner_address}"
        return 0
      fi
    fi
    now="$(date +%s)"
    if (( now >= deadline )); then
      echo "ETHW protocol alignment is enabled, but ETHW identity marker is unavailable or incomplete: ${ethw_identity_marker}" >&2
      exit 1
    fi
    sleep 1
  done
}

wait_until_ord_runtime_stable() {
  local current_height="${1:?current height is required}"

  ensure_wallet_loaded "${miner_wallet_name}"
  wait_until_ord_server_synced_to_bitcoind
  wait_until_balance_history_synced "${current_height}"
  wait_until_usdb_synced "${current_height}"

  IFS=',' read -r -a stable_wallets <<< "${agent_wallets_csv:-}"
  for wallet_name in "${stable_wallets[@]}"; do
    [[ -n "${wallet_name}" ]] || continue
    ensure_wallet_loaded "${wallet_name}"
    wait_until_ord_wallet_stable "${wallet_name}"
  done
}

prepare_deterministic_wallet_identity() {
  local wallet_name="${1:?wallet name is required}"
  local timeout_secs="${2:?timeout is required}"
  local mnemonic
  local descriptor_state
  local attempt
  local probe

  mnemonic="$(wallet_mnemonic_for "${wallet_name}")"

  for attempt in $(seq 1 5); do
    if [[ "${attempt}" -gt 1 ]]; then
      log "Retrying deterministic wallet restore for ${wallet_name} (attempt ${attempt}/5)"
      reset_wallet_identity_artifacts "${wallet_name}"
      sleep 1
    fi

    restore_ord_wallet_from_mnemonic "${wallet_name}" "${timeout_secs}" "${mnemonic}"
    ensure_wallet_loaded "${wallet_name}"

    descriptor_state="invalid"
    for probe in $(seq 1 "${timeout_secs}"); do
      descriptor_state="$(ord_wallet_descriptor_state "${wallet_name}")"
      if [[ "${descriptor_state}" == "ord" ]]; then
        return
      fi
      sleep 1
    done

    if [[ "${descriptor_state}" == "ord" ]]; then
      return
    fi

    log "Deterministic wallet restore produced ${descriptor_state} descriptors for ${wallet_name}; resetting and retrying"
  done

  echo "Failed to establish deterministic ord wallet identity for ${wallet_name} after repeated restore attempts" >&2
  exit 1
}

read_completed_batches() {
  if [[ ! -f "${world_sim_loop_state_file}" ]]; then
    echo 0
    return
  fi
  python3 - "${world_sim_loop_state_file}" <<'PY'
import json
import sys

with open(sys.argv[1], "r", encoding="utf-8") as fp:
    data = json.load(fp)
print(int(data.get("completed_batches", 0)))
PY
}

write_loop_state() {
  local completed_batches="${1:?completed_batches is required}"
  local batch_seed="${2:?batch_seed is required}"
  local current_height="${3:?current_height is required}"

  python3 - "${world_sim_loop_state_file}" \
    "${completed_batches}" \
    "${batch_seed}" \
    "${current_height}" \
    "${sim_loop_batch_blocks}" \
    "${world_sim_state_mode}" \
    "${world_sim_identity_seed}" \
    "${identity_scheme}" <<'PY'
import json
import os
import sys
import time

(
    loop_state_path,
    completed_batches,
    batch_seed,
    current_height,
    batch_blocks,
    world_sim_state_mode,
    world_sim_identity_seed,
    identity_scheme,
) = sys.argv[1:]
payload = {
    "version": 1,
    "updated_at": int(time.time()),
    "completed_batches": int(completed_batches),
    "last_batch_seed": int(batch_seed),
    "last_block_height": int(current_height),
    "batch_blocks": int(batch_blocks),
    "state_mode": world_sim_state_mode,
    "identity_seed": world_sim_identity_seed,
    "identity_scheme": identity_scheme,
}
os.makedirs(os.path.dirname(loop_state_path), exist_ok=True)
with open(loop_state_path, "w", encoding="utf-8") as fp:
    json.dump(payload, fp, indent=2, sort_keys=True)
    fp.write("\n")
PY
}

bootstrap_world_sim() {
  resolve_ethw_protocol_alignment
  if [[ -f "${world_sim_bootstrap_marker}" ]]; then
    if bootstrap_marker_matches; then
      log "World-sim bootstrap already completed: ${world_sim_bootstrap_marker}"
      load_bootstrap_state
      return
    fi
    echo "Existing world-sim bootstrap marker does not match current bootstrap config: ${world_sim_bootstrap_marker}" >&2
    exit 1
  fi

  if [[ -n "${world_sim_identity_seed}" ]]; then
    prepare_deterministic_wallet_identity "${miner_wallet_name}" "${ORD_WALLET_READY_TIMEOUT_SECS:-60}"
  else
    ensure_wallet_loaded "${miner_wallet_name}"
  fi

  log "Allocating mining address from wallet: ${miner_wallet_name}"
  if [[ -n "${world_sim_identity_seed}" ]]; then
    mining_receive_output="$(retry_capture_output "ord wallet receive ${miner_wallet_name}" "${ORD_WALLET_READY_TIMEOUT_SECS:-60}" run_ord_wallet_named "${miner_wallet_name}" receive)"
    mining_address="$(extract_bech32_address "${mining_receive_output}")"
    if [[ -z "${mining_address}" ]]; then
      echo "Failed to parse mining address: wallet=${miner_wallet_name}, output=${mining_receive_output}" >&2
      exit 1
    fi
  else
    mining_address="$(btc_cli -rpcwallet="${miner_wallet_name}" getnewaddress)"
  fi
  current_height="$(btc_cli getblockcount)"

  if [[ "${current_height}" -lt "${premine_blocks}" ]]; then
    blocks_to_mine="$(( premine_blocks - current_height ))"
    log "Premining ${blocks_to_mine} blocks to reach height ${premine_blocks}"
    btc_cli -rpcwallet="${miner_wallet_name}" generatetoaddress "${blocks_to_mine}" "${mining_address}" >/dev/null
  else
    log "Skipping premine: current_height=${current_height} already >= ${premine_blocks}"
  fi

  wait_until_ord_server_synced_to_bitcoind

  declare -a agent_wallets=()
  declare -a agent_addresses=()

  log "Preparing ${agent_count} world-sim agent wallets"
  for i in $(seq 1 "${agent_count}"); do
    wallet_name="${wallet_prefix}-${i}"
    agent_wallets+=("${wallet_name}")
    log "Preparing ord wallet ${wallet_name}"
    if [[ -n "${world_sim_identity_seed}" ]]; then
      prepare_deterministic_wallet_identity "${wallet_name}" "${ORD_WALLET_READY_TIMEOUT_SECS:-60}"
    else
      ensure_ord_wallet_ready "${wallet_name}" "${ORD_WALLET_READY_TIMEOUT_SECS:-60}"
    fi
    receive_output="$(retry_capture_output "ord wallet receive ${wallet_name}" "${ORD_WALLET_READY_TIMEOUT_SECS:-60}" run_ord_wallet_named "${wallet_name}" receive)"
    receive_address="$(extract_bech32_address "${receive_output}")"
    if [[ -z "${receive_address}" ]]; then
      echo "Failed to parse receive address: wallet=${wallet_name}, output=${receive_output}" >&2
      exit 1
    fi
    agent_addresses+=("${receive_address}")

    btc_cli -rpcwallet="${miner_wallet_name}" sendtoaddress "${receive_address}" "${fund_agent_amount_btc}" >/dev/null
  done

  log "Confirming agent funding with ${fund_confirm_blocks} blocks"
  btc_cli -rpcwallet="${miner_wallet_name}" generatetoaddress "${fund_confirm_blocks}" "${mining_address}" >/dev/null

  current_height="$(btc_cli getblockcount)"

  agent_wallets_csv="$(join_by_comma "${agent_wallets[@]}")"
  agent_addresses_csv="$(join_by_comma "${agent_addresses[@]}")"
  wait_until_ord_runtime_stable "${current_height}"
  write_bootstrap_marker "${current_height}" "${agent_wallets_csv}" "${agent_addresses_csv}"
  log "Wrote world-sim bootstrap marker: ${world_sim_bootstrap_marker}"
}

run_simulator_batch() {
  local batch_blocks="${1:?batch_blocks is required}"
  local batch_seed="${2:?batch_seed is required}"

  fail_fast_arg=()
  if [[ "${SIM_FAIL_FAST:-0}" == "1" ]]; then
    fail_fast_arg+=(--fail-fast)
  fi

  report_args=()
  if [[ "${SIM_REPORT_ENABLED:-1}" == "1" ]]; then
    report_args+=(--report-file "${WORLD_SIM_REPORT_FILE:-${world_sim_work_dir}/world-sim-report.jsonl}")
    report_args+=(--report-flush-every "${SIM_REPORT_FLUSH_EVERY:-1}")
  fi

  self_check_args=()
  if [[ "${SIM_AGENT_SELF_CHECK_ENABLED:-1}" != "1" ]]; then
    self_check_args+=(--disable-agent-self-check)
  else
    self_check_args+=(--agent-self-check-interval-blocks "${SIM_AGENT_SELF_CHECK_INTERVAL_BLOCKS:-1}")
    self_check_args+=(--agent-self-check-sample-size "${SIM_AGENT_SELF_CHECK_SAMPLE_SIZE:-0}")
  fi

  global_cross_check_args=()
  if [[ "${SIM_GLOBAL_CROSS_CHECK_ENABLED:-1}" != "1" ]]; then
    global_cross_check_args+=(--disable-global-cross-check)
  else
    global_cross_check_args+=(
      --global-cross-check-interval-blocks "${SIM_GLOBAL_CROSS_CHECK_INTERVAL_BLOCKS:-20}"
      --global-cross-check-leaderboard-top-n "${SIM_GLOBAL_CROSS_CHECK_LEADERBOARD_TOP_N:-20}"
      --global-cross-check-owner-sample-size "${SIM_GLOBAL_CROSS_CHECK_OWNER_SAMPLE_SIZE:-16}"
    )
  fi

  validator_sample_args=()
  if [[ "${SIM_VALIDATOR_SAMPLE_ENABLED:-0}" == "1" ]]; then
    validator_sample_args+=(
      --enable-validator-sample
      --validator-sample-mode "${SIM_VALIDATOR_SAMPLE_MODE:-single}"
      --validator-sample-interval-blocks "${SIM_VALIDATOR_SAMPLE_INTERVAL_BLOCKS:-0}"
      --validator-sample-size "${SIM_VALIDATOR_SAMPLE_SIZE:-1}"
      --validator-sample-min-head-advance "${SIM_VALIDATOR_SAMPLE_MIN_HEAD_ADVANCE:-2}"
    )
    if [[ "${SIM_VALIDATOR_SAMPLE_TAMPER_ENABLED:-0}" == "1" ]]; then
      validator_sample_args+=(--enable-validator-sample-tamper-check)
    fi
  fi

  btc_auth_args=(--btc-auth-mode "${btc_auth_mode}")
  if [[ -n "${cookie_file}" ]]; then
    btc_auth_args+=(--btc-cookie-file "${cookie_file}")
  fi
  if [[ -n "${btc_rpc_user}" ]]; then
    btc_auth_args+=(--btc-rpc-user "${btc_rpc_user}")
  fi
  if [[ -n "${btc_rpc_password}" ]]; then
    btc_auth_args+=(--btc-rpc-password "${btc_rpc_password}")
  fi

  log "Launching world simulator: blocks=${batch_blocks}, seed=${batch_seed}, agents=${agent_count}"

  python3 "${world_simulator}" \
    --btc-cli "${bitcoin_cli}" \
    --bitcoin-dir "${btc_data_dir}" \
    --btc-rpc-host "${btc_host}" \
    --btc-rpc-port "${btc_port}" \
    "${btc_auth_args[@]}" \
    --ord-bin "${ord_bin}" \
    --ord-data-dir "${ord_data_dir}" \
    --ord-server-url "${ord_server_url}" \
    --miner-wallet "${miner_wallet_name}" \
    --mining-address "${mining_address}" \
    --agent-wallets "${agent_wallets_csv}" \
    --agent-addresses "${agent_addresses_csv}" \
    --identity-seed "${world_sim_identity_seed}" \
    --ethw-miner-address "${resolved_ethw_miner_address}" \
    --ethw-miner-agent-id "${ethw_miner_agent_id}" \
    --balance-history-rpc-url "${balance_history_rpc_url}" \
    --usdb-rpc-url "${usdb_rpc_url}" \
    --sync-timeout-sec "${sync_timeout_sec}" \
    --blocks "${batch_blocks}" \
    --seed "${batch_seed}" \
    --fee-rate "${SIM_FEE_RATE:-1}" \
    --max-actions-per-block "${SIM_MAX_ACTIONS_PER_BLOCK:-2}" \
    --mint-probability "${SIM_MINT_PROBABILITY:-0.20}" \
    --invalid-mint-probability "${SIM_INVALID_MINT_PROBABILITY:-0.02}" \
    --transfer-probability "${SIM_TRANSFER_PROBABILITY:-0.20}" \
    --remint-probability "${SIM_REMINT_PROBABILITY:-0.10}" \
    --send-probability "${SIM_SEND_PROBABILITY:-0.30}" \
    --spend-probability "${SIM_SPEND_PROBABILITY:-0.15}" \
    --sleep-ms-between-blocks "${SIM_SLEEP_MS_BETWEEN_BLOCKS:-0}" \
    --initial-active-agents "${SIM_INITIAL_ACTIVE_AGENTS:-3}" \
    --agent-growth-interval-blocks "${SIM_AGENT_GROWTH_INTERVAL_BLOCKS:-30}" \
    --agent-growth-step "${SIM_AGENT_GROWTH_STEP:-1}" \
    --policy-mode "${SIM_POLICY_MODE:-adaptive}" \
    --scripted-cycle "${SIM_SCRIPTED_CYCLE:-mint,send_balance,transfer,remint,spend_balance,noop}" \
    "${self_check_args[@]}" \
    "${global_cross_check_args[@]}" \
    "${validator_sample_args[@]}" \
    "${report_args[@]}" \
    --recovery-state-file "${world_sim_recovery_state_file}" \
    --reorg-interval-blocks "${SIM_REORG_INTERVAL_BLOCKS:-0}" \
    --reorg-depth "${SIM_REORG_DEPTH:-3}" \
    --reorg-max-events "${SIM_REORG_MAX_EVENTS:-1}" \
    --temp-dir "${world_sim_work_dir}" \
    "${fail_fast_arg[@]}"
}

run_loop_mode() {
  resolve_ethw_protocol_alignment
  if ! bootstrap_marker_matches; then
    echo "Missing or incompatible world-sim bootstrap marker: ${world_sim_bootstrap_marker}" >&2
    exit 1
  fi

  load_bootstrap_state
  current_height="$(btc_cli getblockcount)"
  wait_until_ord_runtime_stable "${current_height}"

  if [[ "${sim_blocks}" =~ ^[1-9][0-9]*$ ]]; then
    run_simulator_batch "${sim_blocks}" "${sim_base_seed}"
    current_height="$(btc_cli getblockcount)"
    write_loop_state 1 "${sim_base_seed}" "${current_height}"
    return
  fi

  if ! [[ "${sim_loop_batch_blocks}" =~ ^[1-9][0-9]*$ ]]; then
    echo "SIM_LOOP_BATCH_BLOCKS must be a positive integer when SIM_BLOCKS=0" >&2
    exit 1
  fi

  completed_batches="$(read_completed_batches)"
  while true; do
    batch_seed="$(( sim_base_seed + completed_batches ))"
    batch_number="$(( completed_batches + 1 ))"
    log "Starting continuous world-sim batch ${batch_number}: blocks=${sim_loop_batch_blocks}, seed=${batch_seed}"
    run_simulator_batch "${sim_loop_batch_blocks}" "${batch_seed}"
    completed_batches="$(( completed_batches + 1 ))"
    current_height="$(btc_cli getblockcount)"
    write_loop_state "${completed_batches}" "${batch_seed}" "${current_height}"
  done
}

prepare_runtime_environment
wait_core_services

case "${world_sim_mode}" in
  bootstrap)
    bootstrap_world_sim
    ;;
  loop)
    run_loop_mode
    ;;
  all)
    bootstrap_world_sim
    run_loop_mode
    ;;
  *)
    echo "Unsupported WORLD_SIM_MODE=${world_sim_mode}" >&2
    exit 1
    ;;
esac
