#!/usr/bin/env bash
set -euo pipefail

minimum_kernel_major=5
minimum_kernel_minor=10
supported_arch="x86_64"
os_release_file="${USDB_HOST_OS_RELEASE_FILE:-/etc/os-release}"
command_dir="${USDB_HOST_COMMAND_DIR:-}"
docker_user=""
temporary_key_file=""
temporary_source_file=""

cleanup() {
  rm -f "${temporary_key_file:-}" "${temporary_source_file:-}"
}

trap cleanup EXIT

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/tools/prepare_usdb_host.sh check [--docker-user USER]
  docker/scripts/tools/prepare_usdb_host.sh install [--docker-user USER]

Actions:
  check    Read-only validation of the Linux kernel, release-image architecture,
           command versions, Docker Compose plugin and Docker daemon access.
           The check works on Linux distributions that expose /etc/os-release.
  install  Install packages on the explicitly supported APT distributions from
           Docker's official repository, enable Docker, optionally add an
           existing user to the docker group, then run the same checks.

Options:
  --docker-user USER
           Verify docker-group membership during check. During install, add the
           existing USER to that group. Membership grants root-level privileges
           and requires a new login session before it is effective.

Runtime floor: Linux kernel 5.10 or newer on x86-64.
Automated install: Ubuntu 22.04/24.04 and Debian 12/13.
The installer never removes conflicting container packages or node data.
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

load_platform() {
  [[ -r "${os_release_file}" ]] || fail "OS release file is not readable: ${os_release_file}"

  local id=""
  local version_id=""
  local version_codename=""
  local ubuntu_codename=""
  # shellcheck disable=SC1090
  source "${os_release_file}"
  id="${ID:-}"
  version_id="${VERSION_ID:-}"
  version_codename="${VERSION_CODENAME:-}"
  ubuntu_codename="${UBUNTU_CODENAME:-}"

  HOST_OS_ID="${id}"
  HOST_OS_VERSION="${version_id}"
  HOST_OS_CODENAME="${ubuntu_codename:-${version_codename}}"
  HOST_ARCH="${USDB_HOST_ARCH:-$(uname -m)}"
  HOST_KERNEL_NAME="${USDB_HOST_KERNEL_NAME:-$(uname -s)}"
  HOST_KERNEL_RELEASE="${USDB_HOST_KERNEL_RELEASE:-$(uname -r)}"
}

kernel_is_supported() {
  local release_core="${HOST_KERNEL_RELEASE%%-*}"
  local major="${release_core%%.*}"
  local remainder="${release_core#*.}"
  local minor="${remainder%%.*}"

  [[ "${major}" =~ ^[0-9]+$ && "${minor}" =~ ^[0-9]+$ ]] || return 1
  ((major > minimum_kernel_major)) || \
    ((major == minimum_kernel_major && minor >= minimum_kernel_minor))
}

install_distribution_supported() {
  case "${HOST_OS_ID}:${HOST_OS_VERSION}" in
    ubuntu:22.04 | ubuntu:24.04 | debian:12 | debian:13)
      return 0
      ;;
    *)
      return 1
      ;;
  esac
}

check_platform() {
  load_platform
  local failures=0

  echo "INFO distribution: ${HOST_OS_ID:-unknown} ${HOST_OS_VERSION:-unknown} (${HOST_OS_CODENAME:-unknown})"

  if [[ "${HOST_KERNEL_NAME}" != "Linux" ]]; then
    echo "FAIL kernel: expected Linux, got ${HOST_KERNEL_NAME}" >&2
    failures=$((failures + 1))
  elif ! kernel_is_supported; then
    echo "FAIL kernel: expected 5.10 or newer, got ${HOST_KERNEL_RELEASE}" >&2
    failures=$((failures + 1))
  else
    echo "PASS kernel: Linux ${HOST_KERNEL_RELEASE}"
  fi

  if [[ "${HOST_ARCH}" != "${supported_arch}" ]]; then
    echo "FAIL architecture: expected ${supported_arch}, got ${HOST_ARCH}" >&2
    failures=$((failures + 1))
  else
    echo "PASS architecture: ${HOST_ARCH}"
  fi

  if install_distribution_supported; then
    echo "INFO installer: automatic APT installation is supported"
  else
    echo "INFO installer: check-only distribution; install dependencies with the native package manager"
  fi

  ((failures == 0))
}

print_version() {
  local label="$1"
  shift
  local output
  if ! output="$("$@" 2>&1)"; then
    echo "FAIL ${label}: version command failed" >&2
    return 1
  fi
  output="${output%%$'\n'*}"
  echo "PASS ${label}: ${output}"
}

check_required_tools() {
  local failures=0
  local docker_bin=""
  local git_bin=""
  local python_bin=""
  local curl_bin=""
  local jq_bin=""

  if docker_bin="$(resolve_command docker)"; then
    print_version "Docker Engine CLI" "${docker_bin}" --version || failures=$((failures + 1))
    print_version "Docker Compose plugin" "${docker_bin}" compose version || failures=$((failures + 1))
  else
    echo "FAIL Docker Engine CLI: docker is missing" >&2
    echo "FAIL Docker Compose plugin: docker is missing" >&2
    failures=$((failures + 2))
  fi

  if git_bin="$(resolve_command git)"; then
    print_version "Git" "${git_bin}" --version || failures=$((failures + 1))
  else
    echo "FAIL Git: git is missing" >&2
    failures=$((failures + 1))
  fi

  if python_bin="$(resolve_command python3)"; then
    print_version "Python 3" "${python_bin}" --version || failures=$((failures + 1))
  else
    echo "FAIL Python 3: python3 is missing" >&2
    failures=$((failures + 1))
  fi

  if curl_bin="$(resolve_command curl)"; then
    print_version "curl" "${curl_bin}" --version || failures=$((failures + 1))
  else
    echo "FAIL curl: curl is missing" >&2
    failures=$((failures + 1))
  fi

  if jq_bin="$(resolve_command jq)"; then
    print_version "jq" "${jq_bin}" --version || failures=$((failures + 1))
  else
    echo "FAIL jq: jq is missing" >&2
    failures=$((failures + 1))
  fi

  ((failures == 0))
}

check_docker_runtime() {
  local allow_root_fallback="${1:-0}"
  local docker_bin
  docker_bin="$(resolve_command docker)" || return 1

  local runtime_info=""
  local info_format='{{.ServerVersion}}|{{.CgroupVersion}}|{{.OSType}}'
  local access_mode="current user"
  if runtime_info="$("${docker_bin}" info --format "${info_format}" 2>/dev/null)"; then
    :
  elif [[ "${allow_root_fallback}" == "1" ]] && runtime_info="$(run_root "${docker_bin}" info --format "${info_format}" 2>/dev/null)"; then
    access_mode="elevated privileges"
  else
    echo "FAIL Docker daemon: daemon is stopped or the current user cannot access its socket" >&2
    return 1
  fi

  local server_version=""
  local cgroup_version=""
  local os_type=""
  IFS='|' read -r server_version cgroup_version os_type <<<"${runtime_info}"
  if [[ -z "${server_version}" || "${os_type}" != "linux" ]]; then
    echo "FAIL Docker daemon: expected a local Linux engine, got ${runtime_info}" >&2
    return 1
  fi
  if [[ "${cgroup_version}" != "1" && "${cgroup_version}" != "2" ]]; then
    echo "FAIL Docker cgroup: expected version 1 or 2, got ${cgroup_version:-unknown}" >&2
    return 1
  fi
  echo "PASS Docker daemon: server ${server_version} is accessible with ${access_mode}"
  echo "PASS Docker cgroup: version ${cgroup_version}"

  if [[ "${USDB_HOST_SKIP_SYSTEMD_CHECK:-0}" != "1" && -d /run/systemd/system ]]; then
    if systemctl is-active --quiet docker.service; then
      echo "PASS Docker service: active"
    else
      echo "FAIL Docker service: docker.service is not active" >&2
      return 1
    fi
  fi
}

check_docker_user() {
  [[ -n "${docker_user}" ]] || return 0
  id "${docker_user}" >/dev/null 2>&1 || {
    echo "FAIL Docker user: user does not exist: ${docker_user}" >&2
    return 1
  }
  if id -nG "${docker_user}" | tr ' ' '\n' | grep -Fxq docker; then
    echo "PASS Docker user: ${docker_user} belongs to the docker group"
  else
    echo "FAIL Docker user: ${docker_user} is not in the docker group" >&2
    return 1
  fi
}

check_host() {
  local allow_root_fallback="${1:-0}"
  local failures=0

  check_platform || failures=$((failures + 1))
  check_required_tools || failures=$((failures + 1))
  check_docker_runtime "${allow_root_fallback}" || failures=$((failures + 1))
  check_docker_user || failures=$((failures + 1))

  if ((failures > 0)); then
    echo "Host prerequisite check failed (${failures} category/categories)." >&2
    return 1
  fi
  echo "Host prerequisite check passed."
}

run_root() {
  if [[ "${EUID}" -eq 0 ]]; then
    "$@"
  elif command -v sudo >/dev/null 2>&1; then
    sudo "$@"
  else
    fail "install requires root privileges or sudo"
  fi
}

installed_package() {
  dpkg-query -W -f='${db:Status-Abbrev}' "$1" 2>/dev/null | grep -q '^ii '
}

reject_conflicting_docker_packages() {
  local packages=(
    docker.io
    docker-compose
    docker-compose-v2
    docker-doc
    docker-buildx
    podman-docker
    containerd
    runc
  )
  local conflicts=()
  local package
  for package in "${packages[@]}"; do
    if installed_package "${package}"; then
      conflicts+=("${package}")
    fi
  done
  if ((${#conflicts[@]} > 0)); then
    echo "Conflicting container packages are installed: ${conflicts[*]}" >&2
    echo "Review workloads and remove conflicts explicitly before rerunning install." >&2
    echo "Suggested command after review: sudo apt-get remove ${conflicts[*]}" >&2
    return 1
  fi
}

install_host() {
  check_platform
  install_distribution_supported || fail \
    "automatic install supports Ubuntu 22.04/24.04 and Debian 12/13; use check after manual installation"
  command -v apt-get >/dev/null 2>&1 || fail "apt-get is required"
  command -v dpkg >/dev/null 2>&1 || fail "dpkg is required"
  command -v dpkg-query >/dev/null 2>&1 || fail "dpkg-query is required"

  local preserve_existing_docker=0
  local docker_bin=""
  if docker_bin="$(command -v docker 2>/dev/null)" && "${docker_bin}" compose version >/dev/null 2>&1; then
    preserve_existing_docker=1
  else
    reject_conflicting_docker_packages
  fi

  run_root apt-get update
  run_root apt-get install -y ca-certificates curl git python3 jq

  if [[ "${preserve_existing_docker}" == "1" ]]; then
    echo "Docker Engine and Compose plugin are already installed; preserving the existing installation."
  else
    temporary_key_file="$(mktemp)"
    temporary_source_file="$(mktemp)"

    run_root install -m 0755 -d /etc/apt/keyrings
    curl -fsSL "https://download.docker.com/linux/${HOST_OS_ID}/gpg" -o "${temporary_key_file}"
    run_root install -m 0644 "${temporary_key_file}" /etc/apt/keyrings/docker.asc

    cat >"${temporary_source_file}" <<EOF
Types: deb
URIs: https://download.docker.com/linux/${HOST_OS_ID}
Suites: ${HOST_OS_CODENAME}
Components: stable
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/docker.asc
EOF
    run_root install -m 0644 "${temporary_source_file}" /etc/apt/sources.list.d/docker.sources
    run_root apt-get update
    run_root apt-get install -y \
      docker-ce \
      docker-ce-cli \
      containerd.io \
      docker-buildx-plugin \
      docker-compose-plugin
  fi

  if command -v systemctl >/dev/null 2>&1 && [[ -d /run/systemd/system ]]; then
    run_root systemctl enable --now docker.service containerd.service
  fi

  if [[ -n "${docker_user}" ]]; then
    id "${docker_user}" >/dev/null 2>&1 || fail "Docker user does not exist: ${docker_user}"
    if ! getent group docker >/dev/null 2>&1; then
      run_root groupadd docker
    fi
    if ! id -nG "${docker_user}" | tr ' ' '\n' | grep -Fxq docker; then
      run_root usermod -aG docker "${docker_user}"
      echo "Added ${docker_user} to the docker group. Start a new login session before deployment."
    fi
    echo "WARNING: docker-group membership grants root-level privileges."
  fi

  check_host 1
}

action="${1:-}"
if [[ -z "${action}" || "${action}" == "help" || "${action}" == "--help" || "${action}" == "-h" ]]; then
  usage
  exit 0
fi
shift

while (($# > 0)); do
  case "$1" in
    --docker-user)
      (($# >= 2)) || fail "--docker-user requires a value"
      docker_user="$2"
      shift 2
      ;;
    *)
      fail "unknown argument: $1"
      ;;
  esac
done

case "${action}" in
  check)
    check_host 0
    ;;
  install)
    install_host
    ;;
  *)
    echo "Unknown action: ${action}" >&2
    usage >&2
    exit 1
    ;;
esac
