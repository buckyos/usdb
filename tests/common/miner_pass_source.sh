#!/usr/bin/env bash

# Resolve the actual inscription sat source, without spending a previous pass as funding.
regtest_select_cardinal_satpoint() {
  local wallet_name="$1" source_address="$2" evidence_dir satpoint
  evidence_dir="$(mktemp -d "$WORK_DIR/source-selection-XXXXXX")"
  "$BITCOIN_CLI_BIN" -regtest -datadir="$BITCOIN_DIR" -rpcport="$BTC_RPC_PORT" \
    -rpcwallet="$wallet_name" listunspent 1 >"$evidence_dir/unspent.json" || return 1
  regtest_run_ord_wallet_named "$wallet_name" inscriptions >"$evidence_dir/inscriptions.json" || return 1
  satpoint="$(python3 "$REPO_ROOT/tests/common/miner_pass_regtest.py" select-satpoint \
    --unspent "$evidence_dir/unspent.json" --inscriptions "$evidence_dir/inscriptions.json" \
    --address "$source_address")" || return 1
  echo "${REGTEST_LOG_PREFIX:-[usdb-indexer-reorg]} Selected source: address=${source_address}, satpoint=${satpoint}, evidence=${evidence_dir}" >&2
  echo "$satpoint"
}

