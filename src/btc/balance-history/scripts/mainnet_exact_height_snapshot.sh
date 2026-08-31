#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"

USDB_ROOT="${USDB_ROOT:-${HOME}/.usdb}"
LIVE_SERVICE_ROOT="${BALANCE_HISTORY_ROOT:-${USDB_ROOT}/balance-history}"
SNAPSHOT_ROOT="${SNAPSHOT_ROOT:-${USDB_ROOT}/balance-history-snapshot-mainnet}"
BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-${SNAPSHOT_ROOT}/builder}"
RELEASE_ROOT="${SNAPSHOT_RELEASE_ROOT:-${SNAPSHOT_ROOT}/releases}"
FINALIZATION_BASE="${SNAPSHOT_FINALIZATION_ROOT:-${RELEASE_ROOT}/finalized}"
VALIDATION_BASE="${SNAPSHOT_VALIDATION_ROOT:-${SNAPSHOT_ROOT}/validation}"
VALIDATION_REPORT_ROOT="${SNAPSHOT_VALIDATION_REPORT_ROOT:-${RELEASE_ROOT}/validation-reports}"
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
DISTRIBUTION_TOOL="${SNAPSHOT_DISTRIBUTION_TOOL:-${REPO_ROOT}/docker/scripts/tools/snapshot_distribution.py}"
SIGNER_ID="${SNAPSHOT_SIGNER_ID:-usdb-mainnet-snapshot-v1}"
MIN_CONFIRMATIONS="${SNAPSHOT_MIN_CONFIRMATIONS:-144}"
POLL_INTERVAL_SECS="${SNAPSHOT_POLL_INTERVAL_SECS:-30}"
CACHE_BUDGET_PERCENT="${SNAPSHOT_CACHE_BUDGET_PERCENT:-66}"
MAX_MEMORY_PERCENT="${SNAPSHOT_MAX_MEMORY_PERCENT:-80}"
RECORD_ROOT="${SNAPSHOT_RECORD_ROOT:-${RELEASE_ROOT}/records}"
S3_BUCKET="${SNAPSHOT_S3_BUCKET:-usdb-snapshot}"
S3_ENDPOINT_URL="${SNAPSHOT_S3_ENDPOINT_URL:-https://87e0bdf811b13ee87fd0bcec7a4fd1e7.r2.cloudflarestorage.com}"
PUBLIC_BASE_URL="${SNAPSHOT_PUBLIC_BASE_URL:-https://usdb-snapshot.tbudr.top}"
AWS_REGION="${SNAPSHOT_AWS_REGION:-auto}"
AWS_PROFILE="${SNAPSHOT_AWS_PROFILE-usdb-snapshot-publisher}"
AWS_EXECUTABLE="${SNAPSHOT_AWS_EXECUTABLE:-aws}"
S3_UPLOAD_CONCURRENCY="${SNAPSHOT_S3_UPLOAD_CONCURRENCY:-16}"
S3_CHUNK_SIZE_MIB="${SNAPSHOT_S3_CHUNK_SIZE_MIB:-64}"
UPLOAD_PROGRESS="${SNAPSHOT_UPLOAD_PROGRESS:-1}"

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
  mainnet_exact_height_snapshot.sh resume-verify --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh status [--height H]
  mainnet_exact_height_snapshot.sh list
  mainnet_exact_height_snapshot.sh verify --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh finalize --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh validate-install --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh archive --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh prepare-release --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh publish --height H [--block-hash HASH]
  mainnet_exact_height_snapshot.sh paths

Commands:
  init       Build release binaries, generate the signing key once, and freeze builder config.
  preflight  Check the mainnet node, target height/hash, confirmations, paths, and filesystem.
  create     Pin H/hash on first use, then create or resume the exact-height snapshot.
  resume-verify
             Resume verification/publication of an existing temporary artifact without RocksDB.
  status     Show the persisted builder/job state.
  list       List all persisted snapshot jobs.
  verify     Reopen the completed artifact and recheck the current canonical hash.
  finalize   Recheck artifact identity, DB hash, and signature without opening SQLite or RocksDB.
  validate-install
             Independently restore one finalized artifact into a dedicated RocksDB validation root.
  archive    Optionally create and checksum an offline tar from one finalized artifact.
  prepare-release
             Infer finalized artifact inputs and create a content-addressed release record.
  publish    Prepare the release record and idempotently upload it and its files to object storage.
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
  SNAPSHOT_AWS_PROFILE       AWS CLI profile used by publish. Default: usdb-snapshot-publisher
  SNAPSHOT_S3_BUCKET         S3-compatible bucket. Default: usdb-snapshot
  SNAPSHOT_S3_ENDPOINT_URL   Private S3-compatible endpoint.
  SNAPSHOT_PUBLIC_BASE_URL   Public HTTPS base recorded for downloads.
  SNAPSHOT_S3_UPLOAD_CONCURRENCY
                              Concurrent AWS CLI multipart requests. Default: 16
  SNAPSHOT_S3_CHUNK_SIZE_MIB AWS CLI multipart part size in MiB. Default: 64
  SNAPSHOT_UPLOAD_PROGRESS   Force upload progress when set to 1. Default: 1

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
  for candidate in "$BUILDER_ROOT" "$RELEASE_ROOT" "$FINALIZATION_BASE" "$VALIDATION_BASE" \
    "$VALIDATION_REPORT_ROOT" "$CONFIG_ROOT" "$TARGET_ROOT"; do
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
  mkdir -p "$BUILDER_ROOT" "$RELEASE_ROOT/reports" "$RECORD_ROOT" "$FINALIZATION_BASE" \
    "$VALIDATION_BASE" "$VALIDATION_REPORT_ROOT" "$KEY_ROOT" "$CONFIG_ROOT" "$TARGET_ROOT"
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

run_resume_verify() {
  local report
  check_static_preflight
  create_operational_dirs
  ensure_initialized
  resolve_target 0
  report="${RELEASE_ROOT}/reports/resume-verify-${HEIGHT}-${REQUESTED_HASH}.json"
  "$SNAPSHOT_TOOL" \
    --root-dir "$BUILDER_ROOT" \
    --json \
    resume-verify \
    --height "$HEIGHT" \
    --expected-block-hash "$REQUESTED_HASH" | tee "$report"
  log "Resume verify report: ${report}"
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
  local height_dir marker_dir marker report finalization_output target_record producer_revision finalizer_revision

  check_static_preflight
  create_operational_dirs
  require_release_binaries
  [[ -f "$TRUSTED_KEYS" ]] || die "Trusted key set is missing: ${TRUSTED_KEYS}; run init"
  resolve_target 0
  height_dir="$(printf '%012d' "$HEIGHT")"
  target_record="$(target_file)"
  producer_revision="$(jq -er '.code_revision' "$target_record")" || \
    die "Pinned target record has no producer code revision: ${target_record}"
  [[ "$producer_revision" =~ ^[0-9a-f]{40}$ ]] || \
    die "Pinned target producer revision is invalid: ${producer_revision}"
  finalizer_revision="$(git -C "$REPO_ROOT" rev-parse HEAD)"

  report="${RELEASE_ROOT}/reports/finalize-${HEIGHT}-${REQUESTED_HASH}.json"
  finalization_output="$(
    "$SNAPSHOT_TOOL" \
      --root-dir "$BUILDER_ROOT" \
      --json \
      finalize-artifact \
      --height "$HEIGHT" \
      --block-hash "$REQUESTED_HASH" \
      --trusted-keys "$TRUSTED_KEYS"
  )"
  jq -e . >/dev/null <<<"$finalization_output" || \
    die "Snapshot finalization tool returned invalid JSON"
  printf '%s\n' "$finalization_output" | tee "$report"

  marker_dir="${FINALIZATION_BASE}/${height_dir}-${REQUESTED_HASH}"
  marker="${marker_dir}/artifact-finalized.json"
  mkdir -p "$marker_dir"
  if [[ -f "$marker" ]]; then
    jq -e \
      --arg producer_revision "$producer_revision" \
      --arg finalizer_revision "$finalizer_revision" \
      --slurpfile report "$report" '
      .version == 1
      and .height == $report[0].height
      and .network == $report[0].network
      and .btc_block_hash == $report[0].btc_block_hash
      and .snapshot_id == $report[0].snapshot_id
      and .snapshot_file == $report[0].snapshot_file
      and .manifest_file == $report[0].manifest_file
      and .signature_file == $report[0].signature_file
      and .file_sha256 == $report[0].file_sha256
      and .signing_key_id == $report[0].signing_key_id
      and .trusted_keys_sha256 == $report[0].trusted_keys_sha256
      and .producer_revision == $producer_revision
      and .finalizer_revision == $finalizer_revision
      and (.finalized_at_utc | type == "string" and length > 0)
    ' "$marker" >/dev/null || die "Existing finalization marker does not match artifact: ${marker}"
    log "Artifact finalization already verified: ${marker}"
  else
    python3 - "$marker" "$report" "$producer_revision" "$finalizer_revision" <<'PY'
import json
import os
import sys
from datetime import datetime, timezone

path, report_path, producer_revision, finalizer_revision = sys.argv[1:]
with open(report_path, encoding="utf-8") as source:
    report = json.load(source)
record = {
    "version": 1,
    "height": report["height"],
    "network": report["network"],
    "btc_block_hash": report["btc_block_hash"],
    "snapshot_id": report["snapshot_id"],
    "snapshot_file": report["snapshot_file"],
    "manifest_file": report["manifest_file"],
    "signature_file": report["signature_file"],
    "file_sha256": report["file_sha256"],
    "signing_key_id": report["signing_key_id"],
    "trusted_keys_sha256": report["trusted_keys_sha256"],
    "producer_revision": producer_revision,
    "finalizer_revision": finalizer_revision,
    "finalized_at_utc": datetime.now(timezone.utc).isoformat(),
}
temporary = f"{path}.tmp.{os.getpid()}"
try:
    with open(temporary, "x", encoding="utf-8") as output:
        json.dump(record, output, indent=2, sort_keys=True)
        output.write("\n")
        output.flush()
        os.fsync(output.fileno())
    os.replace(temporary, path)
    directory = os.open(os.path.dirname(path), os.O_RDONLY)
    try:
        os.fsync(directory)
    finally:
        os.close(directory)
finally:
    try:
        os.unlink(temporary)
    except FileNotFoundError:
        pass
PY
  fi
  log "Finalized artifact height=${HEIGHT} hash=${REQUESTED_HASH} marker=${marker}"
}

resolve_finalized_snapshot() {
  local target_record height_dir complete file_hash marker_file_hash

  check_static_preflight
  create_operational_dirs
  require_height
  target_record="$(target_file)"
  [[ -f "$target_record" ]] || \
    die "Pinned target record is missing: ${target_record}; run create and finalize first"
  resolve_target 0

  height_dir="$(printf '%012d' "$HEIGHT")"
  FINALIZED_ARTIFACT_DIR="${BUILDER_ROOT}/snapshots/${height_dir}/${REQUESTED_HASH}"
  FINALIZED_FINALIZATION_MARKER="${FINALIZATION_BASE}/${height_dir}-${REQUESTED_HASH}/artifact-finalized.json"
  FINALIZED_PRODUCER_REVISION="$(jq -er '.code_revision' "$target_record")" || \
    die "Pinned target record has no producer code revision: ${target_record}"
  [[ "$FINALIZED_PRODUCER_REVISION" =~ ^[0-9a-f]{40}$ ]] || \
    die "Pinned target producer revision is invalid: ${FINALIZED_PRODUCER_REVISION}"
  complete="${FINALIZED_ARTIFACT_DIR}/complete.json"
  [[ -f "$complete" ]] || die "Finalized snapshot artifact is missing: ${complete}"
  [[ -f "$FINALIZED_FINALIZATION_MARKER" ]] || \
    die "Artifact finalization marker is missing: ${FINALIZED_FINALIZATION_MARKER}; run finalize first"
  file_hash="$(jq -er '.file_sha256' "$complete")" || die "Completed artifact has no file SHA-256: ${complete}"
  marker_file_hash="$(jq -er '.file_sha256' "$FINALIZED_FINALIZATION_MARKER")" || \
    die "Artifact finalization marker has no file SHA-256: ${FINALIZED_FINALIZATION_MARKER}"
  [[ "$file_hash" =~ ^[0-9a-f]{64}$ ]] || die "Completed artifact file SHA-256 is invalid: ${file_hash}"
  [[ "$marker_file_hash" == "$file_hash" ]] || \
    die "Artifact finalization marker does not match finalized artifact"
  jq -e \
    --argjson height "$HEIGHT" \
    --arg block_hash "$REQUESTED_HASH" \
    --arg producer_revision "$FINALIZED_PRODUCER_REVISION" '
      .version == 1
      and .height == $height
      and .network == "bitcoin"
      and .btc_block_hash == $block_hash
      and .producer_revision == $producer_revision
      and (.finalizer_revision | type == "string" and test("^[0-9a-f]{40}$"))
      and (.snapshot_id | type == "string" and test("^[0-9a-f]{64}$"))
      and (.signing_key_id | type == "string" and length > 0)
      and (.trusted_keys_sha256 | type == "string" and test("^[0-9a-f]{64}$"))
      and (.finalized_at_utc | type == "string" and length > 0)
    ' "$FINALIZED_FINALIZATION_MARKER" >/dev/null || \
    die "Artifact finalization marker identity is invalid: ${FINALIZED_FINALIZATION_MARKER}"
  FINALIZED_FILE_SHA256="$file_hash"
  FINALIZED_FINALIZER_REVISION="$(jq -er '.finalizer_revision' "$FINALIZED_FINALIZATION_MARKER")"
}

run_validate_install() {
  local height_dir validation_root report validation_revision

  resolve_finalized_snapshot
  require_release_binaries
  [[ -f "$TRUSTED_KEYS" ]] || die "Trusted key set is missing: ${TRUSTED_KEYS}; run init"
  height_dir="$(printf '%012d' "$HEIGHT")"
  validation_root="${VALIDATION_BASE}/${height_dir}-${REQUESTED_HASH}"
  validation_revision="$(git -C "$REPO_ROOT" rev-parse HEAD)"
  report="${VALIDATION_REPORT_ROOT}/validate-install-${HEIGHT}-${REQUESTED_HASH}-${validation_revision}.json"
  if [[ -f "$report" ]]; then
    jq -e \
      --argjson height "$HEIGHT" \
      --arg block_hash "$REQUESTED_HASH" \
      --arg file_hash "$FINALIZED_FILE_SHA256" \
      --arg producer_revision "$FINALIZED_PRODUCER_REVISION" \
      --arg finalizer_revision "$FINALIZED_FINALIZER_REVISION" \
      --arg validation_revision "$validation_revision" '
        .version == 1
        and .height == $height
        and .btc_block_hash == $block_hash
        and .file_sha256 == $file_hash
        and .producer_revision == $producer_revision
        and .finalizer_revision == $finalizer_revision
        and .validation_revision == $validation_revision
        and (.validated_at_utc | type == "string" and length > 0)
      ' "$report" >/dev/null || die "Existing install-validation report is invalid: ${report}"
    log "Independent signed install already validated: ${report}"
    return
  fi

  generate_validation_config "$validation_root"
  "$BALANCE_HISTORY" \
    --root-dir "$validation_root" \
    install-snapshot \
    --file "${FINALIZED_ARTIFACT_DIR}/$(jq -er '.snapshot_file' "${FINALIZED_ARTIFACT_DIR}/complete.json")"
  python3 - "$report" "$HEIGHT" "$REQUESTED_HASH" "$FINALIZED_FILE_SHA256" \
    "$FINALIZED_PRODUCER_REVISION" "$FINALIZED_FINALIZER_REVISION" "$validation_revision" \
    "$validation_root" <<'PY'
import json
import os
import sys
from datetime import datetime, timezone

path, height, block_hash, file_hash, producer_revision, finalizer_revision, validation_revision, validation_root = sys.argv[1:]
record = {
    "version": 1,
    "height": int(height),
    "btc_block_hash": block_hash,
    "file_sha256": file_hash,
    "producer_revision": producer_revision,
    "finalizer_revision": finalizer_revision,
    "validation_revision": validation_revision,
    "validation_root": validation_root,
    "validated_at_utc": datetime.now(timezone.utc).isoformat(),
}
with open(path, "x", encoding="utf-8") as output:
    json.dump(record, output, indent=2, sort_keys=True)
    output.write("\n")
    output.flush()
    os.fsync(output.fileno())
PY
  log "Independent signed install validated: report=${report} validation_root=${validation_root}"
}

run_archive() {
  local package_name package temp_package

  resolve_finalized_snapshot
  require_command tar
  require_command sha256sum
  package_name="balance-history-mainnet-${HEIGHT}-${REQUESTED_HASH}.tar"
  package="${RELEASE_ROOT}/${package_name}"
  if [[ -f "$package" && -f "$package.sha256" ]]; then
    (cd "$RELEASE_ROOT" && sha256sum -c "${package_name}.sha256")
    log "Archive already exists and passed checksum verification: ${package}"
    return
  fi
  temp_package="${package}.partial"
  [[ ! -e "$package" && ! -e "$package.sha256" && ! -e "$temp_package" ]] || \
    die "Partial archive exists; inspect ${package}, ${package}.sha256, and ${temp_package}"
  tar -C "$FINALIZED_ARTIFACT_DIR" -cf "$temp_package" .
  mv "$temp_package" "$package"
  (cd "$RELEASE_ROOT" && sha256sum "$package_name" >"${package_name}.sha256")
  log "Created optional offline archive ${package}"
}

prepare_snapshot_release() {
  local report prepare_output

  resolve_finalized_snapshot
  [[ -f "$TRUSTED_KEYS" ]] || die "Trusted-key catalog is missing: ${TRUSTED_KEYS}"
  [[ -f "$DISTRIBUTION_TOOL" ]] || die "Snapshot distribution tool is missing: ${DISTRIBUTION_TOOL}"

  report="${RELEASE_ROOT}/reports/prepare-release-${HEIGHT}-${REQUESTED_HASH}.json"
  prepare_output="$(
    python3 "$DISTRIBUTION_TOOL" prepare \
      --artifact-dir "$FINALIZED_ARTIFACT_DIR" \
      --trusted-keys "$TRUSTED_KEYS" \
      --finalization-marker "$FINALIZED_FINALIZATION_MARKER" \
      --producer-revision "$FINALIZED_PRODUCER_REVISION" \
      --public-base-url "$PUBLIC_BASE_URL" \
      --output-dir "$RECORD_ROOT"
  )"
  jq -e . >/dev/null <<<"$prepare_output" || die "Snapshot distribution tool returned invalid JSON"
  printf '%s\n' "$prepare_output" | tee "$report"
  PREPARED_RECORD="$(jq -er '.record_path' <<<"$prepare_output")" || \
    die "Snapshot distribution prepare output has no record_path"
  [[ -f "$PREPARED_RECORD" ]] || die "Prepared release record does not exist: ${PREPARED_RECORD}"
  log "Release record report: ${report}"
}

run_prepare_release() {
  prepare_snapshot_release
}

run_publish() {
  local report
  local -a upload_args

  prepare_snapshot_release
  require_command "$AWS_EXECUTABLE"
  upload_args=(
    "$DISTRIBUTION_TOOL" upload
    --record "$PREPARED_RECORD"
    --source-dir "$FINALIZED_ARTIFACT_DIR"
    --bucket "$S3_BUCKET"
    --endpoint-url "$S3_ENDPOINT_URL"
    --aws-region "$AWS_REGION"
    --s3-upload-concurrency "$S3_UPLOAD_CONCURRENCY"
    --s3-chunk-size-mib "$S3_CHUNK_SIZE_MIB"
    --aws-executable "$AWS_EXECUTABLE"
  )
  if [[ -n "$AWS_PROFILE" ]]; then
    upload_args+=(--aws-profile "$AWS_PROFILE")
  fi
  if [[ "$UPLOAD_PROGRESS" == "1" ]]; then
    upload_args+=(--progress)
  elif [[ "$UPLOAD_PROGRESS" != "0" ]]; then
    die "SNAPSHOT_UPLOAD_PROGRESS must be 0 or 1"
  fi
  report="${RELEASE_ROOT}/reports/publish-${HEIGHT}-${REQUESTED_HASH}.json"
  python3 "${upload_args[@]}" | tee "$report"
  log "Publish report: ${report}"
}

print_paths() {
  cat <<EOF
USDB_ROOT=${USDB_ROOT}
LIVE_SERVICE_ROOT=${LIVE_SERVICE_ROOT}
SNAPSHOT_ROOT=${SNAPSHOT_ROOT}
BUILDER_ROOT=${BUILDER_ROOT}
RELEASE_ROOT=${RELEASE_ROOT}
FINALIZATION_BASE=${FINALIZATION_BASE}
VALIDATION_BASE=${VALIDATION_BASE}
VALIDATION_REPORT_ROOT=${VALIDATION_REPORT_ROOT}
KEY_ROOT=${KEY_ROOT}
CONFIG_ROOT=${CONFIG_ROOT}
TARGET_ROOT=${TARGET_ROOT}
BITCOIN_DATA_DIR=${BITCOIN_DATA_DIR}
BALANCE_HISTORY=${BALANCE_HISTORY}
SNAPSHOT_TOOL=${SNAPSHOT_TOOL}
DISTRIBUTION_TOOL=${DISTRIBUTION_TOOL}
SIGNER_ID=${SIGNER_ID}
MIN_CONFIRMATIONS=${MIN_CONFIRMATIONS}
CACHE_BUDGET_PERCENT=${CACHE_BUDGET_PERCENT}
MAX_MEMORY_PERCENT=${MAX_MEMORY_PERCENT}
RECORD_ROOT=${RECORD_ROOT}
S3_BUCKET=${S3_BUCKET}
S3_ENDPOINT_URL=${S3_ENDPOINT_URL}
PUBLIC_BASE_URL=${PUBLIC_BASE_URL}
AWS_REGION=${AWS_REGION}
AWS_PROFILE=${AWS_PROFILE}
S3_UPLOAD_CONCURRENCY=${S3_UPLOAD_CONCURRENCY}
S3_CHUNK_SIZE_MIB=${S3_CHUNK_SIZE_MIB}
UPLOAD_PROGRESS=${UPLOAD_PROGRESS}
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
  resume-verify)
    parse_target_args "$@"
    run_resume_verify
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
  validate-install)
    parse_target_args "$@"
    run_validate_install
    ;;
  archive)
    parse_target_args "$@"
    run_archive
    ;;
  prepare-release)
    parse_target_args "$@"
    run_prepare_release
    ;;
  publish)
    parse_target_args "$@"
    run_publish
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
