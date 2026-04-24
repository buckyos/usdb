# World-Sim Release Image Plan

## 1. Goal

The optional Docker `world-sim` overlay should move from development-only
host-mounted binaries to release-style images that can be distributed and run
without extra host binary preparation.

The immediate goal is:

- build `world-sim` images from the already validated local binaries
- switch the Docker overlay to consume those packaged images at runtime
- keep a clear separation between:
  - `usdb-services`
  - Bitcoin Core regtest runtime
  - ord / simulator runtime

## 2. Image Split

The world-sim packaging model uses two dedicated images.

### 2.1 `usdb-bitcoin28-regtest`

Responsibilities:

- provide Bitcoin Core 28.x `bitcoind`
- provide `bitcoin-cli`
- run the regtest `btc-node` service

### 2.2 `usdb-world-sim-tools`

Responsibilities:

- provide `ord`
- provide `bitcoin-cli`
- provide the simulator scripts and Python runtime
- run:
  - `ord-server`
  - `world-sim-runner`

## 3. Binary Source Strategy

The packaging model now deliberately splits Bitcoin Core and `ord`:

- Bitcoin Core 28.x
  - still comes from the already validated local host binaries
- `ord`
  - defaults to a fixed official git tag build inside Docker
  - can still be overridden to use a local compiled binary for development

This keeps the runtime contract stable while improving reproducibility for
`ord`:

- no `curl | bash` latest-install flow in Docker builds
- no silent version drift between two image builds done on different days
- no dependence on the host Rust toolchain for formal image builds
- a clear escape hatch remains for internal debugging against local `ord`
  patches

## 4. Build Workflow

Recommended local command:

```bash
docker/scripts/tools/run_local_world_sim.sh build-images
```

This helper packages host binaries into release-style images:

- `usdb-bitcoin28-regtest:local`
- `usdb-world-sim-tools:local`

The helper currently reads host binaries from:

- `WORLD_SIM_RELEASE_BITCOIN_BIN_HOST_DIR`
  - default: `/home/bucky/btc/bitcoin-28.1/bin`

For `ord`, the helper supports two sources:

- `WORLD_SIM_RELEASE_ORD_SOURCE=git-tag`
  - default
  - builds from a fixed git tag using:
    - `WORLD_SIM_RELEASE_ORD_VERSION`
      - default: `0.23.3`
- `WORLD_SIM_RELEASE_ORD_SOURCE=local`
  - packages a locally built binary from:
    - `WORLD_SIM_RELEASE_ORD_BIN_HOST_PATH`
      - default: `/home/bucky/ord/target/release/ord`

It stages them temporarily under `docker/.build-world-sim/`, builds both
images, validates the packaged binary versions, and then removes the staging
directory.

## 5. Runtime Model

After the images are built, the `world-sim` overlay no longer requires:

- host-mounted Bitcoin Core binaries
- a host-mounted `ord` binary

At runtime, the overlay consumes:

- `WORLD_SIM_BITCOIN_IMAGE`
- `WORLD_SIM_TOOLS_IMAGE`
- `ORD_IMAGE`

The runtime layering is now:

- `compose.base.yml + compose.dev-sim.yml`
  - baseline local regtest stack
- `+ compose.ord.yml`
  - reusable `ord-server` / `full` runtime layer
- `+ compose.world-sim.yml`
  - optional simulation services on top of the same `ord` layer

This makes:

- local testing
- Docker smoke
- later release publishing

all follow the same runtime contract.

## 6. Relationship to Default Dev-Sim

This does not change the meaning of plain `dev-sim`.

- default `dev-sim` remains the standard local baseline stack
- `world-sim` remains an optional overlay
- the packaging change only affects how the optional overlay obtains Bitcoin
  Core 28.x and ord at runtime

## 7. Future Release Path

Once the release-image layout is stable, the next improvements are:

1. move Bitcoin Core packaging from local binaries to CI-produced artifacts
2. tag and publish:
   - `usdb-bitcoin28-regtest`
   - `usdb-world-sim-tools`
3. make `run_local_world_sim.sh` default to published tags instead of `:local`
4. keep host-derived image builds as a local override for internal debugging
