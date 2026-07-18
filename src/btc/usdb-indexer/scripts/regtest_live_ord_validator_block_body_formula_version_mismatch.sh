#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

export MISMATCH_CASE_NAME="formula-version-mismatch"
export MISMATCH_FIELD="usdb_index_formula_version"
export MISMATCH_VALUE="pass-energy-formula:unexpected"
export MISMATCH_ERROR_CODE="-32052"
export MISMATCH_ERROR_MESSAGE="FORMULA_VERSION_MISMATCH"
export REGTEST_LOG_PREFIX="[usdb-validator-block-body-formula-version-mismatch]"
export WALLET_NAME="${WALLET_NAME:-usdbvalidatorformulamismatch}"
export ORD_WALLET_NAME="${ORD_WALLET_NAME:-ord-validator-formula-mismatch-a}"
export ORD_WALLET_NAME_B="${ORD_WALLET_NAME_B:-ord-validator-formula-mismatch-b}"
export BTC_RPC_PORT="${BTC_RPC_PORT:-31032}"
export BTC_P2P_PORT="${BTC_P2P_PORT:-31033}"
export BH_RPC_PORT="${BH_RPC_PORT:-31010}"
export USDB_RPC_PORT="${USDB_RPC_PORT:-31020}"
export ORD_RPC_PORT="${ORD_RPC_PORT:-31030}"

exec bash "${SCRIPT_DIR}/regtest_live_ord_validator_block_body_protocol_version_mismatch.sh" "$@"
