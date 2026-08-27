#!/usr/bin/env bash
set -euo pipefail

command_dir="${USDB_FIREWALL_COMMAND_DIR:-}"
node_env=""
ssh_port=""
bitcoin_p2p_mode="private"
confirmed=0

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/tools/prepare_usdb_firewall.sh check \
    --node-env PATH --ssh-port PORT [--bitcoin-p2p private|public]
  sudo docker/scripts/tools/prepare_usdb_firewall.sh apply \
    --node-env PATH --ssh-port PORT [--bitcoin-p2p private|public] --confirm

Actions:
  check    Validate node.env bind addresses and the active UFW policy.
  apply    Install UFW on APT hosts when needed, add the required rules, enable
           the firewall, and run the same validation. Existing unrelated rules
           are preserved.

Options:
  --node-env PATH
           Uncommitted node.env whose Docker bind addresses define the actual
           container exposure boundary.
  --ssh-port PORT
           Explicit operator SSH port to preserve before enabling UFW.
  --bitcoin-p2p private|public
           private requires BTC_P2P_BIND_ADDRESS=127.0.0.1 and does not expose
           8333/TCP. public requires 0.0.0.0 and allows 8333/TCP.
  --confirm
           Required by apply because enabling a firewall can affect remote
           access. Not accepted as implicit confirmation from an environment
           variable.

USDB devp2p always requires 31303/TCP+UDP. RPC and service ports must remain
bound to 127.0.0.1. Docker-published ports can bypass UFW, so bind validation is
part of both actions and is not optional.
EOF
}

fail() {
  echo "ERROR: $*" >&2
  exit 1
}

resolve_command() {
  local name="$1"
  if [[ -n "${command_dir}" ]]; then
    if [[ -x "${command_dir}/${name}" ]]; then
      printf '%s\n' "${command_dir}/${name}"
      return 0
    fi
    return 1
  fi
  command -v "${name}"
}

run_root() {
  if [[ "${USDB_FIREWALL_SKIP_ROOT:-0}" == "1" || "${EUID}" -eq 0 ]]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    fail "this action requires root privileges or sudo"
  fi
}

validate_port() {
  local label="$1"
  local value="$2"
  [[ "${value}" =~ ^[0-9]+$ ]] || fail "${label} must be a decimal port"
  ((value >= 1 && value <= 65535)) || fail "${label} must be between 1 and 65535"
}

node_env_value() {
  local key="$1"
  local value
  value="$(awk -F= -v key="${key}" '
    $1 == key {
      sub(/^[^=]*=/, "")
      print
      found = 1
    }
    END { if (!found) exit 1 }
  ' "${node_env}")" || fail "node.env is missing ${key}: ${node_env}"
  value="${value%$'\r'}"
  printf '%s\n' "${value}"
}

require_env_value() {
  local key="$1"
  local expected="$2"
  local actual
  actual="$(node_env_value "${key}")"
  [[ "${actual}" == "${expected}" ]] || fail \
    "${key} must be ${expected} for this firewall profile, got ${actual:-<empty>}"
}

validate_node_bindings() {
  [[ -r "${node_env}" ]] || fail "node.env is not readable: ${node_env}"

  require_env_value "BTC_P2P_BIND_PORT" "8333"
  require_env_value "USDB_P2P_BIND_ADDRESS" "0.0.0.0"
  require_env_value "USDB_P2P_BIND_PORT" "31303"

  if [[ "${bitcoin_p2p_mode}" == "private" ]]; then
    require_env_value "BTC_P2P_BIND_ADDRESS" "127.0.0.1"
  else
    require_env_value "BTC_P2P_BIND_ADDRESS" "0.0.0.0"
  fi

  local key
  for key in \
    USDB_HTTP_BIND_ADDRESS \
    USDB_WS_BIND_ADDRESS \
    BH_BIND_ADDRESS \
    USDB_INDEXER_BIND_ADDRESS \
    CONTROL_PLANE_BIND_ADDRESS; do
    require_env_value "${key}" "127.0.0.1"
  done

  echo "PASS node bindings: USDB devp2p is public; operator APIs are loopback-only"
  if [[ "${bitcoin_p2p_mode}" == "private" ]]; then
    echo "PASS Bitcoin P2P binding: loopback-only; outbound peer connections remain available"
  else
    echo "PASS Bitcoin P2P binding: public 8333/TCP"
  fi
}

ufw_status() {
  local ufw_bin="$1"
  run_root "${ufw_bin}" status verbose
}

has_allow_rule() {
  local status="$1"
  local port="$2"
  local protocol="$3"
  awk -v endpoint="${port}/${protocol}" '
    $1 == endpoint && $2 == "ALLOW" { found = 1 }
    END { exit(found ? 0 : 1) }
  ' <<<"${status}"
}

validate_ufw_status() {
  local status="$1"
  local failures=0

  if grep -Fqx "Status: active" <<<"${status}"; then
    echo "PASS UFW status: active"
  else
    echo "FAIL UFW status: expected active" >&2
    failures=$((failures + 1))
  fi

  if grep -Eq '^Default: deny \(incoming\), allow \(outgoing\),' <<<"${status}"; then
    echo "PASS UFW defaults: deny incoming, allow outgoing"
  else
    echo "FAIL UFW defaults: expected deny incoming and allow outgoing" >&2
    failures=$((failures + 1))
  fi

  local endpoint
  for endpoint in "${ssh_port}/tcp" "31303/tcp" "31303/udp"; do
    if has_allow_rule "${status}" "${endpoint%/*}" "${endpoint#*/}"; then
      echo "PASS UFW allow: ${endpoint}"
    else
      echo "FAIL UFW allow: missing ${endpoint}" >&2
      failures=$((failures + 1))
    fi
  done

  if [[ "${bitcoin_p2p_mode}" == "public" ]]; then
    if has_allow_rule "${status}" "8333" "tcp"; then
      echo "PASS UFW allow: 8333/tcp"
    else
      echo "FAIL UFW allow: missing 8333/tcp for public Bitcoin P2P" >&2
      failures=$((failures + 1))
    fi
  elif has_allow_rule "${status}" "8333" "tcp"; then
    echo "FAIL UFW allow: 8333/tcp must not be allowed in private Bitcoin P2P mode" >&2
    failures=$((failures + 1))
  else
    echo "PASS UFW Bitcoin policy: 8333/tcp is not allowed"
  fi

  local port
  for port in 8332 8545 8546 28010 28020 28040; do
    if has_allow_rule "${status}" "${port}" "tcp"; then
      echo "FAIL UFW policy: sensitive port ${port}/tcp is explicitly allowed" >&2
      failures=$((failures + 1))
    fi
  done

  ((failures == 0))
}

check_firewall() {
  validate_node_bindings

  local ufw_bin
  ufw_bin="$(resolve_command ufw)" || fail "ufw is not installed"
  local status
  status="$(ufw_status "${ufw_bin}")" || fail "failed to read UFW status"
  validate_ufw_status "${status}" || fail "firewall check failed"

  echo "Firewall check passed. Verify equivalent ingress rules in any upstream cloud firewall."
}

install_ufw_if_needed() {
  if resolve_command ufw >/dev/null 2>&1; then
    return 0
  fi

  local apt_get
  apt_get="$(resolve_command apt-get)" || fail \
    "ufw is missing; install it with the host distribution package manager"
  run_root "${apt_get}" update
  run_root "${apt_get}" install -y ufw
}

apply_firewall() {
  [[ "${confirmed}" == "1" ]] || fail "apply requires --confirm"
  validate_node_bindings
  install_ufw_if_needed

  local ufw_bin
  ufw_bin="$(resolve_command ufw)" || fail "ufw installation did not provide the ufw command"

  # Preserve SSH before changing the default inbound policy to avoid locking
  # out the operator who is applying the rules over a remote session.
  run_root "${ufw_bin}" allow "${ssh_port}/tcp" comment "USDB operator SSH"
  run_root "${ufw_bin}" allow "31303/tcp" comment "USDB devp2p TCP"
  run_root "${ufw_bin}" allow "31303/udp" comment "USDB devp2p discovery"

  if [[ "${bitcoin_p2p_mode}" == "public" ]]; then
    run_root "${ufw_bin}" allow "8333/tcp" comment "Bitcoin P2P"
  else
    local status
    status="$(ufw_status "${ufw_bin}" 2>/dev/null || true)"
    if has_allow_rule "${status}" "8333" "tcp"; then
      run_root "${ufw_bin}" --force delete allow "8333/tcp"
    fi
  fi

  run_root "${ufw_bin}" default deny incoming
  run_root "${ufw_bin}" default allow outgoing
  run_root "${ufw_bin}" logging low
  run_root "${ufw_bin}" --force enable

  check_firewall
}

action="${1:-}"
if [[ -z "${action}" || "${action}" == "help" || "${action}" == "--help" || "${action}" == "-h" ]]; then
  usage
  exit 0
fi
shift

while (($# > 0)); do
  case "$1" in
    --node-env)
      (($# >= 2)) || fail "--node-env requires a value"
      node_env="$2"
      shift 2
      ;;
    --ssh-port)
      (($# >= 2)) || fail "--ssh-port requires a value"
      ssh_port="$2"
      shift 2
      ;;
    --bitcoin-p2p)
      (($# >= 2)) || fail "--bitcoin-p2p requires a value"
      bitcoin_p2p_mode="$2"
      shift 2
      ;;
    --confirm)
      confirmed=1
      shift
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

[[ -n "${node_env}" ]] || fail "--node-env is required"
[[ -n "${ssh_port}" ]] || fail "--ssh-port is required"
validate_port "--ssh-port" "${ssh_port}"
[[ "${bitcoin_p2p_mode}" == "private" || "${bitcoin_p2p_mode}" == "public" ]] || fail \
  "--bitcoin-p2p must be private or public"

case "${action}" in
  check)
    check_firewall
    ;;
  apply)
    apply_firewall
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
