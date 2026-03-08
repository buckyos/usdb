#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WORLD_SIM_SCRIPT="${SCRIPT_DIR}/regtest_world_sim.sh"

# Base workspace for this long-running session.
WORK_DIR="${WORK_DIR:-/tmp/usdb-world-live}"

# Bitcoin Core binaries directory (contains bitcoind and bitcoin-cli).
BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR:-/home/bucky/btc/bitcoin-28.1/bin}"
# ord executable used for mint/transfer transaction construction.
ORD_BIN="${ORD_BIN:-/home/bucky/ord/target/release/ord}"

# RPC and p2p ports for isolated local services.
BTC_RPC_PORT="${BTC_RPC_PORT:-28132}"
BTC_P2P_PORT="${BTC_P2P_PORT:-28133}"
BH_RPC_PORT="${BH_RPC_PORT:-28110}"
USDB_RPC_PORT="${USDB_RPC_PORT:-28120}"
ORD_SERVER_PORT="${ORD_SERVER_PORT:-28130}"

# Total agent wallets created for simulation.
AGENT_COUNT="${AGENT_COUNT:-200}"
# Number of premine blocks to bootstrap spendable regtest coins.
PREMINE_BLOCKS="${PREMINE_BLOCKS:-220}"
# Initial BTC funded to each agent wallet.
FUND_AGENT_AMOUNT_BTC="${FUND_AGENT_AMOUNT_BTC:-8.0}"
# Confirm blocks mined after the initial funding round.
FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS:-2}"
# Max seconds waiting for indexer/balance-history catch-up.
SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC:-1800}"

# Number of blocks to simulate (set 0 for infinite run).
SIM_BLOCKS="${SIM_BLOCKS:-5000}"
# Deterministic random seed for reproducible behavior.
SIM_SEED="${SIM_SEED:-20260308}"
# Fee rate used by ord wallet operations.
SIM_FEE_RATE="${SIM_FEE_RATE:-1}"
# Max operations submitted in each simulated block.
SIM_MAX_ACTIONS_PER_BLOCK="${SIM_MAX_ACTIONS_PER_BLOCK:-8}"
# Sleep between blocks to slow down the run for observation.
SIM_SLEEP_MS_BETWEEN_BLOCKS="${SIM_SLEEP_MS_BETWEEN_BLOCKS:-120}"
# Stop immediately when one action fails (1=true, 0=false).
SIM_FAIL_FAST="${SIM_FAIL_FAST:-0}"

# Initial active agents before growth starts.
SIM_INITIAL_ACTIVE_AGENTS="${SIM_INITIAL_ACTIVE_AGENTS:-30}"
# Expand active agents every N blocks.
SIM_AGENT_GROWTH_INTERVAL_BLOCKS="${SIM_AGENT_GROWTH_INTERVAL_BLOCKS:-25}"
# Number of agents activated at each growth step.
SIM_AGENT_GROWTH_STEP="${SIM_AGENT_GROWTH_STEP:-4}"

# Agent action policy: adaptive or scripted.
SIM_POLICY_MODE="${SIM_POLICY_MODE:-adaptive}"
# Action cycle used only when SIM_POLICY_MODE=scripted.
SIM_SCRIPTED_CYCLE="${SIM_SCRIPTED_CYCLE:-mint,send_balance,transfer,remint,spend_balance,noop}"

# Action probability weights in adaptive mode.
SIM_MINT_PROBABILITY="${SIM_MINT_PROBABILITY:-0.24}"
SIM_INVALID_MINT_PROBABILITY="${SIM_INVALID_MINT_PROBABILITY:-0.03}"
SIM_TRANSFER_PROBABILITY="${SIM_TRANSFER_PROBABILITY:-0.22}"
SIM_REMINT_PROBABILITY="${SIM_REMINT_PROBABILITY:-0.15}"
SIM_SEND_PROBABILITY="${SIM_SEND_PROBABILITY:-0.24}"
SIM_SPEND_PROBABILITY="${SIM_SPEND_PROBABILITY:-0.12}"

# Structured JSONL report toggle and path.
SIM_REPORT_ENABLED="${SIM_REPORT_ENABLED:-1}"
SIM_REPORT_FILE="${SIM_REPORT_FILE:-${WORK_DIR}/world-sim-200x5000.jsonl}"
# Flush report after every N events.
SIM_REPORT_FLUSH_EVERY="${SIM_REPORT_FLUSH_EVERY:-1}"
# Number of lines printed per log file when failure diagnostics triggers.
DIAG_TAIL_LINES="${DIAG_TAIL_LINES:-200}"

log() {
  echo "[run-live] $*"
}

main() {
  if [[ ! -x "${WORLD_SIM_SCRIPT}" ]]; then
    echo "Missing executable script: ${WORLD_SIM_SCRIPT}" >&2
    exit 1
  fi

  log "Starting live world simulation with AGENT_COUNT=${AGENT_COUNT}, SIM_BLOCKS=${SIM_BLOCKS}"
  log "USDB RPC endpoint: http://127.0.0.1:${USDB_RPC_PORT}"
  log "Report file: ${SIM_REPORT_FILE}"

  env \
    WORK_DIR="${WORK_DIR}" \
    BITCOIN_BIN_DIR="${BITCOIN_BIN_DIR}" \
    ORD_BIN="${ORD_BIN}" \
    BTC_RPC_PORT="${BTC_RPC_PORT}" \
    BTC_P2P_PORT="${BTC_P2P_PORT}" \
    BH_RPC_PORT="${BH_RPC_PORT}" \
    USDB_RPC_PORT="${USDB_RPC_PORT}" \
    ORD_SERVER_PORT="${ORD_SERVER_PORT}" \
    AGENT_COUNT="${AGENT_COUNT}" \
    PREMINE_BLOCKS="${PREMINE_BLOCKS}" \
    FUND_AGENT_AMOUNT_BTC="${FUND_AGENT_AMOUNT_BTC}" \
    FUND_CONFIRM_BLOCKS="${FUND_CONFIRM_BLOCKS}" \
    SYNC_TIMEOUT_SEC="${SYNC_TIMEOUT_SEC}" \
    SIM_BLOCKS="${SIM_BLOCKS}" \
    SIM_SEED="${SIM_SEED}" \
    SIM_FEE_RATE="${SIM_FEE_RATE}" \
    SIM_MAX_ACTIONS_PER_BLOCK="${SIM_MAX_ACTIONS_PER_BLOCK}" \
    SIM_SLEEP_MS_BETWEEN_BLOCKS="${SIM_SLEEP_MS_BETWEEN_BLOCKS}" \
    SIM_FAIL_FAST="${SIM_FAIL_FAST}" \
    SIM_INITIAL_ACTIVE_AGENTS="${SIM_INITIAL_ACTIVE_AGENTS}" \
    SIM_AGENT_GROWTH_INTERVAL_BLOCKS="${SIM_AGENT_GROWTH_INTERVAL_BLOCKS}" \
    SIM_AGENT_GROWTH_STEP="${SIM_AGENT_GROWTH_STEP}" \
    SIM_POLICY_MODE="${SIM_POLICY_MODE}" \
    SIM_SCRIPTED_CYCLE="${SIM_SCRIPTED_CYCLE}" \
    SIM_MINT_PROBABILITY="${SIM_MINT_PROBABILITY}" \
    SIM_INVALID_MINT_PROBABILITY="${SIM_INVALID_MINT_PROBABILITY}" \
    SIM_TRANSFER_PROBABILITY="${SIM_TRANSFER_PROBABILITY}" \
    SIM_REMINT_PROBABILITY="${SIM_REMINT_PROBABILITY}" \
    SIM_SEND_PROBABILITY="${SIM_SEND_PROBABILITY}" \
    SIM_SPEND_PROBABILITY="${SIM_SPEND_PROBABILITY}" \
    SIM_REPORT_ENABLED="${SIM_REPORT_ENABLED}" \
    SIM_REPORT_FILE="${SIM_REPORT_FILE}" \
    SIM_REPORT_FLUSH_EVERY="${SIM_REPORT_FLUSH_EVERY}" \
    DIAG_TAIL_LINES="${DIAG_TAIL_LINES}" \
    "${WORLD_SIM_SCRIPT}"
}

main "$@"
