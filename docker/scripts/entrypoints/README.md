# Docker Entrypoints

This directory contains the scripts used as container entrypoints by Compose
services and Docker images.

## Scripts

- [bootstrap_init.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/bootstrap_init.sh)
  - copies and validates cold-start bootstrap inputs into `/bootstrap`
  - used by the bootstrap-init service before ETHW and SourceDAO startup
- [ethw_init.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/ethw_init.sh)
  - validates the ETHW genesis artifact and runs `geth init` when needed
  - note: guarded by the ETHW init marker to keep repeated starts idempotent
- [snapshot_loader.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/snapshot_loader.sh)
  - installs a balance-history snapshot before the service starts
  - note: fails fast when an existing DB does not match the requested snapshot inputs
- [start_balance_history.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_balance_history.sh)
  - renders config, waits for BTC RPC, validates snapshot state, then starts
    `balance-history`
- [start_control_plane.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_control_plane.sh)
  - renders control-plane config and starts `usdb-control-plane`
- [start_ethw_full_sim.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_ethw_full_sim.sh)
  - derives or restores the ETHW miner identity used by `run_local_full_sim.sh`
  - note: the final `geth` command is `exec`'d so Docker stop signals reach it directly
- [start_ethw_node.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_ethw_node.sh)
  - validates ETHW init state and launches the runtime ETHW node
  - note: also `exec`'s the final `geth` process for cleaner shutdown
- [start_ord_server.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_ord_server.sh)
  - waits for BTC RPC and starts the shared `ord server`
- [start_sourcedao_bootstrap.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_sourcedao_bootstrap.sh)
  - runs the one-shot SourceDAO bootstrap job and writes state markers
- [start_usdb_indexer.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_usdb_indexer.sh)
  - renders config, waits for dependencies, then starts `usdb-indexer`
- [start_world_sim.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_world_sim.sh)
  - handles world-sim bootstrap and loop execution inside the tools image
  - note: this script is internal; prefer `tools/run_local_world_sim.sh` for operator flows
- [start_world_sim_bitcoind.sh](/home/bucky/work/usdb/docker/scripts/entrypoints/start_world_sim_bitcoind.sh)
  - starts the regtest `bitcoind` used by world-sim images

## Usage Notes

- These scripts are internal container entrypoints; they are not intended to be
  the normal operator interface.
- Compose and Dockerfiles should reference these canonical paths directly.
