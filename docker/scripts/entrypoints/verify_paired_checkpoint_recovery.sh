#!/usr/bin/env bash
set -euo pipefail

snapshot_mode="${SNAPSHOT_MODE:-none}"
if [[ "${snapshot_mode}" != "paired-checkpoint" ]]; then
  echo "Paired checkpoint recovery verification disabled (SNAPSHOT_MODE=${snapshot_mode})"
  exit 0
fi

checkpoint_manifest="${USDB_INDEXER_CHECKPOINT_MANIFEST:-}"
trusted_keys="${BH_SNAPSHOT_TRUSTED_KEYS_FILE:-}"
if [[ -z "${checkpoint_manifest}" || -z "${trusted_keys}" ]]; then
  echo "Paired checkpoint recovery verification requires checkpoint manifest and trusted keys" >&2
  exit 1
fi

exec usdb-indexer-checkpoint-tool verify-recovery \
  --checkpoint-manifest "${checkpoint_manifest}" \
  --trusted-keys "${trusted_keys}" \
  --indexer-root "${USDB_INDEXER_ROOT_DIR:-/data/usdb-indexer}" \
  --indexer-rpc-url "${USDB_INDEXER_RPC_URL:-http://usdb-indexer:28020}" \
  --balance-history-rpc-url "${BALANCE_HISTORY_RPC_URL:-http://balance-history:28010}" \
  --readiness-timeout-secs "${USDB_CHECKPOINT_RECOVERY_TIMEOUT_SECS:-300}"
