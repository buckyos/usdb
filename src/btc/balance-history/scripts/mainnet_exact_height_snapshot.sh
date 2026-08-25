#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

USDB_ROOT="${USDB_ROOT:-${HOME}/.usdb}"
LIVE_SERVICE_ROOT="${BALANCE_HISTORY_ROOT:-${USDB_ROOT}/balance-history}"
SNAPSHOT_ROOT="${SNAPSHOT_ROOT:-${USDB_ROOT}/balance-history-snapshot-mainnet}"
BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-${SNAPSHOT_ROOT}/builder}"
RELEASE_ROOT="${SNAPSHOT_RELEASE_ROOT:-${SNAPSHOT_ROOT}/releases}"
VALIDATION_BASE="${SNAPSHOT_VALIDATION_ROOT:-${SNAPSHOT_ROOT}/validation}"
KEY_ROOT="${SNAPSHOT_KEY_ROOT:-${USDB_ROOT}/secure/snapshot-keys}"
CONFIG_ROOT="${SNAPSHOT_CONFIG_ROOT:-${SNAPSHOT_ROOT}/config}"
TARGET_ROOT="${SNAPSHOT_TARGET_ROOT:-${SNAPSHOT_ROOT}/targets}"

BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BITCOIN_DATA_DIR="${BITCOIN_DATA_DIR:-${HOME}/.bitcoin}"
BTC_RPC_URL="${BTC_RPC_URL:-http://127.0.0.1:8332}"
BITCOIN_CLI="${BITCOIN_CLI:-${BITCOIN_BIN_DIR}/bitcoin-cli}"
BITCOIND="${BITCOIND:-${BITCOIN_BIN_DIR}/bitcoind}"

BALANCE_HISTORY="${BALANCE_HISTORY_BIN:-${REPO_ROOT}/src/btc/target/release/balance-history}"
SNAPSHOT_TOOL="${SNAPSHOT_TOOL_BIN:-${REPO_ROOT}/src/btc/target/release/balance-history-snapshot-tool}"
SIGNER_ID="${SNAPSHOT_SIGNER_ID:-usdb-mainnet-snapshot-v1}"
MIN_CONFIRMATIONS="${SNAPSHOT_MIN_CONFIRMATIONS:-144}"
POLL_INTERVAL_SECS="${SNAPSHOT_POLL_INTERVAL_SECS:-30}"
CACHE_BUDGET_PERCENT="${SNAPSHOT_CACHE_BUDGET_PERCENT:-66}"
MAX_MEMORY_PERCENT="${SNAPSHOT_MAX_MEMORY_PERCENT:-80}"

BUILDER_CONFIG="${CONFIG_ROOT}/builder.toml"
SIGNING_KEY="${KEY_ROOT}/${SIGNER_ID}.signing-key.json"
PUBLIC_KEY="${KEY_ROOT}/${SIGNER_ID}.public-key.json"
TRUSTED_KEYS="${KEY_ROOT}/${SIGNER_ID}.trusted-keys.json"

COMMAND="${1:-help}"
if (($# > 0)); then
  shift
fi

HEIGHT=""
REQUESTED_HASH=""
MEMORY_PLAN_JSON=""

usage() {
  cat <<'USAGE'
Usage:
  mainnet_exact_height_snapshot.sh init
  mainnet_exact_height_snapshot.sh preflight --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh create --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh status [--height H]
  mainnet_exact_height_snapshot.sh list
  mainnet_exact_height_snapshot.sh verify --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh finalize --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh paths

Commands:
  init       Build release binaries, generate the signing key once, and freeze builder config.
  preflight  Check the mainnet node, target height/hash, confirmations, paths, and filesystem.
  create     Pin H/hash on first use, then create or resume the exact-height snapshot.
  status     Show the persisted builder/job state.
  list       List all persisted snapshot jobs.
  verify     Reopen the completed artifact and recheck the current canonical hash.
  finalize   Verify, perform an independent signed install, and create the release tar/checksum.
  paths      Print all resolved operational paths without modifying them.

Primary overrides:
  SNAPSHOT_ROOT              Snapshot-only root. Default: ~/.usdb/balance-history-snapshot-mainnet
  SNAPSHOT_SIGNER_ID         Frozen signer ID. Default: usdb-mainnet-snapshot-v1
  BITCOIN_BIN_DIR            Bitcoin Core bin directory.
  BITCOIN_DATA_DIR           Mainnet datadir. Default: ~/.bitcoin
  SNAPSHOT_MIN_CONFIRMATIONS Minimum target confirmations. Default: 144
  SNAPSHOT_CACHE_BUDGET_PERCENT
                              Effective memory assigned to both caches. Default: 66
  SNAPSHOT_MAX_MEMORY_PERCENT Whole-host/cgroup pressure threshold. Default: 80

The snapshot root must be independent from the live balance-history root. For a dedicated disk,
mount it at SNAPSHOT_ROOT or set SNAPSHOT_ROOT to a directory on that filesystem.
USAGE
}

log() {
  printf '[balance-history-mainnet-snapshot] %s\n' "$*"
}

warn() {
  printf '[balance-history-mainnet-snapshot] WARNING: %s\n' "$*" >&2
}

die() {
  printf '[balance-history-mainnet-snapshot] ERROR: %s\n' "$*" >&2
  exit 2
}

parse_target_args() {
  while (($# > 0)); do
    case "$1" in
      --height)
        (($# >= 2)) || die "--height requires a value"
        HEIGHT="$2"
        shift 2
        ;;
      --block-hash|--expected-block-hash)
        (($# >= 2)) || die "$1 requires a value"
        REQUESTED_HASH="${2,,}"
        shift 2
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        die "Unknown argument: $1"
        ;;
    esac
  done
}

require_height() {
  [[ "$HEIGHT" =~ ^[0-9]+$ ]] || die "--height must be a positive integer"
  ((10#$HEIGHT > 0)) || die "height 0 is unsupported"
  HEIGHT=$((10#$HEIGHT))
}

require_command() {
  command -v "$1" >/dev/null 2>&1 || die "Required command is unavailable: $1"
}

real_path() {
  python3 - "$1" <<'PY'
import os
import sys
print(os.path.realpath(os.path.expanduser(sys.argv[1])))
PY
}

check_path_isolation() {
  local live snapshot candidate candidate_path
  live="$(real_path "$LIVE_SERVICE_ROOT")"
  snapshot="$(real_path "$SNAPSHOT_ROOT")"
  if [[ "$snapshot" == "$live" || "$snapshot" == "$live/"* || "$live" == "$snapshot/"* ]]; then
    die "SNAPSHOT_ROOT and live root must not overlap: snapshot=${snapshot}, live=${live}"
  fi
  for candidate in "$BUILDER_ROOT" "$RELEASE_ROOT" "$VALIDATION_BASE" "$CONFIG_ROOT" "$TARGET_ROOT"; do
    candidate_path="$(real_path "$candidate")"
    if [[ "$candidate_path" == "$live" || "$candidate_path" == "$live/"* ]]; then
      die "Snapshot operational path must not equal or reside inside live root: ${candidate_path}"
    fi
  done
}

existing_ancestor() {
  local path="$1"
  while [[ ! -e "$path" ]]; do
    [[ "$path" != "/" ]] || break
    path="$(dirname "$path")"
  done
  printf '%s\n' "$path"
}

create_operational_dirs() {
  check_path_isolation
  mkdir -p "$BUILDER_ROOT" "$RELEASE_ROOT/reports" "$VALIDATION_BASE" \
    "$KEY_ROOT" "$CONFIG_ROOT" "$TARGET_ROOT"
  chmod 700 "$KEY_ROOT"
}

bitcoin_cli() {
  local rpc_host rpc_port
  read -r rpc_host rpc_port < <(python3 - "$BTC_RPC_URL" <<'PY'
import sys
from urllib.parse import urlparse

parsed = urlparse(sys.argv[1])
if parsed.scheme not in {"http", "https"} or not parsed.hostname or not parsed.port:
    raise SystemExit(f"Unsupported BTC_RPC_URL: {sys.argv[1]}")
print(parsed.hostname, parsed.port)
PY
  )
  "$BITCOIN_CLI" -datadir="$BITCOIN_DATA_DIR" -rpcconnect="$rpc_host" -rpcport="$rpc_port" "$@"
}

check_static_preflight() {
  local info chain blocks headers ibd pruned cookie

  require_command python3
  require_command jq
  require_command df
  [[ "$MIN_CONFIRMATIONS" =~ ^[0-9]+$ ]] || die "SNAPSHOT_MIN_CONFIRMATIONS must be a positive integer"
  ((10#$MIN_CONFIRMATIONS > 0)) || die "SNAPSHOT_MIN_CONFIRMATIONS must be greater than zero"
  [[ "$POLL_INTERVAL_SECS" =~ ^[0-9]+$ ]] || die "SNAPSHOT_POLL_INTERVAL_SECS must be a positive integer"
  ((10#$POLL_INTERVAL_SECS > 0)) || die "SNAPSHOT_POLL_INTERVAL_SECS must be greater than zero"
  [[ "$CACHE_BUDGET_PERCENT" =~ ^[0-9]+$ ]] || die "SNAPSHOT_CACHE_BUDGET_PERCENT must be an integer"
  ((10#$CACHE_BUDGET_PERCENT >= 1 && 10#$CACHE_BUDGET_PERCENT <= 100)) || \
    die "SNAPSHOT_CACHE_BUDGET_PERCENT must be in the range 1..=100"
  [[ "$MAX_MEMORY_PERCENT" =~ ^[0-9]+$ ]] || die "SNAPSHOT_MAX_MEMORY_PERCENT must be an integer"
  ((10#$MAX_MEMORY_PERCENT >= 20 && 10#$MAX_MEMORY_PERCENT <= 95)) || \
    die "SNAPSHOT_MAX_MEMORY_PERCENT must be in the range 20..=95"
  [[ -x "$BITCOIN_CLI" ]] || die "bitcoin-cli is not executable: ${BITCOIN_CLI}"
  [[ -x "$BITCOIND" ]] || die "bitcoind is not executable: ${BITCOIND}"
  [[ -d "$BITCOIN_DATA_DIR/blocks" ]] || die "BTC blocks directory is missing: ${BITCOIN_DATA_DIR}/blocks"
  cookie="${BITCOIN_DATA_DIR}/.cookie"
  [[ -r "$cookie" ]] || die "BTC RPC cookie is not readable: ${cookie}"

  info="$(bitcoin_cli getblockchaininfo)"
  chain="$(jq -r '.chain' <<<"$info")"
  blocks="$(jq -r '.blocks' <<<"$info")"
  headers="$(jq -r '.headers' <<<"$info")"
  ibd="$(jq -r '.initialblockdownload' <<<"$info")"
  pruned="$(jq -r '.pruned' <<<"$info")"

  [[ "$chain" == "main" ]] || die "Expected BTC mainnet, got chain=${chain}"
  [[ "$ibd" == "false" ]] || die "bitcoind is still in initial block download"
  [[ "$pruned" == "false" ]] || die "A pruned bitcoind cannot build a full historical snapshot"
  [[ "$blocks" == "$headers" ]] || die "bitcoind is not caught up: blocks=${blocks}, headers=${headers}"

  check_path_isolation
  log "BTC preflight passed: chain=main tip=${blocks} pruned=false ibd=false"
  log "live service root: ${LIVE_SERVICE_ROOT}"
  log "snapshot root: ${SNAPSHOT_ROOT}"
  if [[ -e "$SNAPSHOT_ROOT" ]]; then
    df -h "$SNAPSHOT_ROOT"
    df -i "$SNAPSHOT_ROOT"
  else
    df -h "$(existing_ancestor "$SNAPSHOT_ROOT")"
    df -i "$(existing_ancestor "$SNAPSHOT_ROOT")"
  fi
  warn "Mainnet disk capacity cannot be inferred from synthetic capacity runs alone; confirm workspace, validation copy, artifact, package, and safety margin fit before create."
}

target_file() {
  printf '%s/%012d.json\n' "$TARGET_ROOT" "$HEIGHT"
}

resolve_target() {
  local pin_target="$1"
  local tip observed confirmations record recorded_hash temp revision core_version

  require_height
  tip="$(bitcoin_cli getblockcount)"
  ((HEIGHT <= tip)) || die "Target height ${HEIGHT} exceeds BTC tip ${tip}"
  observed="$(bitcoin_cli getblockhash "$HEIGHT")"
  observed="${observed,,}"
  confirmations=$((tip - HEIGHT + 1))
  ((confirmations >= MIN_CONFIRMATIONS)) || \
    die "Target height ${HEIGHT} has ${confirmations} confirmations; required ${MIN_CONFIRMATIONS}"

  record="$(target_file)"
  if [[ -f "$record" ]]; then
    recorded_hash="$(jq -r '.btc_block_hash' "$record")"
    [[ "$recorded_hash" =~ ^[0-9a-f]{64}$ ]] || die "Invalid pinned target record: ${record}"
    if [[ -n "$REQUESTED_HASH" && "$REQUESTED_HASH" != "$recorded_hash" ]]; then
      die "Requested hash differs from pinned target ${recorded_hash} in ${record}"
    fi
    REQUESTED_HASH="$recorded_hash"
  elif [[ -z "$REQUESTED_HASH" ]]; then
    REQUESTED_HASH="$observed"
  fi

  [[ "$REQUESTED_HASH" =~ ^[0-9a-f]{64}$ ]] || die "block hash must be 64 lowercase hex characters"
  [[ "$REQUESTED_HASH" == "$observed" ]] || \
    die "Pinned target hash ${REQUESTED_HASH} is no longer canonical at height ${HEIGHT}; current hash is ${observed}"

  if [[ "$pin_target" == "1" && ! -f "$record" ]]; then
    create_operational_dirs
    temp="${record}.partial.$$"
    revision="$(git -C "$REPO_ROOT" rev-parse HEAD)"
    core_version="$($BITCOIND --version | head -n 1)"
    python3 - "$temp" "$HEIGHT" "$REQUESTED_HASH" "$tip" "$confirmations" \
      "$MIN_CONFIRMATIONS" "$revision" "$core_version" <<'PY'
import json
import os
import sys
from datetime import datetime, timezone

path, height, block_hash, tip, confirmations, minimum, revision, core_version = sys.argv[1:]
record = {
    "version": 1,
    "network": "bitcoin",
    "height": int(height),
    "btc_block_hash": block_hash,
    "tip_when_pinned": int(tip),
    "confirmations_when_pinned": int(confirmations),
    "minimum_confirmations": int(minimum),
    "code_revision": revision,
    "bitcoin_core_version": core_version,
    "pinned_at_utc": datetime.now(timezone.utc).isoformat(),
}
with open(path, "x", encoding="utf-8") as output:
    json.dump(record, output, indent=2, sort_keys=True)
    output.write("\n")
    output.flush()
    os.fsync(output.fileno())
PY
    mv "$temp" "$record"
    log "Pinned target identity in ${record}"
  fi

  log "Target passed: height=${HEIGHT} hash=${REQUESTED_HASH} confirmations=${confirmations}"
}

build_release_binaries() {
  log "Building release binaries"
  cargo build --release --manifest-path "$REPO_ROOT/src/btc/Cargo.toml" \
    -p balance-history -p balance-history-snapshot-tool
}

require_release_binaries() {
  [[ -x "$BALANCE_HISTORY" ]] || die "Missing release binary ${BALANCE_HISTORY}; run init"
  [[ -x "$SNAPSHOT_TOOL" ]] || die "Missing release binary ${SNAPSHOT_TOOL}; run init"
}

load_memory_plan() {
  if [[ -z "$MEMORY_PLAN_JSON" ]]; then
    MEMORY_PLAN_JSON="$(
      "$SNAPSHOT_TOOL" \
        --root-dir "$BUILDER_ROOT" \
        --json \
        memory-plan \
        --cache-budget-percent "$CACHE_BUDGET_PERCENT" \
        --max-memory-percent "$MAX_MEMORY_PERCENT"
    )" || die "Snapshot memory plan is invalid; adjust SNAPSHOT_CACHE_BUDGET_PERCENT or SNAPSHOT_MAX_MEMORY_PERCENT"
  fi
}

log_memory_plan() {
  load_memory_plan
  log "Memory plan: source=$(jq -r '.source' <<<"$MEMORY_PLAN_JSON") limit_bytes=$(jq -r '.memory_limit_bytes' <<<"$MEMORY_PLAN_JSON") total_cache_bytes=$(jq -r '.total_cache_bytes' <<<"$MEMORY_PLAN_JSON") utxo_cache_bytes=$(jq -r '.utxo_cache_bytes' <<<"$MEMORY_PLAN_JSON") balance_cache_bytes=$(jq -r '.balance_cache_bytes' <<<"$MEMORY_PLAN_JSON") pressure_threshold_percent=$(jq -r '.max_memory_percent' <<<"$MEMORY_PLAN_JSON")"
}

configs_match_except_memory() {
  python3 - "$1" "$2" <<'PY'
import sys
import tomllib

MEMORY_KEYS = {
    "utxo_max_cache_bytes",
    "balance_max_cache_bytes",
    "max_memory_percent",
}

def load(path):
    with open(path, "rb") as config_file:
        config = tomllib.load(config_file)
    sync = config.get("sync", {})
    for key in MEMORY_KEYS:
        sync.pop(key, None)
    return config

raise SystemExit(0 if load(sys.argv[1]) == load(sys.argv[2]) else 1)
PY
}

publish_generated_config() {
  local generated="$1"
  local destination="$2"
  local label="$3"
  if [[ -f "$destination" ]]; then
    if cmp -s "$generated" "$destination"; then
      rm -f "$generated"
      return
    fi
    if configs_match_except_memory "$generated" "$destination"; then
      mv "$generated" "$destination"
      log "Refreshed ${label} operational memory settings in ${destination}"
      return
    fi
    rm -f "$generated"
    die "Frozen ${label} config differs outside operational memory settings: ${destination}; restore the original settings or use a new root"
  fi
  mv "$generated" "$destination"
  log "Created frozen ${label} config ${destination}"
}

generate_key_if_missing() {
  if [[ -f "$SIGNING_KEY" && -f "$PUBLIC_KEY" && -f "$TRUSTED_KEYS" ]]; then
    log "Using existing signer ${SIGNER_ID} in ${KEY_ROOT}"
    return
  fi
  if [[ -e "$SIGNING_KEY" || -e "$PUBLIC_KEY" || -e "$TRUSTED_KEYS" ]]; then
    die "Incomplete signing material exists for ${SIGNER_ID}; inspect ${KEY_ROOT} before retrying"
  fi
  umask 077
  "$BALANCE_HISTORY" \
    --root-dir "$SNAPSHOT_ROOT/keygen" \
    snapshot-keygen \
    --key-id "$SIGNER_ID" \
    --out-dir "$KEY_ROOT"
}

generate_builder_config() {
  local temp="${BUILDER_CONFIG}.generated.$$"
  local utxo_cache_bytes balance_cache_bytes max_memory_percent
  load_memory_plan
  utxo_cache_bytes="$(jq -er '.utxo_cache_bytes' <<<"$MEMORY_PLAN_JSON")"
  balance_cache_bytes="$(jq -er '.balance_cache_bytes' <<<"$MEMORY_PLAN_JSON")"
  max_memory_percent="$(jq -er '.max_memory_percent' <<<"$MEMORY_PLAN_JSON")"
  python3 - "$temp" "$BUILDER_ROOT/workspace" "$BITCOIN_DATA_DIR" "$BTC_RPC_URL" \
    "$SIGNING_KEY" "$utxo_cache_bytes" "$balance_cache_bytes" "$max_memory_percent" <<'PY'
import json
import sys

path, root, btc_data, btc_rpc, signing_key, utxo_cache, balance_cache, max_memory = sys.argv[1:]
q = json.dumps
text = f'''root_dir = {q(root)}

[btc]
network = "bitcoin"
data_dir = {q(btc_data)}
rpc_url = {q(btc_rpc)}

[ordinals]
rpc_url = "http://127.0.0.1:"

[electrs]
rpc_url = "tcp://127.0.0.1:50001"

[sync]
local_loader_threshold = 500
batch_size = 128
utxo_max_cache_bytes = {utxo_cache}
balance_max_cache_bytes = {balance_cache}
max_memory_percent = {max_memory}
max_sync_block_height = 4294967295
undo_retention_blocks = 64
undo_cleanup_interval_blocks = 16

[rpc_server]
host = "127.0.0.1"
port = 8292

[snapshot]
trust_mode = "signed"
signing_key_file = {q(signing_key)}
'''
with open(path, "x", encoding="utf-8") as output:
    output.write(text)
PY
  publish_generated_config "$temp" "$BUILDER_CONFIG" "builder"
  log_memory_plan
}

ensure_initialized() {
  require_release_binaries
  [[ -f "$SIGNING_KEY" ]] || die "Signing key is missing: ${SIGNING_KEY}; run init"
  [[ -f "$PUBLIC_KEY" ]] || die "Public key is missing: ${PUBLIC_KEY}; run init"
  [[ -f "$TRUSTED_KEYS" ]] || die "Trusted key set is missing: ${TRUSTED_KEYS}; run init"
  [[ -f "$BUILDER_CONFIG" ]] || die "Builder config is missing: ${BUILDER_CONFIG}; run init"
  generate_builder_config
}

run_create() {
  local report
  check_static_preflight
  create_operational_dirs
  ensure_initialized
  resolve_target 1
  report="${RELEASE_ROOT}/reports/create-${HEIGHT}-${REQUESTED_HASH}.json"
  "$SNAPSHOT_TOOL" \
    --root-dir "$BUILDER_ROOT" \
    --json \
    create \
    --height "$HEIGHT" \
    --expected-block-hash "$REQUESTED_HASH" \
    --config "$BUILDER_CONFIG" \
    --poll-interval-secs "$POLL_INTERVAL_SECS" | tee "$report"
  log "Create report: ${report}"
}

run_verify() {
  local report
  check_static_preflight
  create_operational_dirs
  ensure_initialized
  resolve_target 0
  report="${RELEASE_ROOT}/reports/verify-${HEIGHT}-${REQUESTED_HASH}.json"
  "$SNAPSHOT_TOOL" \
    --root-dir "$BUILDER_ROOT" \
    --json \
    verify \
    --height "$HEIGHT" \
    --block-hash "$REQUESTED_HASH" | tee "$report"
  log "Verify report: ${report}"
}

generate_validation_config() {
  local validation_root="$1"
  local config="$validation_root/config.toml"
  local temp="${config}.generated.$$"
  local utxo_cache_bytes balance_cache_bytes max_memory_percent
  load_memory_plan
  utxo_cache_bytes="$(jq -er '.utxo_cache_bytes' <<<"$MEMORY_PLAN_JSON")"
  balance_cache_bytes="$(jq -er '.balance_cache_bytes' <<<"$MEMORY_PLAN_JSON")"
  max_memory_percent="$(jq -er '.max_memory_percent' <<<"$MEMORY_PLAN_JSON")"
  mkdir -p "$validation_root"
  python3 - "$temp" "$validation_root" "$BITCOIN_DATA_DIR" "$BTC_RPC_URL" \
    "$TRUSTED_KEYS" "$utxo_cache_bytes" "$balance_cache_bytes" "$max_memory_percent" <<'PY'
import json
import sys

path, root, btc_data, btc_rpc, trusted_keys, utxo_cache, balance_cache, max_memory = sys.argv[1:]
q = json.dumps
text = f'''root_dir = {q(root)}

[btc]
network = "bitcoin"
data_dir = {q(btc_data)}
rpc_url = {q(btc_rpc)}

[ordinals]
rpc_url = "http://127.0.0.1:"

[electrs]
rpc_url = "tcp://127.0.0.1:50001"

[sync]
local_loader_threshold = 500
batch_size = 128
utxo_max_cache_bytes = {utxo_cache}
balance_max_cache_bytes = {balance_cache}
max_memory_percent = {max_memory}
max_sync_block_height = 4294967295
undo_retention_blocks = 64
undo_cleanup_interval_blocks = 16

[rpc_server]
host = "127.0.0.1"
port = 8293

[snapshot]
trust_mode = "signed"
trusted_keys_file = {q(trusted_keys)}
'''
with open(path, "x", encoding="utf-8") as output:
    output.write(text)
PY
  publish_generated_config "$temp" "$config" "validation"
}

run_finalize() {
  local height_dir artifact_dir complete snapshot_file file_hash validation_root marker
  local package_name package temp_package

  run_verify
  height_dir="$(printf '%012d' "$HEIGHT")"
  artifact_dir="${BUILDER_ROOT}/snapshots/${height_dir}/${REQUESTED_HASH}"
  complete="${artifact_dir}/complete.json"
  [[ -f "$complete" ]] || die "Completed artifact marker is missing: ${complete}"
  snapshot_file="$(jq -r '.snapshot_file' "$complete")"
  file_hash="$(jq -r '.file_sha256' "$complete")"
  [[ -f "$artifact_dir/$snapshot_file" ]] || die "Snapshot file is missing: ${artifact_dir}/${snapshot_file}"

  validation_root="${VALIDATION_BASE}/${height_dir}-${REQUESTED_HASH}"
  marker="${validation_root}/signed-install-complete.json"
  generate_validation_config "$validation_root"
  if [[ -f "$marker" ]]; then
    [[ "$(jq -r '.file_sha256' "$marker")" == "$file_hash" ]] || \
      die "Existing validation marker does not match artifact hash: ${marker}"
    log "Independent signed install already verified: ${validation_root}"
  else
    "$BALANCE_HISTORY" \
      --root-dir "$validation_root" \
      install-snapshot \
      --file "$artifact_dir/$snapshot_file"
    python3 - "$marker" "$HEIGHT" "$REQUESTED_HASH" "$file_hash" <<'PY'
import json
import os
import sys
from datetime import datetime, timezone

path, height, block_hash, file_hash = sys.argv[1:]
record = {
    "version": 1,
    "height": int(height),
    "btc_block_hash": block_hash,
    "file_sha256": file_hash,
    "verified_at_utc": datetime.now(timezone.utc).isoformat(),
}
with open(path, "x", encoding="utf-8") as output:
    json.dump(record, output, indent=2, sort_keys=True)
    output.write("\n")
    output.flush()
    os.fsync(output.fileno())
PY
  fi

  package_name="balance-history-mainnet-${HEIGHT}-${REQUESTED_HASH}.tar"
  package="${RELEASE_ROOT}/${package_name}"
  if [[ -f "$package" && -f "$package.sha256" ]]; then
    (cd "$RELEASE_ROOT" && sha256sum -c "${package_name}.sha256")
    log "Release package already exists and passed checksum verification: ${package}"
  else
    [[ ! -e "$package" && ! -e "$package.sha256" ]] || \
      die "Partial release package exists; inspect ${package} and ${package}.sha256"
    temp_package="${package}.partial.$$"
    tar -C "$artifact_dir" -cf "$temp_package" .
    mv "$temp_package" "$package"
    (cd "$RELEASE_ROOT" && sha256sum "$package_name" >"${package_name}.sha256")
    log "Published release package ${package}"
  fi
  log "Finalized height=${HEIGHT} hash=${REQUESTED_HASH} validation_root=${validation_root}"
}

print_paths() {
  cat <<EOF
USDB_ROOT=${USDB_ROOT}
LIVE_SERVICE_ROOT=${LIVE_SERVICE_ROOT}
SNAPSHOT_ROOT=${SNAPSHOT_ROOT}
BUILDER_ROOT=${BUILDER_ROOT}
RELEASE_ROOT=${RELEASE_ROOT}
VALIDATION_BASE=${VALIDATION_BASE}
KEY_ROOT=${KEY_ROOT}
CONFIG_ROOT=${CONFIG_ROOT}
TARGET_ROOT=${TARGET_ROOT}
BITCOIN_DATA_DIR=${BITCOIN_DATA_DIR}
BALANCE_HISTORY=${BALANCE_HISTORY}
SNAPSHOT_TOOL=${SNAPSHOT_TOOL}
SIGNER_ID=${SIGNER_ID}
MIN_CONFIRMATIONS=${MIN_CONFIRMATIONS}
CACHE_BUDGET_PERCENT=${CACHE_BUDGET_PERCENT}
MAX_MEMORY_PERCENT=${MAX_MEMORY_PERCENT}
EOF
}

case "$COMMAND" in
  init)
    (($# == 0)) || die "init does not accept positional arguments"
    check_static_preflight
    create_operational_dirs
    build_release_binaries
    require_release_binaries
    generate_key_if_missing
    generate_builder_config
    print_paths
    ;;
  preflight)
    parse_target_args "$@"
    check_static_preflight
    resolve_target 0
    if [[ -x "$SNAPSHOT_TOOL" ]]; then
      log_memory_plan
    else
      warn "Release snapshot tool is unavailable; run init before validating the memory plan"
    fi
    ;;
  create)
    parse_target_args "$@"
    run_create
    ;;
  status)
    parse_target_args "$@"
    require_release_binaries
    if [[ -n "$HEIGHT" ]]; then
      require_height
      "$SNAPSHOT_TOOL" --root-dir "$BUILDER_ROOT" --json status --height "$HEIGHT"
    else
      "$SNAPSHOT_TOOL" --root-dir "$BUILDER_ROOT" --json status
    fi
    ;;
  list)
    (($# == 0)) || die "list does not accept arguments"
    require_release_binaries
    "$SNAPSHOT_TOOL" --root-dir "$BUILDER_ROOT" --json list
    ;;
  verify)
    parse_target_args "$@"
    run_verify
    ;;
  finalize)
    parse_target_args "$@"
    run_finalize
    ;;
  paths)
    (($# == 0)) || die "paths does not accept arguments"
    check_path_isolation
    print_paths
    ;;
  help|-h|--help)
    usage
    ;;
  *)
    usage >&2
    die "Unknown command: ${COMMAND}"
    ;;
esac
