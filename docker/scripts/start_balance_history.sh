#!/usr/bin/env bash
set -euo pipefail

root_dir="${BH_ROOT_DIR:-/data/balance-history}"
config_path="${root_dir}/config.toml"

/opt/usdb/docker/scripts/render_balance_history_config.sh "${config_path}"

btc_url="${BTC_RPC_URL:-http://btc-node:8332}"
btc_target="${btc_url#*://}"
btc_host="${btc_target%%:*}"
btc_port="${btc_target##*:}"

/opt/usdb/docker/scripts/wait_for_tcp.sh \
  "${btc_host}" \
  "${btc_port}" \
  "${WAIT_FOR_BTC_TIMEOUT_SECS:-120}"

if [[ -n "${BH_EXTRA_ARGS:-}" ]]; then
  # shellcheck disable=SC2086
  exec balance-history --root-dir "${root_dir}" ${BH_EXTRA_ARGS}
fi

exec balance-history --root-dir "${root_dir}"
