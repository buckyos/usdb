# Docker Script Tools

This directory contains the repo-facing helpers that developers are expected to
run directly.

These tool names now follow the canonical local/profile naming scheme defined
in
[TOOL_NAMING_PLAN.md](/home/bucky/work/usdb/docker/scripts/TOOL_NAMING_PLAN.md).

## Tool Families

The tools in this directory are easiest to understand when split into two
families:

- `runtime profile tools`
  - bring up one concrete local stack profile
  - the main comparison dimensions are service set, BTC runtime, ETHW runtime,
    state model, and bootstrap behavior
- `utility tools`
  - build images, run smoke checks, or validate a stack
  - these do not define a long-running runtime profile by themselves

## Runtime Profile Matrix

This table is the fastest way to decide which tool to start from.

| Tool | Primary use | Default env file | Compose overlays | Startup style | State model |
| --- | --- | --- | --- | --- | --- |
| [run_local_console.sh](/home/bucky/work/usdb/docker/scripts/tools/run_local_console.sh) | Fastest console/control-plane preview | `docker/local/dev-sim/env/dev-sim.env` | `base + dev-sim` | foreground | keep volumes until `down -v` or manual cleanup |
| [run_local_runtime.sh](/home/bucky/work/usdb/docker/scripts/tools/run_local_runtime.sh) | Full local BTC + ord + ETHW runtime without simulation | `docker/local/dev-full/env/dev-full.env` | `base + dev-sim + ord` | foreground | keep volumes until `down -v` or manual cleanup |
| [run_local_world_sim.sh](/home/bucky/work/usdb/docker/scripts/tools/run_local_world_sim.sh) | BTC-side simulation with deterministic identities, without ETHW node | `docker/local/world-sim/env/world-sim.env` | `base + dev-sim + ord + world-sim` | detached by default, then readiness checks | controlled by `WORLD_SIM_STATE_MODE` |
| [run_local_world_sim_ethw.sh](/home/bucky/work/usdb/docker/scripts/tools/run_local_world_sim_ethw.sh) | Same as `run_local_world_sim.sh`, plus local ETHW alignment | `docker/local/world-sim/env/world-sim.env` | `base + dev-sim + ord + world-sim` | detached by default, then readiness checks | controlled by `WORLD_SIM_STATE_MODE` |
| [run_local_bootstrap.sh](/home/bucky/work/usdb/docker/scripts/tools/run_local_bootstrap.sh) | ETHW cold-start + SourceDAO bootstrap validation | `docker/local/bootstrap/env/bootstrap.env` | `base + dev-sim + bootstrap` | detached by default | bootstrap artifacts are preserved locally; Docker volumes persist until `reset` |
| [run_local_full_sim.sh](/home/bucky/work/usdb/docker/scripts/tools/run_local_full_sim.sh) | Complete local development simulation stack | `docker/local/dev-full-sim/env/dev-full-sim.env` | `base + dev-sim + ord + bootstrap + world-sim` | foreground | controlled by `WORLD_SIM_STATE_MODE`; `reset` also clears volumes |

## Service Coverage Matrix

Use this matrix when the main question is “which stack actually contains the
service I need”.

| Tool | `btc-node` | `snapshot-loader` | `balance-history` | `usdb-indexer` | `usdb-control-plane` | `ord-server` | `ethw-node` | `world-sim-bootstrap` + `world-sim-runner` | `sourcedao-bootstrap` |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `run_local_console.sh` | yes | yes | yes | yes | yes | no | no | no | no |
| `run_local_runtime.sh` | yes | yes | yes | yes | yes | yes | yes | no | no |
| `run_local_world_sim.sh` | yes | yes | yes | yes | yes | yes | no | yes | no |
| `run_local_world_sim_ethw.sh` | yes | yes | yes | yes | yes | yes | yes | yes | no |
| `run_local_bootstrap.sh` | yes | yes | yes | yes | yes | no | yes | no | yes |
| `run_local_full_sim.sh` | yes | yes | yes | yes | yes | yes | yes | yes | yes |

## BTC / ETHW Runtime Matrix

This table compares the runtime semantics instead of only listing services.

| Tool | BTC network today | BTC auth today | ord role today | ETHW role today | SourceDAO role today | Identity model |
| --- | --- | --- | --- | --- | --- | --- |
| `run_local_console.sh` | `regtest` | `cookie` | none | not started | none | manual or browser-driven console testing only |
| `run_local_runtime.sh` | `regtest` | `cookie` | long-running `ord-server` | local dev `geth` node, no bootstrap flow | none | no deterministic world-sim identity layer |
| `run_local_world_sim.sh` | `regtest` | `userpass` | long-running `ord-server` plus world-sim wallet actions | not started | none | deterministic BTC ord wallets from `WORLD_SIM_IDENTITY_SEED` |
| `run_local_world_sim_ethw.sh` | `regtest` | `userpass` | long-running `ord-server` plus world-sim wallet actions | local dev `geth` node, protocol-aligned with world-sim | none | deterministic BTC ord wallets; optional ETHW deterministic alignment |
| `run_local_bootstrap.sh` | `regtest` | `cookie` | none | bootstrap-oriented local ETHW chain from generated genesis | dev-workspace bootstrap by default | bootstrap artifacts and ETHW init inputs, not world-sim identities |
| `run_local_full_sim.sh` | `regtest` | `userpass` | long-running `ord-server` plus world-sim wallet actions | bootstrap-oriented local ETHW chain plus deterministic miner identity | full dev-workspace bootstrap | deterministic BTC ord wallets and ETHW identity alignment |

Current status:

- BTC runtime today is split mainly between `bitcoin mainnet` style joiner /
  bootstrap envs and `regtest` development envs.
- ETHW runtime today is still mostly local-dev or local-bootstrap oriented.
- future tools should document whether they target `mainnet`, official testnet,
  or local development chain explicitly instead of relying on implied naming.

## Key Environment Variable Matrix

These are the runtime-defining variables that most often explain why two tools
behave differently.

### Core BTC / ord / control-plane variables

| Variable or group | `run_local_console.sh` | `run_local_runtime.sh` | `run_local_world_sim.sh` | `run_local_bootstrap.sh` | `run_local_full_sim.sh` |
| --- | --- | --- | --- | --- | --- |
| env file | `local/dev-sim/env/dev-sim.env` | `local/dev-full/env/dev-full.env` | `local/world-sim/env/world-sim.env` | `local/bootstrap/env/bootstrap.env` | `local/dev-full-sim/env/dev-full-sim.env` |
| `BTC_NETWORK` | `regtest` | `regtest` | `regtest` | `regtest` in local helper defaults | `regtest` |
| `BTC_AUTH_MODE` | `cookie` | `cookie` | `userpass` | `cookie` by default | `userpass` |
| `BTC_RPC_USER` / `BTC_RPC_PASSWORD` | not used by default | usually empty | `usdb` / `usdb-dev-sim` | not used by default | `usdb` / `usdb-dev-sim` |
| `ORD_SERVER_BIND_PORT` | n/a | `28130` | `28130` | n/a | `28130` |
| `CONTROL_PLANE_BIND_PORT` | `28140` | `28140` | `28140` | `28140` by local helper default | `28140` |
| `SNAPSHOT_MODE` | `none` | `none` | `none` | `none` by local helper default | `none` |

### ETHW / world-sim / bootstrap variables

| Variable or group | `run_local_console.sh` | `run_local_runtime.sh` | `run_local_world_sim.sh` | `run_local_bootstrap.sh` | `run_local_full_sim.sh` |
| --- | --- | --- | --- | --- | --- |
| `ETHW_COMMAND` | present in env, but service not started | local dev `geth` launch command | only used by `run_local_world_sim_ethw.sh` | bootstrap-target ETHW command | bootstrap-target ETHW command |
| `ETHW_IDENTITY_MODE` | n/a | n/a | `deterministic-seed` | n/a | `deterministic-seed` |
| `ETHW_SIM_PROTOCOL_ALIGNMENT` | n/a | n/a | auto-set by helper: `run_local_world_sim.sh=0`, `run_local_world_sim_ethw.sh=1` | n/a | forced to `1` |
| `WORLD_SIM_STATE_MODE` | n/a | n/a | `persistent` / `reset` / `seeded-reset` | n/a | `persistent` / `reset` / `seeded-reset` |
| `WORLD_SIM_IDENTITY_SEED` | n/a | n/a | optional but required for deterministic seeded reset | n/a | expected for deterministic world-sim and ETHW alignment |
| `SOURCE_DAO_BOOTSTRAP_MODE` / `SCOPE` / `PREPARE` | n/a | n/a | n/a | defaults to `dev-workspace / full / auto` | defaults to `dev-workspace / full / auto` |
| bootstrap manifest dirs | n/a | n/a | n/a | `local/bootstrap/manifests` | `local/dev-full-sim/bootstrap/manifests` |

## Utility Tool Matrix

These tools do not define a new runtime profile; they support the profiles
above.

| Tool | Category | What it changes | Typical use | Notes |
| --- | --- | --- | --- | --- |
| [build_world_sim_images.sh](/home/bucky/work/usdb/docker/scripts/tools/build_world_sim_images.sh) | image packaging | builds `WORLD_SIM_BITCOIN_IMAGE` and `WORLD_SIM_TOOLS_IMAGE` | first-time world-sim image setup or when host `bitcoind` / `ord` changes | supports `WORLD_SIM_RELEASE_ORD_SOURCE=local` and `git-tag` |
| [smoke_bootstrap_stack.sh](/home/bucky/work/usdb/docker/scripts/tools/smoke_bootstrap_stack.sh) | smoke validation | creates a temporary bootstrap project and local manifests | validating cold-start wiring, manifests, and bootstrap one-shots | can keep the stack running with `KEEP_RUNNING=1` |

## Recommended Documentation Contract For Future Tools

When a new tool is added, keep the comparison tables above maintainable by
documenting the new entry with the same fields.

| Dimension | Why it matters | Example values |
| --- | --- | --- |
| `tool class` | separates runtime profiles from utilities | `runtime-profile`, `utility`, `builder`, `smoke-check` |
| `default env file` | shows where behavior is really configured | `local/dev-full/env/dev-full.env` |
| `compose overlays` | explains which services can appear | `base + dev-sim + ord` |
| `BTC runtime class` | distinguishes `mainnet`, `regtest`, joiner, or mixed profiles | `bitcoin mainnet`, `regtest`, `joiner external-RPC` |
| `ord mode` | clarifies whether ord is absent, server-only, or server plus wallet actions | `none`, `server`, `server + wallet actions` |
| `ETHW runtime class` | makes future `mainnet / official testnet / local dev` differences explicit | `not started`, `local dev chain`, `bootstrap local chain`, `official testnet` |
| `identity model` | avoids ambiguity around browser wallets, deterministic seeds, or bootstrap inputs | `browser/manual`, `deterministic-seed`, `bootstrap-artifact-driven` |
| `state model` | makes reset/persistent behavior obvious | `persistent`, `reset`, `seeded-reset`, `ephemeral temp project` |
| `bootstrap role` | clarifies whether SourceDAO or genesis init is involved | `none`, `SourceDAO full bootstrap`, `ETHW genesis only` |
| `critical env variables` | gives readers the shortest path to the real knobs | `BTC_NETWORK`, `BTC_AUTH_MODE`, `WORLD_SIM_STATE_MODE` |

## Tool Paths

These repo-facing helpers now live directly under `docker/scripts/tools/`.
