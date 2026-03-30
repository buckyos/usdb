# USDB Docker

This directory contains the first Docker deployment scaffold for USDB.

Current scope:

- `joiner`: start one `bitcoind` + `balance-history` + `usdb-indexer` + `ethw-node`
- `dev-sim`: single-machine simulation using `bitcoind` regtest and the same USDB services
- optional `balance-history` snapshot restore via `snapshot-loader`

Current non-goals:

- no built-in `ord` container yet outside development simulation
- no `usdb-indexer` snapshot support yet
- no extra snapshot management for `bitcoind` or `ethw`

## Layout

- `Dockerfile.usdb-services`
  - builds the `balance-history` and `usdb-indexer` binaries
- `compose.base.yml`
  - shared service definitions
- `compose.joiner.yml`
  - mainnet-style joiner overlay
- `compose.dev-sim.yml`
  - regtest/local simulation overlay
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
- optionally validate an ETHW genesis manifest against the copied genesis file
- optionally copy ETHW / SourceDAO bootstrap config files
- record a `bootstrap-manifest.json` for downstream inspection
- run a dedicated `ethw-init` one-shot `geth init` flow before `ethw-node`
- reuse the existing `snapshot-loader` flow for `balance-history`

Current bootstrap non-goals:

- it does not generate ETHW genesis by itself
- it does not yet run DAO / Dividend initialization transactions
- it does not yet implement a full canonical release flow

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
- `docker/local/bootstrap/manifests/ethw-genesis.manifest.json`
- optional `docker/local/bootstrap/manifests/ethw-bootstrap-config.json`
- optional `docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json`

Brief distinction:

- `ethw-genesis.json`
  - the actual genesis content consumed by `geth init`
- `ethw-genesis.manifest.json`
  - a sidecar description of that genesis artifact
  - currently used to validate `file_sha256`
  - later can also carry `genesis_hash`, `chain_id`, `network_id`, and release metadata

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

`ethw-init` writes its own marker into the shared ETHW data volume.
`ethw-node` requires a matching marker before it will start.

## Next Stage

Planned follow-ups:

- DAO / Dividend initialization hooks after cold start
- signed ETHW genesis manifests and trusted-key validation
- development-only genesis generation flow from `go-ethereum dumpgenesis`
- optional `ord` container/profile
- `usdb-indexer` snapshot restore
- published image tags and release-oriented manifests
