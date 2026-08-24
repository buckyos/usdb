#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
export REPO_ROOT
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-snapshot-signed-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/config-source}"
SNAPSHOT_BUILDER_ROOT="${SNAPSHOT_BUILDER_ROOT:-$WORK_DIR/snapshot-builder}"
KEY_ROOT="${KEY_ROOT:-$WORK_DIR/keys}"
BAD_SIGNATURE_ROOT="${BAD_SIGNATURE_ROOT:-$WORK_DIR/bad-signature-target}"
UNTRUSTED_ROOT="${UNTRUSTED_ROOT:-$WORK_DIR/untrusted-target}"
SUCCESS_ROOT="${SUCCESS_ROOT:-$WORK_DIR/success-target}"
BTC_RPC_PORT="${BTC_RPC_PORT:-30432}"
BTC_P2P_PORT="${BTC_P2P_PORT:-30433}"
BH_RPC_PORT="${BH_RPC_PORT:-30410}"
WALLET_NAME="${WALLET_NAME:-bhsnapshotsigned}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-180}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
export REGTEST_LOG_PREFIX="[exact-snapshot-signed]"

# SCRIPT_DIR is resolved from this file at runtime.
# shellcheck disable=SC1091
source "${SCRIPT_DIR}/regtest_lib.sh"

json_field() {
  local file="$1"
  local field="$2"
  python3 - "$file" "$field" <<'PY'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as source:
    data = json.load(source)
print(data[sys.argv[2]])
PY
}

assert_no_install_artifacts() {
  local root_dir="$1"
  local count
  count="$(find "$root_dir" -maxdepth 1 -type d \
    \( -name 'snapshot_install_staging_*' -o -name 'db_backup_snapshot_install_*' \) \
    | wc -l | tr -d ' ')"
  if [[ "$count" != "0" ]]; then
    regtest_log "Failed signed install left staging or backup artifacts in ${root_dir}"
    exit 1
  fi
}

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd python3
  regtest_ensure_workspace_dirs
  mkdir -p "$SNAPSHOT_BUILDER_ROOT" "$KEY_ROOT" "$BAD_SIGNATURE_ROOT" \
    "$UNTRUSTED_ROOT" "$SUCCESS_ROOT"

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address tip snapshot_height baseline_height snapshot_hash report
  local signer_key trusted_keys untrusted_keys artifact_dir snapshot_file manifest_file signature_file
  local snapshot_path manifest_path signature_path tampered_dir tampered_snapshot tampered_signature
  local failure_output baseline_hash resp

  mining_address="$(regtest_get_new_address)"
  regtest_ensure_mature_funds "$mining_address"
  tip="$($BITCOIN_CLI_BIN -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockcount)"
  snapshot_height=$((tip - 6))
  baseline_height=$((snapshot_height - 1))
  snapshot_hash="$(regtest_get_block_hash_by_height "$snapshot_height")"
  baseline_hash="$(regtest_get_block_hash_by_height "$baseline_height")"

  regtest_run_balance_history_cli "$KEY_ROOT" snapshot-keygen \
    --key-id exact-signer --out-dir "$KEY_ROOT" >/dev/null
  regtest_run_balance_history_cli "$KEY_ROOT" snapshot-keygen \
    --key-id other-signer --out-dir "$KEY_ROOT" >/dev/null
  signer_key="$KEY_ROOT/exact-signer.signing-key.json"
  trusted_keys="$KEY_ROOT/exact-signer.trusted-keys.json"
  untrusted_keys="$KEY_ROOT/other-signer.trusted-keys.json"

  regtest_create_balance_history_config_at "$BALANCE_HISTORY_ROOT" "$BH_RPC_PORT"
  regtest_config_set_snapshot_policy \
    "$BALANCE_HISTORY_ROOT/config.toml" manifest "$signer_key" ""

  report="$WORK_DIR/signed-create.json"
  regtest_run_snapshot_tool "$SNAPSHOT_BUILDER_ROOT" create \
    --height "$snapshot_height" \
    --expected-block-hash "$snapshot_hash" \
    --config "$BALANCE_HISTORY_ROOT/config.toml" \
    --poll-interval-secs 1 >"$report"
  regtest_assert_json_file "$report" "data['signature_file'] is not None" "True"

  artifact_dir="$(json_field "$report" artifact_dir)"
  snapshot_file="$(json_field "$report" snapshot_file)"
  manifest_file="$(json_field "$report" manifest_file)"
  signature_file="$(json_field "$report" signature_file)"
  snapshot_path="$SNAPSHOT_BUILDER_ROOT/$artifact_dir/$snapshot_file"
  manifest_path="$SNAPSHOT_BUILDER_ROOT/$artifact_dir/$manifest_file"
  signature_path="$SNAPSHOT_BUILDER_ROOT/$artifact_dir/$signature_file"

  tampered_dir="$WORK_DIR/tampered-artifact"
  mkdir -p "$tampered_dir"
  cp "$snapshot_path" "$tampered_dir/$snapshot_file"
  cp "$manifest_path" "$tampered_dir/$manifest_file"
  cp "$signature_path" "$tampered_dir/$signature_file"
  tampered_snapshot="$tampered_dir/$snapshot_file"
  tampered_signature="$tampered_dir/$signature_file"
  python3 - "$tampered_signature" <<'PY'
import base64
from pathlib import Path
import sys

Path(sys.argv[1]).write_text(base64.b64encode(bytes(64)).decode("ascii") + "\n", encoding="utf-8")
PY

  BALANCE_HISTORY_ROOT="$BAD_SIGNATURE_ROOT"
  BH_RPC_PORT=30411
  BALANCE_HISTORY_LOG_FILE="$WORK_DIR/bad-signature.log"
  regtest_create_balance_history_config
  regtest_config_set_max_sync_block_height "$BAD_SIGNATURE_ROOT/config.toml" "$baseline_height"
  regtest_config_set_snapshot_policy \
    "$BAD_SIGNATURE_ROOT/config.toml" signed "" "$trusted_keys"
  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$baseline_height"
  regtest_stop_balance_history

  failure_output="$WORK_DIR/tampered-signature.out"
  regtest_expect_command_failure "$failure_output" "Snapshot signature verification failed" \
    regtest_run_balance_history_cli "$BAD_SIGNATURE_ROOT" install-snapshot \
      --file "$tampered_snapshot"
  assert_no_install_artifacts "$BAD_SIGNATURE_ROOT"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$baseline_height"
  resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['stable_height']" "$baseline_height"
  regtest_assert_json_expr "$resp" "data['result']['stable_block_hash']" "$baseline_hash"
  regtest_stop_balance_history

  BALANCE_HISTORY_ROOT="$UNTRUSTED_ROOT"
  BH_RPC_PORT=30412
  BALANCE_HISTORY_LOG_FILE="$WORK_DIR/untrusted.log"
  regtest_create_balance_history_config
  regtest_config_set_max_sync_block_height "$UNTRUSTED_ROOT/config.toml" "$baseline_height"
  regtest_config_set_snapshot_policy \
    "$UNTRUSTED_ROOT/config.toml" signed "" "$untrusted_keys"
  failure_output="$WORK_DIR/untrusted-signer.out"
  regtest_expect_command_failure "$failure_output" "is not trusted" \
    regtest_run_balance_history_cli "$UNTRUSTED_ROOT" install-snapshot \
      --file "$snapshot_path"
  assert_no_install_artifacts "$UNTRUSTED_ROOT"

  BALANCE_HISTORY_ROOT="$SUCCESS_ROOT"
  BH_RPC_PORT=30413
  BALANCE_HISTORY_LOG_FILE="$WORK_DIR/success.log"
  regtest_create_balance_history_config
  regtest_config_set_max_sync_block_height "$SUCCESS_ROOT/config.toml" "$snapshot_height"
  regtest_config_set_snapshot_policy \
    "$SUCCESS_ROOT/config.toml" signed "" "$trusted_keys"
  regtest_run_balance_history_cli "$SUCCESS_ROOT" install-snapshot \
    --file "$snapshot_path"

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready
  regtest_wait_until_synced_height "$snapshot_height"
  resp="$(regtest_rpc_call_balance_history "get_snapshot_provenance" "[]")"
  regtest_assert_json_expr "$resp" "data['result']['verification_state']" "signature_verified"
  regtest_assert_json_expr "$resp" "data['result']['signature_present']" "True"
  regtest_assert_json_expr "$resp" "data['result']['signature_verified']" "True"
  regtest_assert_json_expr "$resp" "data['result']['signing_key_id']" "exact-signer"

  regtest_log "Signed exact-height snapshot install and rejection paths succeeded."
}

main "$@"
