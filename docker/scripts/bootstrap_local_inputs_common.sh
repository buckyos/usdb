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

source_dao_artifacts_dir() {
  local artifacts_dir
  local source_dao_repo

  artifacts_dir="$(env_get SOURCE_DAO_ARTIFACTS_DIR /workspace/SourceDAO/artifacts-usdb)"
  source_dao_repo="$(source_dao_repo_dir)"

  case "${artifacts_dir}" in
    /workspace/SourceDAO/*)
      printf '%s\n' "${source_dao_repo}/${artifacts_dir#/workspace/SourceDAO/}"
      ;;
    ./*|../*|*)
      if [[ "${artifacts_dir}" = /* ]]; then
        printf '%s\n' "${artifacts_dir}"
      else
        printf '%s\n' "${source_dao_repo}/${artifacts_dir#./}"
      fi
      ;;
  esac
}

genesis_runtime_artifacts_dir() {
  printf '%s\n' "/workspace/source-dao-artifacts"
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

  SOURCE_DAO_SOURCE_TEMPLATE="${source_template}" \
  SOURCE_DAO_TARGET_CONFIG="${config_file}" \
  node <<'NODE'
const fs = require("node:fs");

const sourceTemplate = process.env.SOURCE_DAO_SOURCE_TEMPLATE;
const targetConfig = process.env.SOURCE_DAO_TARGET_CONFIG;
const source = JSON.parse(fs.readFileSync(sourceTemplate, "utf8"));
const target = JSON.parse(fs.readFileSync(targetConfig, "utf8"));

let updated = false;
for (const field of ["genesisDifficulty", "minimumDifficulty"]) {
  if ((target[field] === undefined || target[field] === "") && source[field] !== undefined && source[field] !== "") {
    target[field] = source[field];
    updated = true;
  }
}

if (updated) {
  fs.writeFileSync(targetConfig, `${JSON.stringify(target, null, 2)}\n`);
  console.log(`Backfilled difficulty defaults in ${targetConfig}`);
}
NODE
}

ensure_source_dao_artifacts() {
  local source_dao_repo
  local artifacts_dir

  source_dao_repo="$(source_dao_repo_dir)"
  artifacts_dir="$(source_dao_artifacts_dir)"

  [[ -d "${source_dao_repo}" ]] || {
    echo "Missing SourceDAO repo: ${source_dao_repo}" >&2
    exit 1
  }

  if [[ -d "${artifacts_dir}" ]]; then
    return
  fi

  if [[ ! -d "${source_dao_repo}/node_modules" ]]; then
    echo "Installing SourceDAO node_modules with npm ci"
    (cd "${source_dao_repo}" && npm ci)
  fi

  echo "Building SourceDAO USDB artifacts"
  (
    cd "${source_dao_repo}" && \
    SOURCE_DAO_ARTIFACTS_DIR="${artifacts_dir}" \
    SOURCE_DAO_CACHE_DIR="${source_dao_repo}/cache-usdb" \
    npm run build:usdb
  )
}

write_genesis_runtime_config() {
  local source_config="${1:?source config is required}"
  local runtime_config="${2:?runtime config is required}"
  local runtime_artifacts_dir

  runtime_artifacts_dir="$(genesis_runtime_artifacts_dir)"

  SOURCE_DAO_SOURCE_CONFIG="${source_config}" \
  SOURCE_DAO_RUNTIME_CONFIG="${runtime_config}" \
  SOURCE_DAO_RUNTIME_ARTIFACTS="${runtime_artifacts_dir}" \
  node <<'NODE'
const fs = require("node:fs");
const sourceConfig = process.env.SOURCE_DAO_SOURCE_CONFIG;
const runtimeConfig = process.env.SOURCE_DAO_RUNTIME_CONFIG;
const artifactsDir = process.env.SOURCE_DAO_RUNTIME_ARTIFACTS;

const data = JSON.parse(fs.readFileSync(sourceConfig, "utf8"));
data.artifactsDir = artifactsDir;
fs.writeFileSync(runtimeConfig, `${JSON.stringify(data, null, 2)}\n`);
NODE
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
  local runtime_config_file
  local artifacts_dir
  local ethw_image
  local tmp_genesis_file

  manifests_dir="$(bootstrap_manifests_dir)"
  genesis_file="${manifests_dir}/ethw-genesis.json"
  config_file="${manifests_dir}/sourcedao-bootstrap-config.json"
  runtime_config_file="${manifests_dir}/sourcedao-bootstrap.runtime.json"
  artifacts_dir="$(source_dao_artifacts_dir)"
  ethw_image="$(env_get ETHW_IMAGE usdb-ethw:local)"

  ensure_ethw_image_exists
  ensure_source_dao_artifacts
  [[ -f "${config_file}" ]] || {
    echo "Missing SourceDAO bootstrap config: ${config_file}" >&2
    exit 1
  }

  write_genesis_runtime_config "${config_file}" "${runtime_config_file}"

  if [[ -f "${genesis_file}" ]]; then
    if python3 -m json.tool "${genesis_file}" >/dev/null 2>&1; then
      if SOURCE_DAO_BOOTSTRAP_CONFIG="${config_file}" ETHW_GENESIS_FILE="${genesis_file}" node <<'NODE'
const fs = require("node:fs");

const configPath = process.env.SOURCE_DAO_BOOTSTRAP_CONFIG;
const genesisPath = process.env.ETHW_GENESIS_FILE;
const config = JSON.parse(fs.readFileSync(configPath, "utf8"));
const genesis = JSON.parse(fs.readFileSync(genesisPath, "utf8"));

const parseBigInt = (value) => {
  if (value === undefined || value === null || value === "") {
    return null;
  }
  if (typeof value === "number") {
    return BigInt(value);
  }
  if (typeof value === "string") {
    return BigInt(value);
  }
  throw new Error(`unsupported difficulty value: ${value}`);
};

const expectedGenesisDifficulty = parseBigInt(config.genesisDifficulty);
const expectedMinimumDifficulty = parseBigInt(config.minimumDifficulty);
const actualGenesisDifficulty = parseBigInt(genesis.difficulty);
const actualMinimumDifficulty = parseBigInt(genesis.config?.ethPoWMinimumDifficulty);

if (expectedGenesisDifficulty !== null && actualGenesisDifficulty !== expectedGenesisDifficulty) {
  process.exit(1);
}
if (expectedMinimumDifficulty !== null && actualMinimumDifficulty !== expectedMinimumDifficulty) {
  process.exit(1);
}
NODE
      then
        return
      fi

      echo "Existing ${genesis_file} does not match current difficulty config; regenerating"
      rm -f "${genesis_file}"
    else
      echo "Existing ${genesis_file} is invalid JSON; regenerating"
      rm -f "${genesis_file}"
    fi
  fi

  tmp_genesis_file="$(mktemp "${manifests_dir}/ethw-genesis.json.tmp.XXXXXX")"
  echo "Generating ${genesis_file} from ${config_file}"
  if ! docker run --rm \
    -v "${manifests_dir}:/workspace/bootstrap:ro" \
    -v "${artifacts_dir}:$(genesis_runtime_artifacts_dir):ro" \
    "${ethw_image}" \
    dumpgenesis \
    --usdb \
    --usdb.bootstrap.config /workspace/bootstrap/sourcedao-bootstrap.runtime.json \
    > "${tmp_genesis_file}"; then
    rm -f "${tmp_genesis_file}"
    echo "Failed to generate ETHW genesis from ${config_file}" >&2
    exit 1
  fi

  if ! python3 -m json.tool "${tmp_genesis_file}" >/dev/null 2>&1; then
    echo "Generated ETHW genesis is not valid JSON: ${tmp_genesis_file}" >&2
    rm -f "${tmp_genesis_file}"
    exit 1
  fi

  mv "${tmp_genesis_file}" "${genesis_file}"
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
