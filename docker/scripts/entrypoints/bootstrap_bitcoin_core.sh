#!/usr/bin/env bash
set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if [[ "${SNAPSHOT_MODE:-none}" != "assumeutxo" ]]; then
  echo "Bitcoin snapshot bootstrap requires SNAPSHOT_MODE=assumeutxo" >&2
  exit 1
fi
exec python3 "${script_dir}/../tools/bitcoin_assumeutxo.py" bootstrap "$@"
