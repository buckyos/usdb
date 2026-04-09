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

For quick browser-only control-console preview, keep using:

- `docker/local/dev-sim/env/dev-sim.env`

and start the reduced service subset with:

```bash
docker/scripts/run_console_preview.sh up
```

If the local `dev-sim.env` file is missing, the helper will scaffold it from
`docker/env/dev-sim.env.example` automatically and will never overwrite an
existing file.

Do not store:

- production signing private keys
- published canonical release manifests
- large persistent chain data directories

Persistent chain data should remain in Docker volumes unless you explicitly need
bind-mounted data for debugging.
