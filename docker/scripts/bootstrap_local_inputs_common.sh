#!/usr/bin/env bash
set -euo pipefail

# Expected caller variables:
# - docker_dir
# - env_file
# - example_env_file

env_get() {
  local key="${1:?key is required}"
  local fallback="${2:-}"
  local value
  if [[ "${!key+x}" == "x" ]]; then
    printf '%s\n' "${!key}"
    return
  fi
  if [[ -f "${env_file}" ]]; then
    value="$(awk -F= -v key="${key}" '$1 == key { sub(/^[^=]+=*/, "", $0); print $0 }' "${env_file}" | tail -n 1)"
  else
    value=""
  fi
  if [[ -n "${value}" ]]; then
    printf '%s\n' "${value}"
  else
    printf '%s\n' "${fallback}"
  fi
}

host_path_from_docker_dir() {
  local path="${1:?path is required}"
  if [[ "${path}" = /* ]]; then
    printf '%s\n' "${path}"
  else
    printf '%s\n' "${docker_dir}/${path#./}"
  fi
}

init_env_file() {
  local env_dir
  env_dir="$(dirname "${env_file}")"
  if [[ ! -f "${env_file}" ]]; then
    mkdir -p "${env_dir}"
    cp "${example_env_file}" "${env_file}"
    echo "Initialized ${env_file} from ${example_env_file}"
  fi
}

bootstrap_manifests_dir() {
  host_path_from_docker_dir "$(env_get BOOTSTRAP_HOST_DIR ./local/bootstrap/manifests)"
}

source_dao_repo_dir() {
  host_path_from_docker_dir "$(env_get SOURCE_DAO_REPO_HOST_DIR ../../SourceDAO)"
}

go_ethereum_repo_dir() {
  host_path_from_docker_dir "${GO_ETHEREUM_REPO_HOST_DIR:-../../go-ethereum}"
}

ensure_source_dao_config() {
  local manifests_dir
  local source_dao_repo
  local config_file
  local source_template

  manifests_dir="$(bootstrap_manifests_dir)"
  source_dao_repo="$(source_dao_repo_dir)"
  config_file="${manifests_dir}/sourcedao-bootstrap-config.json"
  source_template="${source_dao_repo}/tools/config/usdb-bootstrap-full.example.json"

  mkdir -p "${manifests_dir}"

  [[ -f "${source_template}" ]] || {
    echo "Missing SourceDAO bootstrap template: ${source_template}" >&2
    exit 1
  }

  if [[ ! -f "${config_file}" ]]; then
    cp "${source_template}" "${config_file}"
    echo "Initialized ${config_file} from ${source_template}"
  fi
}

ensure_ethw_image_exists() {
  local image
  image="$(env_get ETHW_IMAGE usdb-ethw:local)"
  docker image inspect "${image}" >/dev/null 2>&1 || {
    cat <<EOF >&2
Missing ETHW image ${image}

Build it first before preparing ETHW bootstrap inputs.
EOF
    exit 1
  }
}

ensure_ethw_genesis() {
  local manifests_dir
  local genesis_file
  local config_file
  local ethw_image

  manifests_dir="$(bootstrap_manifests_dir)"
  genesis_file="${manifests_dir}/ethw-genesis.json"
  config_file="${manifests_dir}/sourcedao-bootstrap-config.json"
  ethw_image="$(env_get ETHW_IMAGE usdb-ethw:local)"

  if [[ -f "${genesis_file}" ]]; then
    return
  fi

  ensure_ethw_image_exists
  [[ -f "${config_file}" ]] || {
    echo "Missing SourceDAO bootstrap config: ${config_file}" >&2
    exit 1
  }

  echo "Generating ${genesis_file} from ${config_file}"
  docker run --rm \
    -v "${manifests_dir}:/workspace/bootstrap:ro" \
    "${ethw_image}" \
    dumpgenesis \
    --usdb \
    --usdb.bootstrap.config /workspace/bootstrap/sourcedao-bootstrap-config.json \
    > "${genesis_file}"
}

prepare_local_inputs() {
  init_env_file
  ensure_source_dao_config
  ensure_ethw_genesis
}

build_ethw_image() {
  local ethw_image
  local go_ethereum_repo

  ethw_image="$(env_get ETHW_IMAGE usdb-ethw:local)"
  go_ethereum_repo="$(go_ethereum_repo_dir)"

  [[ -d "${go_ethereum_repo}" ]] || {
    echo "Missing go-ethereum repo: ${go_ethereum_repo}" >&2
    exit 1
  }

  echo "Building ${ethw_image} from ${go_ethereum_repo}"
  docker build -t "${ethw_image}" "${go_ethereum_repo}"
}
