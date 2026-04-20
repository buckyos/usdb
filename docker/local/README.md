# Local Docker Runtime Files

This directory is intentionally excluded from git and is used for local runtime
configuration only.

Recommended layout:

```text
docker/local/
  bootstrap/
    env/
      bootstrap.env
    snapshots/
    keys/
    manifests/
  joiner/
    env/
      joiner.env
    snapshots/
    keys/
    manifests/
  dev-sim/
    env/
      dev-sim.env
    snapshots/
    keys/
    manifests/
  world-sim/
    env/
      world-sim.env
    runtime/
```

Suggested usage:

- `env/`
  - real `.env` files copied from `docker/env/*.env.example`
- `snapshots/`
  - `balance-history` snapshot DB, manifest, and signature files
- `keys/`
  - trusted snapshot public-key sets
  - trusted ETHW genesis manifest public-key sets
- `manifests/`
  - bootnodes manifests, ETHW genesis artifact manifests, local service manifests, or other local metadata
  - development-only `sourcedao-bootstrap-config.json`

For quick browser-only control-console preview, keep using:

- `docker/local/dev-sim/env/dev-sim.env`

and start the reduced service subset with:

```bash
docker/scripts/run_console_preview.sh up
```

If the local `dev-sim.env` file is missing, the helper will scaffold it from
`docker/env/dev-sim.env.example` automatically and will never overwrite an
existing file.

For the optional regtest world-sim overlay, use:

- `docker/local/world-sim/env/world-sim.env`

and start it with:

```bash
docker/scripts/run_world_sim.sh build-images
docker/scripts/run_world_sim.sh up
```

For the bootstrap overlay, use:

- `docker/local/bootstrap/env/bootstrap.env`
- `docker/local/bootstrap/manifests/sourcedao-bootstrap-config.json`

The easiest way to run the full local ETHW + SourceDAO bootstrap stack is:

```bash
docker/scripts/run_sourcedao_bootstrap.sh up
```

The current stage-one `sourcedao-bootstrap` flow also expects a local
`SourceDAO` workspace outside this directory. By default the bootstrap env uses:

- `SOURCE_DAO_REPO_HOST_DIR=../../SourceDAO`

Do not store:

- production signing private keys
- published canonical release manifests
- large persistent chain data directories

Persistent chain data should remain in Docker volumes unless you explicitly need
bind-mounted data for debugging.
