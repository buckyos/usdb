#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../../../.." && pwd)"
LOG_PREFIX="[balance-history-regtest-suite]"

KEEP_GOING=0
DRY_RUN=0
LIST_ONLY=0
SUITE="smoke"
SUITE_SET=0

usage() {
  cat <<'USAGE'
Usage:
  bash src/btc/balance-history/scripts/run_regtest_suite.sh [suite] [options]

Suites:
  smoke   Curated smoke suite covering sync, RPC semantics, reorg, snapshot repeat install, and oracle balance checks.

Options:
  --list        Print scripts in the selected suite and exit.
  --dry-run     Print commands without executing them.
  --keep-going  Continue running remaining scripts after a failure.
  -h, --help    Show this help.

Examples:
  bash src/btc/balance-history/scripts/run_regtest_suite.sh
  bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke
  bash src/btc/balance-history/scripts/run_regtest_suite.sh smoke --dry-run
USAGE
}

log() {
  echo "${LOG_PREFIX} $*"
}

select_suite() {
  local suite="$1"

  case "$suite" in
    smoke)
      SUITE_SCRIPTS=(
        regtest_smoke.sh
        regtest_rpc_semantics.sh
        regtest_reorg_smoke.sh
        regtest_snapshot_install_repeat.sh
        regtest_history_balance_oracle.sh
      )
      ;;
    *)
      echo "Unknown balance-history regtest suite: ${suite}" >&2
      usage >&2
      exit 2
      ;;
  esac
}

parse_args() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --list)
        LIST_ONLY=1
        ;;
      --dry-run)
        DRY_RUN=1
        ;;
      --keep-going)
        KEEP_GOING=1
        ;;
      -h|--help)
        usage
        exit 0
        ;;
      -*)
        echo "Unknown option: $1" >&2
        usage >&2
        exit 2
        ;;
      *)
        if [[ "$SUITE_SET" == "1" ]]; then
          echo "Only one suite argument is supported." >&2
          usage >&2
          exit 2
        fi
        SUITE="$1"
        SUITE_SET=1
        ;;
    esac
    shift
  done
}

print_suite() {
  local script

  log "Suite '${SUITE}' contains ${#SUITE_SCRIPTS[@]} scripts:"
  for script in "${SUITE_SCRIPTS[@]}"; do
    echo "  ${script}"
  done
}

run_script() {
  local script="$1"
  local script_path="${SCRIPT_DIR}/${script}"
  local started ended duration exit_code

  if [[ ! -f "$script_path" ]]; then
    log "Missing script: ${script_path}"
    return 127
  fi

  started="$(date +%s)"
  log "START ${script}"

  if [[ "$DRY_RUN" == "1" ]]; then
    echo "  (cd ${REPO_ROOT} && bash ${script_path})"
    return 0
  fi

  set +e
  (
    cd "$REPO_ROOT"
    bash "$script_path"
  )
  exit_code=$?
  set -e

  ended="$(date +%s)"
  duration=$((ended - started))

  if [[ "$exit_code" == "0" ]]; then
    log "PASS ${script} (${duration}s)"
  else
    log "FAIL ${script} (${duration}s, exit=${exit_code})"
  fi

  return "$exit_code"
}

main() {
  local script
  local failures=()

  parse_args "$@"
  select_suite "$SUITE"

  if [[ "$LIST_ONLY" == "1" ]]; then
    print_suite
    exit 0
  fi

  log "Running suite '${SUITE}' from repo root: ${REPO_ROOT}"
  if [[ "$DRY_RUN" == "1" ]]; then
    log "Dry-run mode enabled; scripts will not be executed."
  fi

  for script in "${SUITE_SCRIPTS[@]}"; do
    if ! run_script "$script"; then
      failures+=("$script")
      if [[ "$KEEP_GOING" != "1" ]]; then
        break
      fi
    fi
  done

  if [[ "${#failures[@]}" -gt 0 ]]; then
    log "Suite '${SUITE}' failed: ${failures[*]}"
    exit 1
  fi

  log "Suite '${SUITE}' passed."
}

main "$@"
