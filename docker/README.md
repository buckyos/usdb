# USDB Docker

This directory contains the first Docker deployment scaffold for USDB.

Current scope:

- `joiner`: start one `bitcoind` + `balance-history` + `usdb-indexer` + `ethw-node`
- `dev-sim`: single-machine simulation using `bitcoind` regtest and the same USDB services
- optional `balance-history` snapshot restore via `snapshot-loader`

Current non-goals:

- no built-in `bootstrap-init` flow yet
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

Then edit:

- `ETHW_IMAGE`
- `ETHW_COMMAND`
- snapshot-related variables if you want `balance-history` snapshot restore

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

## Local Runtime Files

Do not treat Docker runtime config as a full committed rootfs.

The recommended split is:

- repository-tracked templates under `docker/`
- gitignored local runtime files under `docker/local/`
- persistent chain data in Docker volumes

Recommended local layout:

```text
docker/local/
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

Do not use it for:

- production signing private keys
- published release artifacts
- long-lived chain state databases

## Snapshot Restore

The optional `snapshot-loader` container only handles `balance-history`.

Supported modes:

- `SNAPSHOT_MODE=none`
- `SNAPSHOT_MODE=balance-history`

When enabled, `snapshot-loader`:

1. renders `balance-history/config.toml`
2. runs `balance-history install-snapshot`
3. exits successfully before `balance-history` starts

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

## Next Stage

Planned follow-ups:

- `bootstrap-init` and canonical bootstrap-genesis flow
- optional `ord` container/profile
- `usdb-indexer` snapshot restore
- published image tags and release-oriented manifests
