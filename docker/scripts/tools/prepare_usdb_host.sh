#!/usr/bin/env bash
set -euo pipefail

minimum_kernel_major=5
minimum_kernel_minor=10
supported_arch="x86_64"
os_release_file="${USDB_HOST_OS_RELEASE_FILE:-/etc/os-release}"
command_dir="${USDB_HOST_COMMAND_DIR:-}"
apt_sources_dir="${USDB_HOST_APT_SOURCES_DIR:-/etc/apt/sources.list.d}"
docker_user=""
docker_mirror="auto"
# SHA-256 of Docker's public APT signing key, verified against both official
# Ubuntu/Debian endpoints. Mirror fallback must not introduce a different key.
docker_key_sha256="1500c1f56fa9e26b9b8f42452a553675796ade0807cdce11975eb98170b3a570"
docker_packages=(docker-ce docker-ce-cli containerd.io docker-buildx-plugin docker-compose-plugin)
temporary_key_file=""
temporary_source_file=""
temporary_apt_sources_dir=""

cleanup() {
  rm -f "${temporary_key_file:-}" "${temporary_source_file:-}"
  if [[ -n "${temporary_apt_sources_dir}" ]]; then
    rm -f "${temporary_apt_sources_dir}"/*
    rmdir "${temporary_apt_sources_dir}"
  fi
}

trap cleanup EXIT

usage() {
  cat <<'EOF'
Usage:
  docker/scripts/tools/prepare_usdb_host.sh check [--docker-user USER]
  docker/scripts/tools/prepare_usdb_host.sh install [--docker-user USER] [--docker-mirror auto|official|tuna]

Actions:
  check    Read-only validation of the Linux kernel, release-image architecture,
           command versions, Docker Compose plugin and Docker daemon access.
           The check works on Linux distributions that expose /etc/os-release.
  install  Install packages on the explicitly supported APT distributions from
           Docker's repository or its Tsinghua mirror, enable Docker, optionally add an
           existing user to the docker group, then run the same checks.

Options:
  --docker-mirror auto|official|tuna
           Installation source (default: auto). Auto tries Docker's official
           repository, then the Tsinghua Docker CE mirror if downloads fail.
           Explicit official/tuna selection disables source fallback.
           HTTPS, the pinned Docker signing key and APT signatures are checked.
  --docker-user USER
           Verify docker-group membership during check. During install, add the
           existing USER to that group. Membership grants root-level privileges
           and requires a new login session before it is effective.

Runtime floor: Linux kernel 5.10 or newer on x86-64.
Automated install: Ubuntu 22.04/24.04/26.04 and Debian 12/13.
The installer never removes conflicting container packages or node data.
Downloads retry twice. curl connects within 10s and allows 30s per attempt;
APT uses a 30s connection/data timeout. Package installation is not retried.
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
    ubuntu:22.04 | ubuntu:24.04 | ubuntu:26.04 | debian:12 | debian:13)
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

# Account groups can change while an existing login still holds its old groups.
docker_session_pending() {
  [[ -n "${docker_user}" && "$(id -un)" == "${docker_user}" && "$(id -u)" != "0" ]] || return 1
  id -nG "${docker_user}" | tr ' ' '\n' | grep -Fxq docker || return 1
  ! id -nG | tr ' ' '\n' | grep -Fxq docker
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
    if docker_session_pending; then
      echo "FAIL Docker access: docker group membership is not active in this session" >&2
    else
      echo "FAIL Docker daemon: daemon is stopped or the current user cannot access its socket" >&2
    fi
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
    if docker_session_pending; then
      echo "WARN Docker session: before doctor/up, log out and back in, or run 'newgrp docker' and continue in the new shell. Use 'exit' to leave it; other existing sessions remain unchanged." >&2
    fi
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

# Retry acquisition inside APT; never rerun dpkg after an installation failure.
apt_get() {
  run_root apt-get \
    -o Acquire::Retries=2 \
    -o Acquire::http::Timeout=30 \
    -o Acquire::https::Timeout=30 \
    "$@"
}

# Ignore our previous Docker source only for bootstrap. This lets a rerun repair
# an unreachable source without changing the operator's other APT repositories.
install_base_packages() {
  temporary_apt_sources_dir="$(mktemp -d)"
  chmod 0755 "${temporary_apt_sources_dir}"
  local source
  for source in "${apt_sources_dir}"/*.list "${apt_sources_dir}"/*.sources; do
    [[ -f "${source}" ]] || continue
    [[ "${source##*/}" != "docker.sources" ]] || continue
    ln -s "${source}" "${temporary_apt_sources_dir}/${source##*/}"
  done
  local source_options=(
    -o "Dir::Etc::sourceparts=${temporary_apt_sources_dir}"
    -o APT::Get::List-Cleanup=false
  )
  apt_get "${source_options[@]}" update --error-on=any
  apt_get "${source_options[@]}" install -y ca-certificates curl git python3 jq
}

# Complete downloads before invoking dpkg so source fallback remains safe.
download_docker_packages() {
  local mirror="$1"
  local repository="https://download.docker.com/linux/${HOST_OS_ID}"
  if [[ "${mirror}" == "tuna" ]]; then
    repository="https://mirrors.tuna.tsinghua.edu.cn/docker-ce/linux/${HOST_OS_ID}"
  fi
  echo "INFO Docker repository: source=${mirror} url=${repository} suite=${HOST_OS_CODENAME} retries=2"
  echo "INFO Docker signing key: downloading ${repository}/gpg"
  if ! curl -q -fsSL --proto '=https' --proto-redir '=https' \
    --connect-timeout 10 --max-time 30 \
    --retry 2 --retry-all-errors --retry-delay 2 --retry-max-time 90 \
    "${repository}/gpg" -o "${temporary_key_file}"; then
    echo "WARN Docker repository: source=${mirror} stage=signing-key download failed after retries" >&2
    return 1
  fi
  local key_digest
  key_digest="$(sha256sum "${temporary_key_file}")" || fail "cannot hash Docker signing key"
  [[ "${key_digest%% *}" == "${docker_key_sha256}" ]] || \
    fail "Docker signing key checksum mismatch: source=${mirror}; refusing to trust this key"
  run_root install -m 0755 -d /etc/apt/keyrings || fail "cannot create APT keyring directory"
  run_root install -m 0644 "${temporary_key_file}" /etc/apt/keyrings/docker.asc || \
    fail "cannot install Docker signing key"

  local architecture
  architecture="$(dpkg --print-architecture)" || fail "cannot read APT architecture"
  cat >"${temporary_source_file}" <<EOF || fail "cannot write Docker repository configuration"
Types: deb
URIs: ${repository}
Suites: ${HOST_OS_CODENAME}
Components: stable
Architectures: ${architecture}
Signed-By: /etc/apt/keyrings/docker.asc
EOF
  run_root install -m 0644 "${temporary_source_file}" "${apt_sources_dir}/docker.sources" || \
    fail "cannot configure Docker repository"

  # APT normally treats some failed index fetches as warnings and returns zero.
  # Require a fresh, successful update before considering this source usable.
  if ! apt_get update --error-on=any; then
    echo "WARN Docker repository: source=${mirror} stage=apt-update failed after acquisition retries" >&2
    return 1
  fi
  if ! apt_get install -y --download-only "${docker_packages[@]}"; then
    echo "WARN Docker repository: source=${mirror} stage=package-download failed after acquisition retries" >&2
    return 1
  fi
}

# Automatic fallback is limited to the two named, HTTPS Docker CE sources.
install_docker_packages() {
  local mirrors=("${docker_mirror}")
  if [[ "${docker_mirror}" == "auto" ]]; then
    mirrors=(official tuna)
  fi
  temporary_key_file="$(mktemp)"
  temporary_source_file="$(mktemp)"
  local mirror
  for mirror in "${mirrors[@]}"; do
    if [[ "${mirror}" == "tuna" && "${docker_mirror}" == "auto" ]]; then
      echo "WARN Docker repository: official source failed; falling back to Tsinghua Docker CE mirror (tuna)" >&2
    fi
    if download_docker_packages "${mirror}"; then
      echo "INFO Docker packages: installing downloaded packages from source=${mirror}"
      apt_get install -y --no-download "${docker_packages[@]}" || \
        fail "Docker package installation failed after download; resolve the APT/dpkg error before retrying"
      return 0
    fi
  done
  fail "Docker repository acquisition failed: selection=${docker_mirror}; check network/proxy access and the APT errors above"
}

install_host() {
  check_platform
  install_distribution_supported || fail \
    "automatic install supports Ubuntu 22.04/24.04/26.04 and Debian 12/13; use check after manual installation"
  command -v apt-get >/dev/null 2>&1 || fail "apt-get is required"
  command -v dpkg >/dev/null 2>&1 || fail "dpkg is required"
  command -v dpkg-query >/dev/null 2>&1 || fail "dpkg-query is required"
  command -v sha256sum >/dev/null 2>&1 || fail "sha256sum is required"

  local preserve_existing_docker=0
  local docker_bin=""
  if docker_bin="$(command -v docker 2>/dev/null)" && "${docker_bin}" compose version >/dev/null 2>&1; then
    preserve_existing_docker=1
  else
    reject_conflicting_docker_packages
  fi

  install_base_packages

  if [[ "${preserve_existing_docker}" == "1" ]]; then
    echo "Docker Engine and Compose plugin are already installed; preserving the existing installation."
  else
    install_docker_packages
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
    --docker-mirror)
      (($# >= 2)) || fail "--docker-mirror requires a value"
      docker_mirror="$2"
      case "${docker_mirror}" in
        auto | official | tuna) ;;
        *) fail "invalid Docker mirror: ${docker_mirror}; expected auto, official or tuna" ;;
      esac
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
