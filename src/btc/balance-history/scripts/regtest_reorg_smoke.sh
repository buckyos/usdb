#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
WORK_DIR="${WORK_DIR:-$(mktemp -d /tmp/usdb-bh-reorg-XXXXXX)}"
BITCOIN_DIR="${BITCOIN_DIR:-$WORK_DIR/bitcoin}"
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
BALANCE_HISTORY_ROOT="${BALANCE_HISTORY_ROOT:-$WORK_DIR/balance-history}"
BTC_RPC_PORT="${BTC_RPC_PORT:-28232}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28233}"
BH_RPC_PORT="${BH_RPC_PORT:-28210}"
WALLET_NAME="${WALLET_NAME:-bhreorg}"
TARGET_HEIGHT="${TARGET_HEIGHT:-40}"
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-120}"
BALANCE_HISTORY_LOG_FILE="${BALANCE_HISTORY_LOG_FILE:-$WORK_DIR/balance-history.log}"
REGTEST_LOG_PREFIX="[reorg-smoke]"

source "${SCRIPT_DIR}/regtest_lib.sh"

main() {
  trap regtest_cleanup EXIT

  regtest_resolve_bitcoin_binaries
  regtest_require_cmd cargo
  regtest_require_cmd curl
  regtest_require_cmd python3

  regtest_ensure_workspace_dirs

  regtest_start_bitcoind
  regtest_ensure_wallet

  local mining_address
  mining_address="$(regtest_get_new_address)"
  regtest_log "Mining ${TARGET_HEIGHT} blocks to address=${mining_address}"
  regtest_mine_blocks "$TARGET_HEIGHT" "$mining_address"

  regtest_create_balance_history_config

  regtest_start_balance_history
  regtest_wait_balance_history_rpc_ready

  regtest_wait_until_synced_height "$TARGET_HEIGHT"

  local original_hash old_commit_resp old_commit_hash replacement_hash snapshot_resp snapshot_hash replacement_address
  original_hash="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$TARGET_HEIGHT")"
  old_commit_resp="$(regtest_rpc_call_balance_history "get_block_commit" "[${TARGET_HEIGHT}]")"
  old_commit_hash="$(echo "$old_commit_resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result"); print((r or {}).get("btc_block_hash", ""))')"

  regtest_log "Pre-reorg hashes: node=${original_hash}, service=${old_commit_hash}"
  if [[ "$old_commit_hash" != "$original_hash" ]]; then
    regtest_log "Initial service block commit does not match node tip hash: ${old_commit_resp}"
    exit 1
  fi

  regtest_log "Triggering regtest reorg by invalidating tip and immediately mining a replacement block"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" invalidateblock "$original_hash"
  replacement_address="$(regtest_get_new_address)"
  regtest_log "Mining replacement block to fresh address=${replacement_address}"
  regtest_mine_blocks 1 "$replacement_address"

  replacement_hash="$("$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" getblockhash "$TARGET_HEIGHT")"
  regtest_log "Replacement tip hash=${replacement_hash}"
  if [[ "$replacement_hash" == "$original_hash" ]]; then
    regtest_log "Reorg failed: replacement hash matches original hash"
    exit 1
  fi

  regtest_wait_until_block_commit_hash "$TARGET_HEIGHT" "$replacement_hash"

  snapshot_resp="$(regtest_rpc_call_balance_history "get_snapshot_info" "[]")"
  snapshot_hash="$(echo "$snapshot_resp" | regtest_json_extract_python 'import json,sys; d=json.load(sys.stdin); r=d.get("result") or {}; print(r.get("stable_block_hash", ""))')"
  regtest_log "Snapshot info stable_block_hash=${snapshot_hash}"
  if [[ "$snapshot_hash" != "$replacement_hash" ]]; then
    regtest_log "Snapshot info did not converge to replacement hash: ${snapshot_resp}"
    exit 1
  fi

  regtest_log "Reorg smoke test succeeded."
  regtest_log "Logs: ${BALANCE_HISTORY_LOG_FILE}"
}

main "$@"