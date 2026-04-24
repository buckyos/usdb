#!/usr/bin/env bash
set -euo pipefail

host="${1:?host is required}"
port="${2:?port is required}"
timeout_secs="${3:-60}"
deadline=$((SECONDS + timeout_secs))

while (( SECONDS < deadline )); do
  if (echo >/dev/tcp/"${host}"/"${port}") >/dev/null 2>&1; then
    exit 0
  fi
  sleep 1
done

echo "Timed out waiting for TCP endpoint ${host}:${port}" >&2
exit 1
