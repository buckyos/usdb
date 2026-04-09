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

The first release-packaging step still uses the already validated local binary
artifacts as the source of truth:

- Bitcoin Core 28.x binaries from the local host
- the locally built `ord` binary from the local host

This keeps the packaging change small:

- no new upstream build pipeline is required yet
- the runtime stops depending on host-mounted binaries
- later CI/release automation can replace the local binary source with a formal
  artifact publishing flow

## 4. Build Workflow

Recommended local command:

```bash
docker/scripts/run_world_sim.sh build-images
```

This helper packages host binaries into release-style images:

- `usdb-bitcoin28-regtest:local`
- `usdb-world-sim-tools:local`

The helper currently reads host binaries from:

- `WORLD_SIM_RELEASE_BITCOIN_BIN_HOST_DIR`
  - default: `/home/bucky/btc/bitcoin-28.1/bin`
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

1. move from local-binary packaging to CI-produced artifacts
2. tag and publish:
   - `usdb-bitcoin28-regtest`
   - `usdb-world-sim-tools`
3. make `run_world_sim.sh` default to published tags instead of `:local`
4. keep host-derived image builds as a local override for internal debugging
