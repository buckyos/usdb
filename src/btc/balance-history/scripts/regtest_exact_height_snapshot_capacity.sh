#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
export REPO_ROOT
WORK_DIR_AUTO_CREATED=0
if [[ -z "${WORK_DIR:-}" ]]; then
  WORK_DIR="$(mktemp -d /tmp/usdb-bh-snapshot-capacity-XXXXXX)"
  WORK_DIR_AUTO_CREATED=1
fi
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/config-source}"
SNAPSHOT_BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-$WORK_DIR/snapshot-builder}"
INSTALL_ROOT="${INSTALL_ROOT:-$WORK_DIR/install-target}"
METRICS_FILE="${METRICS_FILE:-/tmp/usdb-bh-snapshot-capacity-metrics-$(date +%Y%m%d-%H%M%S)-$$.json}"
SNAPSHOT_CAPACITY_UTXOS="${SNAPSHOT_CAPACITY_UTXOS:-1000}"
SNAPSHOT_CAPACITY_OUTPUTS_PER_TX="${SNAPSHOT_CAPACITY_OUTPUTS_PER_TX:-1000}"
SNAPSHOT_CAPACITY_COLD_CACHE="${SNAPSHOT_CAPACITY_COLD_CACHE:-0}"
SNAPSHOT_CAPACITY_OUTPUT_BTC="${SNAPSHOT_CAPACITY_OUTPUT_BTC:-0.00001000}"
SNAPSHOT_CAPACITY_PROGRESS_EVERY="${SNAPSHOT_CAPACITY_PROGRESS_EVERY:-10}"
SNAPSHOT_CAPACITY_EXECUTION_PROFILE="${SNAPSHOT_CAPACITY_EXECUTION_PROFILE:-debug_split}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30932}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30933}"
BH_RPC_PORT="${BH_RPC_PORT:-30910}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotcapacity}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-600}"
export REGTEST_LOG_PREFIX="[exact-snapshot-capacity]"
LAST_BATCH_TXID=""
SNAPSHOT_TOOL_BIN=""
BALANCE_HISTORY_BIN=""
METRIC_STAGE_LABELS=""
BLOCK_DEVICE_NAME=""
BLOCK_DEVICE_STAT=""

# SCRIPT_DIR is resolved from this file at runtime.
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/regtest_lib.sh"

snapshot_capacity_cleanup() {
  local exit_code=$?
  set +e
  trap - EXIT

  if [[ "$exit_code" -ne 0 ]]; then
    regtest_print_failure_diagnostics "$exit_code"
  fi
  regtest_stop_balance_history
  regtest_stop_bitcoind

  if [[ "$exit_code" -eq 0 && "$WORK_DIR_AUTO_CREATED" == "1" ]]; then
    case "$WORK_DIR" in
      /tmp/usdb-bh-snapshot-capacity-*)
        rm -rf -- "$WORK_DIR"
        regtest_log "Removed successful temporary workspace: ${WORK_DIR}"
        ;;
      *)
        regtest_log "Refusing to remove unexpected temporary workspace path: ${WORK_DIR}"
        ;;
    esac
  else
    regtest_log "Preserving workspace for diagnostics or caller ownership: ${WORK_DIR}"
  fi
  exit "$exit_code"
}

validate_positive_integer() {
  local name="$1"
  local value="$2"
  if [[ ! "$value" =~ ^[1-9][0-9]*$ ]]; then
    regtest_log "${name} must be a positive integer, got ${value}"
    exit 1
  fi
}

elapsed_seconds() {
  local start_ns="$1"
  local end_ns="$2"
  python3 - "$start_ns" "$end_ns" <<'PY'
import sys

print(f"{(int(sys.argv[2]) - int(sys.argv[1])) / 1_000_000_000:.6f}")
PY
}

run_timed_snapshot_tool() {
  local label="$1"
  local output_file="$2"
  shift 2
  local start_ns end_ns status=0 process_pid sampler_pid

  capture_block_device_stat "$label" before
  start_ns="$(date +%s%N)"
  "$SNAPSHOT_TOOL_BIN" \
    --root-dir "$SNAPSHOT_BUILDER_ROOT" \
    --json \
    "$@" >"$output_file" 2>"$WORK_DIR/${label}.stderr" &
  process_pid=$!
  sample_process_metrics "$process_pid" "$label" &
  sampler_pid=$!
  wait "$process_pid" || status=$?
  wait "$sampler_pid"
  end_ns="$(date +%s%N)"
  capture_block_device_stat "$label" after
  elapsed_seconds "$start_ns" "$end_ns" >"$WORK_DIR/${label}.seconds"
  return "$status"
}

run_timed_balance_history_cli() {
  local label="$1"
  local root_dir="$2"
  shift 2
  local start_ns end_ns status=0 process_pid sampler_pid

  capture_block_device_stat "$label" before
  start_ns="$(date +%s%N)"
  "$BALANCE_HISTORY_BIN" \
    --root-dir "$root_dir" \
    "$@" >"$WORK_DIR/${label}.stdout" 2>"$WORK_DIR/${label}.stderr" &
  process_pid=$!
  sample_process_metrics "$process_pid" "$label" &
  sampler_pid=$!
  wait "$process_pid" || status=$?
  wait "$sampler_pid"
  end_ns="$(date +%s%N)"
  capture_block_device_stat "$label" after
  elapsed_seconds "$start_ns" "$end_ns" >"$WORK_DIR/${label}.seconds"
  return "$status"
}

sample_process_metrics() {
  local process_pid="$1"
  local label="$2"
  local peak_kib=0 current_kib current_read_bytes current_write_bytes
  local max_read_bytes=0 max_write_bytes=0

  while kill -0 "$process_pid" 2>/dev/null; do
    current_kib="$(awk '/^VmRSS:/ { print $2 }' "/proc/${process_pid}/status" 2>/dev/null || true)"
    if [[ "$current_kib" =~ ^[0-9]+$ ]] && (( current_kib > peak_kib )); then
      peak_kib="$current_kib"
    fi
    current_read_bytes="$(awk '$1 == "read_bytes:" { print $2 }' "/proc/${process_pid}/io" 2>/dev/null || true)"
    current_write_bytes="$(awk '$1 == "write_bytes:" { print $2 }' "/proc/${process_pid}/io" 2>/dev/null || true)"
    if [[ "$current_read_bytes" =~ ^[0-9]+$ ]] && (( current_read_bytes > max_read_bytes )); then
      max_read_bytes="$current_read_bytes"
    fi
    if [[ "$current_write_bytes" =~ ^[0-9]+$ ]] && (( current_write_bytes > max_write_bytes )); then
      max_write_bytes="$current_write_bytes"
    fi
    sleep 0.02
  done
  printf '%s\n' "$peak_kib" >"$WORK_DIR/${label}.peak-rss-kib"
  printf '%s\n' "$max_read_bytes" >"$WORK_DIR/${label}.process-read-bytes"
  printf '%s\n' "$max_write_bytes" >"$WORK_DIR/${label}.process-write-bytes"
}

resolve_block_device() {
  local source
  source="$(findmnt -n -o SOURCE -T "$WORK_DIR" 2>/dev/null || true)"
  source="${source%%[*}"
  if [[ "$source" == /dev/* ]]; then
    BLOCK_DEVICE_NAME="$(basename "$source")"
    BLOCK_DEVICE_STAT="/sys/class/block/${BLOCK_DEVICE_NAME}/stat"
  fi
  if [[ -z "$BLOCK_DEVICE_NAME" || ! -r "$BLOCK_DEVICE_STAT" ]]; then
    BLOCK_DEVICE_NAME=""
    BLOCK_DEVICE_STAT=""
    regtest_log "Block-device counters unavailable for work_dir=${WORK_DIR}"
    return
  fi
  regtest_log "Collecting shared block-device counters from ${BLOCK_DEVICE_STAT}"
}

capture_block_device_stat() {
  local label="$1"
  local position="$2"
  if [[ -z "$BLOCK_DEVICE_STAT" ]]; then
    return
  fi
  awk '{ print $1, $3, $5, $7, $10 }' "$BLOCK_DEVICE_STAT" \
    >"$WORK_DIR/${label}.block-device-${position}"
}

create_unowned_recipient_descriptor() {
  local recipient_wallet="snapshot-capacity-recipient"
  local descriptor
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -named createwallet wallet_name="$recipient_wallet" load_on_startup=false >/dev/null
  descriptor="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -rpcwallet="$recipient_wallet" listdescriptors \
    | python3 -c 'import json,sys
data=json.load(sys.stdin)
for entry in data["descriptors"]:
    if entry.get("active") and not entry.get("internal") and entry.get("range") is not None:
        print(entry["desc"])
        break')"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    unloadwallet "$recipient_wallet" >/dev/null
  if [[ -z "$descriptor" ]]; then
    regtest_log "Unable to resolve an external ranged descriptor for capacity outputs"
    exit 1
  fi
  printf '%s\n' "$descriptor"
}

create_outputs_json() {
  local descriptor="$1"
  local start_index="$2"
  local count="$3"
  local amount="$4"
  local end_index addresses

  end_index=$((start_index + count - 1))
  addresses="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    deriveaddresses "$descriptor" "[${start_index},${end_index}]")"
  python3 - "$amount" "$addresses" <<'PY'
import json
import sys

addresses = json.loads(sys.argv[2])
amount = float(sys.argv[1])
print(json.dumps([{address: amount} for address in addresses], separators=(",", ":")))
PY
}

fund_output_batch() {
  local recipient_descriptor="$1"
  local start_index="$2"
  local output_count="$3"
  local outputs raw funded signed txid

  outputs="$(create_outputs_json "$recipient_descriptor" "$start_index" \
    "$output_count" "$SNAPSHOT_CAPACITY_OUTPUT_BTC")"
  raw="$(printf '%s\n%s\n' '[]' "$outputs" \
    | "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -stdin createrawtransaction)"
  funded="$(printf '%s\n' "$raw" \
    | "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -rpcwallet="$WALLET_NAME" -stdin fundrawtransaction)"
  raw="$(printf '%s' "$funded" | python3 -c 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  signed="$(printf '%s\n' "$raw" \
    | "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -rpcwallet="$WALLET_NAME" -stdin signrawtransactionwithwallet)"
  raw="$(printf '%s' "$signed" | python3 -c 'import json,sys; print(json.load(sys.stdin)["hex"])')"
  txid="$(printf '%s\n' "$raw" \
    | "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
      -stdin sendrawtransaction)"
  LAST_BATCH_TXID="$txid"
}

evict_workspace_cache() {
  python3 - "$SNAPSHOT_BUILDER_ROOT/workspace" <<'PY'
import os
from pathlib import Path
import sys

if not hasattr(os, "posix_fadvise") or not hasattr(os, "POSIX_FADV_DONTNEED"):
    raise SystemExit("Python runtime does not expose POSIX_FADV_DONTNEED")

for path in Path(sys.argv[1]).rglob("*"):
    if not path.is_file():
        continue
    with path.open("rb") as source:
        os.posix_fadvise(source.fileno(), 0, 0, os.POSIX_FADV_DONTNEED)
PY
}

write_metrics() {
  local create_report="$1"
  local verify_report="$2"
  local snapshot_path="$3"
  local target_height="$4"
  local target_hash="$5"
  local transaction_count="$6"
  python3 - "$create_report" "$verify_report" "$snapshot_path" "$METRICS_FILE" \
    "$target_height" "$target_hash" "$transaction_count" \
    "$SNAPSHOT_CAPACITY_UTXOS" "$SNAPSHOT_CAPACITY_OUTPUTS_PER_TX" \
    "$SNAPSHOT_CAPACITY_COLD_CACHE" "$WORK_DIR" "$SNAPSHOT_CAPACITY_EXECUTION_PROFILE" \
    "$METRIC_STAGE_LABELS" "$BLOCK_DEVICE_NAME" <<'PY'
import json
from pathlib import Path
import sys

create_report = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
verify_report = json.loads(Path(sys.argv[2]).read_text(encoding="utf-8"))
snapshot_path = Path(sys.argv[3])
metrics_path = Path(sys.argv[4])
work_dir = Path(sys.argv[11])
execution_profile = sys.argv[12]
stage_labels = sys.argv[13].split(",")
block_device = sys.argv[14] or None

def metric(label, suffix, cast):
    value = (work_dir / f"{label}.{suffix}").read_text(encoding="utf-8").strip()
    return cast(value)

def block_device_delta(label):
    before_path = work_dir / f"{label}.block-device-before"
    after_path = work_dir / f"{label}.block-device-after"
    if not before_path.exists() or not after_path.exists():
        return None
    before = [int(value) for value in before_path.read_text().split()]
    after = [int(value) for value in after_path.read_text().split()]
    if len(before) != 5 or len(after) != 5:
        raise SystemExit(f"invalid block-device stat for stage {label}")
    delta = [max(0, end - start) for start, end in zip(before, after)]
    return {
        "read_ios": delta[0],
        "read_bytes": delta[1] * 512,
        "write_ios": delta[2],
        "write_bytes": delta[3] * 512,
        "io_time_ms": delta[4],
    }

def stage_metric(label):
    return {
        "elapsed_seconds": metric(label, "seconds", float),
        "peak_rss_kib_sampled": metric(label, "peak-rss-kib", int),
        "process_read_bytes_sampled": metric(label, "process-read-bytes", int),
        "process_write_bytes_sampled": metric(label, "process-write-bytes", int),
        "shared_block_device_io": block_device_delta(label),
    }

metrics = {
    "schema_version": 2,
    "execution_profile": execution_profile,
    "shared_block_device": block_device,
    "target_height": int(sys.argv[5]),
    "target_block_hash": sys.argv[6],
    "requested_utxos": int(sys.argv[8]),
    "outputs_per_transaction": int(sys.argv[9]),
    "transaction_count": int(sys.argv[7]),
    "cache_mode": "cold_advisory" if sys.argv[10] == "1" else "warm",
    "actual_utxo_count": create_report["utxo_count"],
    "balance_history_count": create_report["balance_history_count"],
    "block_commit_count": create_report["block_commit_count"],
    "script_registry_count": create_report["script_registry_count"],
    "snapshot_bytes": snapshot_path.stat().st_size,
    "file_sha256": create_report["file_sha256"],
    "verified_file_sha256": verify_report["file_sha256"],
    "stages": {label: stage_metric(label) for label in stage_labels},
    "chain_load_shared_block_device_io": block_device_delta("chain-load"),
}
if metrics["actual_utxo_count"] < metrics["requested_utxos"]:
    raise SystemExit(
        f"snapshot contains {metrics['actual_utxo_count']} UTXOs, "
        f"below requested capacity load {metrics['requested_utxos']}"
    )
if metrics["file_sha256"] != metrics["verified_file_sha256"]:
    raise SystemExit("create and verify reports disagree on snapshot hash")
metrics_path.write_text(json.dumps(metrics, indent=2) + "\n", encoding="utf-8")
PY
}

main() {
  trap snapshot_capacity_cleanup EXIT

  validate_positive_integer SNAPSHOT_CAPACITY_UTXOS "$SNAPSHOT_CAPACITY_UTXOS"
  validate_positive_integer SNAPSHOT_CAPACITY_OUTPUTS_PER_TX "$SNAPSHOT_CAPACITY_OUTPUTS_PER_TX"
  validate_positive_integer SNAPSHOT_CAPACITY_PROGRESS_EVERY "$SNAPSHOT_CAPACITY_PROGRESS_EVERY"
  if (( SNAPSHOT_CAPACITY_OUTPUTS_PER_TX > 2000 )); then
    regtest_log "SNAPSHOT_CAPACITY_OUTPUTS_PER_TX must not exceed 2000"
    exit 1
  fi
  if [[ "$SNAPSHOT_CAPACITY_COLD_CACHE" != "0" && "$SNAPSHOT_CAPACITY_COLD_CACHE" != "1" ]]; then
    regtest_log "SNAPSHOT_CAPACITY_COLD_CACHE must be 0 or 1"
    exit 1
  fi
  case "$SNAPSHOT_CAPACITY_EXECUTION_PROFILE" in
    debug_split|release_e2e)
      ;;
    *)
      regtest_log "SNAPSHOT_CAPACITY_EXECUTION_PROFILE must be debug_split or release_e2e"
      exit 1
      ;;
  esac
  if [[ "$SNAPSHOT_CAPACITY_EXECUTION_PROFILE" == "release_e2e" && "$SNAPSHOT_CAPACITY_COLD_CACHE" == "1" ]]; then
    regtest_log "release_e2e cannot evict cache between sync and export; use warm mode or debug_split"
    exit 1
  fi

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd python3
  regtest_require_cmd findmnt
  regtest_ensure_workspace_dirs
  mkdir -p "$SNAPSHOT_BUILDER_ROOT" "$INSTALL_ROOT"
  if [[ "$SNAPSHOT_CAPACITY_EXECUTION_PROFILE" == "release_e2e" ]]; then
    (
      cd "$REPO_ROOT" || exit 1
      cargo build --quiet --release --manifest-path src/btc/Cargo.toml \
        -p balance-history-snapshot-tool -p balance-history
    )
    SNAPSHOT_TOOL_BIN="$REPO_ROOT/src/btc/target/release/balance-history-snapshot-tool"
    BALANCE_HISTORY_BIN="$REPO_ROOT/src/btc/target/release/balance-history"
    METRIC_STAGE_LABELS="create,verify,install"
  else
    (
      cd "$REPO_ROOT" || exit 1
      cargo build --quiet --manifest-path src/btc/Cargo.toml \
        -p balance-history-snapshot-tool -p balance-history
    )
    SNAPSHOT_TOOL_BIN="$REPO_ROOT/src/btc/target/debug/balance-history-snapshot-tool"
    BALANCE_HISTORY_BIN="$REPO_ROOT/src/btc/target/debug/balance-history"
    METRIC_STAGE_LABELS="sync,export,verify,install"
  fi
  resolve_block_device
  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address recipient_descriptor remaining batch_count transaction_count=0 output_index=0
  local load_start_ns load_end_ns load_seconds target_height target_hash confirmation_blocks=6
  local sync_report create_report verify_report artifact_dir snapshot_file snapshot_path

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  recipient_descriptor="$(create_unowned_recipient_descriptor)"
  remaining="$SNAPSHOT_CAPACITY_UTXOS"
  capture_block_device_stat chain-load before
  load_start_ns="$(date +%s%N)"
  while (( remaining > 0 )); do
    batch_count="$SNAPSHOT_CAPACITY_OUTPUTS_PER_TX"
    if (( batch_count > remaining )); then
      batch_count="$remaining"
    fi
    fund_output_batch "$recipient_descriptor" "$output_index" "$batch_count"
    regtest_mine_blocks 1 "$mining_address"
    remaining=$((remaining - batch_count))
    output_index=$((output_index + batch_count))
    transaction_count=$((transaction_count + 1))
    if (( transaction_count == 1 || remaining == 0 || transaction_count % SNAPSHOT_CAPACITY_PROGRESS_EVERY == 0 )); then
      regtest_log "Load progress: created=${output_index}/${SNAPSHOT_CAPACITY_UTXOS}, batches=${transaction_count}, last_txid=${LAST_BATCH_TXID}"
    fi
  done
  target_height="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  target_hash="$(regtest_get_block_hash_by_height "$target_height")"
  regtest_mine_blocks "$confirmation_blocks" "$mining_address"
  load_end_ns="$(date +%s%N)"
  capture_block_device_stat chain-load after
  load_seconds="$(elapsed_seconds "$load_start_ns" "$load_end_ns")"

  regtest_create_balance_history_config_at "$BALANCE_HISTORY_ROOT" "$BH_RPC_PORT"
  create_report="$WORK_DIR/create.json"
  if [[ "$SNAPSHOT_CAPACITY_EXECUTION_PROFILE" == "release_e2e" ]]; then
    run_timed_snapshot_tool create "$create_report" create \
      --height "$target_height" \
      --expected-block-hash "$target_hash" \
      --config "$BALANCE_HISTORY_ROOT/config.toml" \
      --poll-interval-secs 1
  else
    sync_report="$WORK_DIR/sync.json"
    if USDB_BH_SNAPSHOT_TEST_ABORT_AFTER_CHECKPOINT=sealed \
      run_timed_snapshot_tool sync "$sync_report" create \
        --height "$target_height" \
        --expected-block-hash "$target_hash" \
        --config "$BALANCE_HISTORY_ROOT/config.toml" \
        --poll-interval-secs 1; then
      regtest_log "Expected sealed checkpoint abort but snapshot create completed"
      exit 1
    fi
    regtest_assert_json_file "$SNAPSHOT_BUILDER_ROOT/jobs/$(printf '%012d' "$target_height")/job.json" \
      "data['stage']" "sealed"

    if [[ "$SNAPSHOT_CAPACITY_COLD_CACHE" == "1" ]]; then
      regtest_log "Advising the kernel to evict closed workspace files before export"
      evict_workspace_cache
    fi

    run_timed_snapshot_tool export "$create_report" create \
      --height "$target_height" \
      --expected-block-hash "$target_hash" \
      --poll-interval-secs 1
  fi
  regtest_assert_json_file "$create_report" "data['utxo_count'] >= ${SNAPSHOT_CAPACITY_UTXOS}" "True"

  verify_report="$WORK_DIR/verify.json"
  run_timed_snapshot_tool verify "$verify_report" verify \
    --height "$target_height" \
    --block-hash "$target_hash"

  artifact_dir="$(python3 - "$create_report" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["artifact_dir"])
PY
)"
  snapshot_file="$(python3 - "$create_report" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1], encoding="utf-8"))["snapshot_file"])
PY
)"
  snapshot_path="$SNAPSHOT_BUILDER_ROOT/$artifact_dir/$snapshot_file"

  regtest_create_balance_history_config_at "$INSTALL_ROOT" 30911
  regtest_config_set_max_sync_block_height "$INSTALL_ROOT/config.toml" "$target_height"
  regtest_config_set_snapshot_policy "$INSTALL_ROOT/config.toml" manifest "" ""
  run_timed_balance_history_cli install "$INSTALL_ROOT" install-snapshot --file "$snapshot_path"

  write_metrics "$create_report" "$verify_report" "$snapshot_path" \
    "$target_height" "$target_hash" "$transaction_count"
  python3 - "$METRICS_FILE" "$load_seconds" <<'PY'
import json
from pathlib import Path
import sys

path = Path(sys.argv[1])
metrics = json.loads(path.read_text(encoding="utf-8"))
metrics["chain_load_seconds"] = float(sys.argv[2])
path.write_text(json.dumps(metrics, indent=2) + "\n", encoding="utf-8")
PY

  regtest_log "Capacity benchmark succeeded. Metrics: ${METRICS_FILE}"
  cat "$METRICS_FILE"
}

main "$@"
