# Docker Script Tools

This directory contains the repo-facing helpers that developers are expected to
run directly.

## Scripts

- [build_world_sim_release_images.sh](/home/bucky/work/usdb/docker/scripts/tools/build_world_sim_release_images.sh)
  - packages host `bitcoind`, `bitcoin-cli`, and `ord` binaries into the local
    world-sim images
  - use this before the first `run_world_sim.sh build-images` or when the host
    binaries change
  - note: this script assumes the host paths configured by the
    `WORLD_SIM_RELEASE_*` variables are valid
- [run_console_preview.sh](/home/bucky/work/usdb/docker/scripts/tools/run_console_preview.sh)
  - starts the smallest stack needed to preview the web console against the BTC
    services
  - use this when working on control-plane and console pages without ETHW,
    world-sim, or SourceDAO
- [run_container_smoke.sh](/home/bucky/work/usdb/docker/scripts/tools/run_container_smoke.sh)
  - runs a container-level smoke flow over the bootstrap stack
  - use this when validating cold-start bootstrap artifacts and service wiring
  - note: it creates temporary local files and can leave the stack running when
    `KEEP_RUNNING=1`
- [run_dev_full_runtime.sh](/home/bucky/work/usdb/docker/scripts/tools/run_dev_full_runtime.sh)
  - starts the BTC + ord + ETHW local runtime without world-sim or SourceDAO
  - use this when you need the full runtime profile but not simulation
- [run_dev_full_sim.sh](/home/bucky/work/usdb/docker/scripts/tools/run_dev_full_sim.sh)
  - starts the complete local development simulation stack
  - use this for the full console + world-sim + ETHW + SourceDAO path
  - note: `down` only stops containers; `reset` also removes volumes
- [run_sourcedao_bootstrap.sh](/home/bucky/work/usdb/docker/scripts/tools/run_sourcedao_bootstrap.sh)
  - prepares local bootstrap inputs and runs the SourceDAO bootstrap stack
  - use this when testing SourceDAO bootstrap artifacts in isolation
- [run_world_sim.sh](/home/bucky/work/usdb/docker/scripts/tools/run_world_sim.sh)
  - starts the BTC-side world-sim stack, with optional ETHW alignment via
    `up-full`
  - use this for simulation-only work, deterministic wallet state, and world-sim
    debugging
  - note: the helper now starts detached by default and includes a `doctor`
    action for readiness diagnostics

## Compatibility Wrappers

Thin wrappers remain at `docker/scripts/*.sh` for the tool scripts above so
existing commands and docs continue to work.
