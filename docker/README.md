# USDB Docker

This directory contains the first Docker deployment scaffold for USDB.

Current scope:

- `joiner`: start one `bitcoind` + `balance-history` + `usdb-indexer` + `ethw-node`
- `dev-sim`: single-machine simulation using `bitcoind` regtest and the same USDB services
- optional `balance-history` snapshot restore via `snapshot-loader`
- a built-in `usdb-control-plane` service serving the unified local console

Current non-goals:

- no built-in `ord` container yet outside development simulation
- no `usdb-indexer` snapshot support yet
- no extra snapshot management for `bitcoind` or `ethw`

## Layout

- `Dockerfile.usdb-services`
  - builds the `balance-history`, `usdb-indexer`, and `usdb-control-plane` binaries
- `compose.base.yml`
  - shared service definitions
- `compose.joiner.yml`
  - mainnet-style joiner overlay
- `compose.dev-sim.yml`
  - regtest/local simulation overlay
- `compose.world-sim.yml`
  - optional overlay that adds ord + continuous protocol simulation on top of `dev-sim`
- `compose.bootstrap.yml`
  - cold-start bootstrap overlay
- `env/*.env.example`
  - example environment files
- `local/`
  - gitignored local runtime config, snapshot, and manifest files
- `scripts/`
  - config renderers and startup helpers

## Prerequisites

The current scaffold only builds the USDB service image from this repository.

You still need to provide:

- an `ethw/geth` image for `ethw-node`
- optionally a `bitcoind` image override if you do not want the default

The current bootstrap helpers assume:

- the ETHW image includes `bash`
- the ETHW image includes `sha256sum`
- if `ETHW_BOOTSTRAP_TRUST_MODE=signed`, the ETHW image also includes `openssl`

Recommended environment variable setup:

```bash
mkdir -p docker/local/joiner/env
cp docker/env/joiner.env.example docker/local/joiner/env/joiner.env
```

Path note:

- the example bind-mounted host paths inside `env/*.env.example` are written relative to the `docker/` compose directory
- for example, `SNAPSHOT_HOST_DIR=./local/joiner/snapshots` resolves to `usdb/docker/local/joiner/snapshots`

Then edit:

- `ETHW_IMAGE`
- `ETHW_COMMAND`
- snapshot-related variables if you want `balance-history` snapshot restore

For cold start, use:

```bash
mkdir -p docker/local/bootstrap/env
cp docker/env/bootstrap.env.example docker/local/bootstrap/env/bootstrap.env
```

## Joiner Mode

Recommended command:

```bash
docker compose \
  --env-file docker/local/joiner/env/joiner.env \
  -f docker/compose.base.yml \
  -f docker/compose.joiner.yml \
  up --build
```

Notes:

- `btc-node` is included by default, but `balance-history` and `usdb-indexer` only depend on the configured `BTC_RPC_URL`.
- `BTC_NODE_DATA_DIR` is the path used inside the `btc-node` container.
- `BTC_DATA_DIR` is the path where the same shared volume is mounted inside USDB service containers.
- If you want to use an external BTC RPC, update:
  - `BTC_RPC_URL`
  - `BTC_DATA_DIR`
  - optional BTC auth variables
- then start only the services you need instead of the local `btc-node`

## Dev-Sim Mode

Recommended command:

```bash
docker compose \
  --env-file docker/local/dev-sim/env/dev-sim.env \
  -f docker/compose.base.yml \
  -f docker/compose.dev-sim.yml \
  up --build
```

This runs:

- `bitcoind` in `regtest`
- `balance-history`
- `usdb-indexer`
- `ethw-node`

Current `dev-sim` still keeps `usdb-indexer` on `inscription_source=bitcoind`.
`ord` is a development-only dependency and will only be added to a future
`dev-sim` profile, not to the default `joiner` stack.

## Dev-Sim World-Sim Overlay

If you want a local regtest stack that continuously mines BTC blocks and
generates protocol activity, first build the packaged world-sim images from the
validated local binaries:

```bash
docker/scripts/run_world_sim.sh build-images
```

Then start the optional overlay:

```bash
docker/scripts/run_world_sim.sh up
```

This helper initializes:

- `docker/local/world-sim/env/world-sim.env`

and starts:

- `btc-node`
- `snapshot-loader`
- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`
- `ord-server`
- `world-sim-runner`

By default this mode does **not** start `ethw-node`, so the console will still
show ETHW as unreachable unless you choose:

```bash
docker/scripts/run_world_sim.sh up-full
```

The packaged images are:

- `WORLD_SIM_BITCOIN_IMAGE`
  - default: `usdb-bitcoin28-regtest:local`
- `WORLD_SIM_TOOLS_IMAGE`
  - default: `usdb-world-sim-tools:local`

The build helper packages local binaries from:

- `WORLD_SIM_RELEASE_BITCOIN_BIN_HOST_DIR`
  - default: `/home/bucky/btc/bitcoin-28.1/bin`
- `WORLD_SIM_RELEASE_ORD_BIN_HOST_PATH`
  - default: `/home/bucky/ord/target/release/ord`

Runtime no longer requires host-mounted binaries once the images are built.

Useful helper actions:

```bash
docker/scripts/run_world_sim.sh build-images
docker/scripts/run_world_sim.sh ps
docker/scripts/run_world_sim.sh logs
docker/scripts/run_world_sim.sh down
```

## Console Preview

If you only want to inspect the local control console in a browser, do not use
the random-port smoke stack. Keep the normal `dev-sim` environment file and
start just the minimum subset of services:

```bash
docker/scripts/run_console_preview.sh up
```

If `docker/local/dev-sim/env/dev-sim.env` does not exist yet, the helper will
create it from `docker/env/dev-sim.env.example` once and then continue.

The helper also defaults `DOCKER_API_VERSION` to `1.41` for environments where
the local Docker client is newer than the available daemon API.

This starts:

- `btc-node`
- `snapshot-loader`
- `balance-history`
- `usdb-indexer`
- `usdb-control-plane`

It intentionally does not start `ethw-node`, so the console will show the ETHW
service as unreachable while the rest of the stack is still usable.

The console is then available at:

```text
http://127.0.0.1:28140/
```

Additional helper actions:

```bash
docker/scripts/run_console_preview.sh ps
docker/scripts/run_console_preview.sh logs
docker/scripts/run_console_preview.sh down
```

## Bootstrap Mode

Recommended command:

```bash
docker compose \
  --env-file docker/local/bootstrap/env/bootstrap.env \
  -f docker/compose.base.yml \
  -f docker/compose.bootstrap.yml \
  up --build
```

Current bootstrap scope:

- prepare a shared `/bootstrap` volume
- require or copy a canonical ETHW genesis artifact
- validate an ETHW genesis manifest against the copied genesis file
- optionally validate a detached ETHW genesis manifest signature against trusted keys
- optionally copy ETHW / SourceDAO bootstrap config files
- record a `bootstrap-manifest.json` for downstream inspection
- run a dedicated `ethw-init` one-shot `geth init` flow before `ethw-node`
- reuse the existing `snapshot-loader` flow for `balance-history`
- optionally run a development-only `sourcedao-bootstrap` one-shot after `ethw-node`

Current bootstrap non-goals:

- it does not generate ETHW genesis by itself
- it does not yet implement a full canonical release flow

Current `sourcedao-bootstrap` scope is intentionally narrow:

- disabled by default
- only supports `SOURCE_DAO_BOOTSTRAP_MODE=dev-workspace`
- reuses the local `SourceDAO` workspace
- supports:
  - `SOURCE_DAO_BOOTSTRAP_SCOPE=dao-dividend-only`
  - `SOURCE_DAO_BOOTSTRAP_SCOPE=full`
- `dao-dividend-only` runs `SourceDAO/scripts/usdb_bootstrap_smoke.ts`
- `full` runs `SourceDAO/scripts/usdb_bootstrap_full.ts`
- `full` additionally deploys and wires:
  - `Committee`
  - `DevToken`
  - `NormalToken`
  - `Project`
  - `TokenLockup`
  - `Acquired`

## Container Smoke

Recommended command:

```bash
docker/scripts/run_container_smoke.sh
```

This smoke currently:

- combines `compose.base.yml + compose.dev-sim.yml + compose.bootstrap.yml`
- uses `bitcoind` in `regtest`
- exercises the `bootstrap-init -> ethw-init -> ethw-node -> balance-history -> usdb-indexer` lifecycle
- verifies `usdb-control-plane` can serve `/api/system/overview`
- validates the signed ETHW genesis manifest path
- uses `usdb-services:local` as a temporary fake `ETHW_IMAGE` so the bootstrap lifecycle can be exercised before a real ETHW image is wired in

By default the script cleans up containers, volumes, and temporary input files.
Set `KEEP_RUNNING=1` if you want to inspect the stack after the smoke run.

## Local Runtime Files

Do not treat Docker runtime config as a full committed rootfs.

The recommended split is:

- repository-tracked templates under `docker/`
- gitignored local runtime files under `docker/local/`
- persistent chain data in Docker volumes

Recommended local layout:

```text
docker/local/
  bootstrap/
    env/bootstrap.env
    snapshots/
    keys/
    manifests/
  joiner/
    env/joiner.env
    snapshots/
    keys/
    manifests/
  dev-sim/
    env/dev-sim.env
    snapshots/
    keys/
    manifests/
  world-sim/
    env/world-sim.env
    runtime/
```

Use this directory for:

- real `.env` files
- snapshot DB / manifest / signature files
- trusted snapshot key sets
- local bootnodes or service manifests
- local bootstrap genesis artifact files and bootstrap config files

Do not use it for:

- production signing private keys
- published release artifacts
- long-lived chain state databases

For bootstrap flows, the recommended local files are:

- `docker/local/bootstrap/manifests/ethw-genesis.json`

For the optional world-sim overlay, see:

- [dev-sim-world-sim-plan.md](/home/bucky/work/usdb/doc/dev-sim-world-sim-plan.md)
- `docker/local/bootstrap/manifests/ethw-genesis.manifest.json`
- optional `docker/local/bootstrap/manifests/ethw-genesis.manifest.sig`
- optional `docker/local/bootstrap/manifests/ethw-bootstrap-config.json`
- optional `docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json`
- optional `docker/local/bootstrap/keys/trusted_ethw_genesis_keys.json`

For the current development-only `sourcedao-bootstrap` flow, prepare local
inputs like this:

```bash
cd /home/bucky/work/SourceDAO
npm ci
npm run build:usdb

cp tools/config/usdb-local.json \
  /home/bucky/work/usdb/docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json
```

Prefer the explicit full-bootstrap template for long-lived setups:

```bash
cp /home/bucky/work/SourceDAO/tools/config/usdb-bootstrap-full.example.json \
  /home/bucky/work/usdb/docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json
```

Then set in `docker/local/bootstrap/env/bootstrap.env`:

```env
SOURCE_DAO_BOOTSTRAP_MODE=dev-workspace
SOURCE_DAO_REPO_HOST_DIR=../../SourceDAO
SOURCE_DAO_BOOTSTRAP_SCOPE=full
SOURCE_DAO_BOOTSTRAP_PREPARE=validate
```

The bootstrap job rewrites a runtime copy of the SourceDAO config inside
`/bootstrap` so the copied config can safely point at container-local
`artifacts-usdb`.

Brief distinction:

- `ethw-genesis.json`
  - the actual genesis content consumed by `geth init`
- `ethw-genesis.manifest.json`
  - a sidecar description of that genesis artifact
  - used to validate `file_sha256`
  - later can also carry `genesis_hash`, `chain_id`, `network_id`, and release metadata
- `ethw-genesis.manifest.sig`
  - detached Ed25519 signature over the exact manifest file bytes
- `trusted_ethw_genesis_keys.json`
  - trusted public-key set used when `ETHW_BOOTSTRAP_TRUST_MODE=signed`

## Snapshot Restore

The optional `snapshot-loader` container only handles `balance-history`.

Supported modes:

- `SNAPSHOT_MODE=none`
- `SNAPSHOT_MODE=balance-history`

When enabled, `snapshot-loader`:

1. renders `balance-history/config.toml`
2. runs `balance-history install-snapshot`
3. writes a success marker under the shared `balance-history` root
4. exits successfully before `balance-history` starts

`balance-history` then performs its own local gate:

- if `SNAPSHOT_MODE=balance-history`, it requires the snapshot-loader marker
- if the marker is missing or does not match the configured snapshot inputs, startup fails fast
- if `SNAPSHOT_MODE=none`, no marker is required and the service starts from zero-sync state

This split is intentional:

- Compose controls startup ordering
- the marker gate controls startup validity inside the shared volume

Important:

- Signed snapshot installs require:
  - `BH_SNAPSHOT_TRUST_MODE=signed`
  - `BH_SNAPSHOT_TRUSTED_KEYS_FILE`
- The snapshot file itself is not baked into the image.
- Mount snapshots from the host or another volume.
- The recommended local host path is under `docker/local/<mode>/snapshots/`.

## BTC Auth

Generated configs support:

- cookie auth (default)
- user/pass auth
- no auth

The recommended default is cookie auth with the `bitcoind` data directory mounted
read-only into the USDB service containers.

When `BTC_NETWORK=regtest`, the cookie file normally lives under the network
subdirectory, for example:

- `BTC_COOKIE_FILE=/data/bitcoind/regtest/.cookie`

## ETHW Bootstrap Artifact

The current bootstrap flow is artifact-first.

Recommended production-style flow:

1. generate a canonical ETHW genesis JSON outside the Docker stack
2. publish it together with a sidecar manifest
3. let `bootstrap-init` validate and stage the artifact under `/bootstrap`
4. let `ethw-init` initialize the local `ethw-data` volume from that staged artifact

Current trust modes:

- `ETHW_BOOTSTRAP_TRUST_MODE=none`
  - genesis manifest is optional
- `ETHW_BOOTSTRAP_TRUST_MODE=manifest`
  - requires `ETHW_BOOTSTRAP_GENESIS_MANIFEST_INPUT_FILE`
  - validates `file_sha256` against the copied genesis file
- `ETHW_BOOTSTRAP_TRUST_MODE=signed`
  - requires `ETHW_BOOTSTRAP_GENESIS_MANIFEST_INPUT_FILE`
  - requires `ETHW_BOOTSTRAP_GENESIS_SIG_INPUT_FILE`
  - requires `ETHW_BOOTSTRAP_TRUSTED_KEYS_INPUT_FILE`
  - validates `file_sha256`
  - requires `manifest.signature_scheme=ed25519`
  - requires `manifest.signing_key_id`
  - verifies the detached signature against the trusted key set

`ethw-init` writes its own marker into the shared ETHW data volume.
`ethw-node` requires a matching marker before it will start.

The trusted key file format intentionally matches the `balance-history` snapshot
trusted-key JSON shape:

```json
{
  "keys": [
    {
      "key_id": "ethw-genesis-signer-1",
      "public_key_base64": "<base64 of raw 32-byte ed25519 public key>"
    }
  ]
}
```

## Next Stage

Planned follow-ups:

- `sourcedao-bootstrap` one-shot job after cold start
- standardized ETHW node startup templates and joiner peer config
- development-only genesis generation flow from `go-ethereum dumpgenesis`
- optional `ord` container/profile
- `usdb-indexer` snapshot restore
- published image tags and release-oriented manifests
