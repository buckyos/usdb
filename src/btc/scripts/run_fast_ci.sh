#!/usr/bin/env bash
set -euo pipefail

BTC_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
REPO_DIR=$(cd "$BTC_DIR/../.." && pwd)
MANIFEST="$BTC_DIR/Cargo.toml"
INDEXER_SCRIPTS="$BTC_DIR/usdb-indexer/scripts"

log() {
  echo "[usdb-rust-fast] $*"
}

require_command() {
  local command="$1"
  if ! command -v "$command" >/dev/null; then
    echo "required command is unavailable: $command" >&2
    exit 1
  fi
}

for command in cargo rustc shellcheck python3; do
  require_command "$command"
done

cd "$BTC_DIR"
log "toolchains: $(rustc --version); $(cargo --version); $(python3 --version 2>&1)"
log "checking Rust formatting and Clippy"
cargo fmt --manifest-path "$MANIFEST" --all -- --check
cargo clippy --manifest-path "$MANIFEST" --workspace --all-targets -- -D warnings

log "running Rust workspace tests"
cargo test --manifest-path "$MANIFEST" --workspace

log "checking indexer shell scripts"
(
  local_scripts="src/btc/usdb-indexer/scripts"
  declare -a shell_files
  cd "$REPO_DIR"
  mapfile -d '' shell_files < <(find "$local_scripts" -maxdepth 1 -type f -name '*.sh' -print0)
  shellcheck -x -P "$local_scripts" "${shell_files[@]}"
)

log "running world simulator tests"
env PYTHONDONTWRITEBYTECODE=1 python3 "$INDEXER_SCRIPTS/test_regtest_world_simulator.py"

log "checking Docker P2P defaults"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_p2p_defaults.py"

log "checking balance-history memory profile"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_balance_history_memory_profile.py"

log "checking testnet network and release manifests"
shellcheck \
  "$REPO_DIR/docker/scripts/entrypoints/start_bitcoin_core.sh" \
  "$REPO_DIR/docker/scripts/tools/prepare_usdb_host.sh" \
  "$REPO_DIR/docker/scripts/tools/run_testnet_bitcoin.sh" \
  "$REPO_DIR/docker/scripts/tools/run_testnet_runtime.sh"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_validate_network_bundle.py"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_release_manifest.py"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_release_candidate_resolver.py"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_check_bitcoin_readiness.py"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_check_json_rpc_readiness.py"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_generate_bitcoin_rpcauth.py"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_testnet_bitcoin_release.py"
env PYTHONDONTWRITEBYTECODE=1 python3 "$REPO_DIR/docker/scripts/tools/test_prepare_usdb_host.py"
log "Rust fast gate passed"
