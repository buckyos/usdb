# Docker Scripts

`docker/scripts/` now separates shell scripts by role instead of keeping every
script in one flat directory.

## Layout

- [tools/README.md](/home/bucky/work/usdb/docker/scripts/tools/README.md)
  - user-facing helpers you can run directly from the repo root
  - includes runtime profile, service coverage, network, and env comparison tables
- [TOOL_NAMING_PLAN.md](/home/bucky/work/usdb/docker/scripts/TOOL_NAMING_PLAN.md)
  - canonical naming scheme for current local tools and future `testnet` / `mainnet` profiles
- [entrypoints/README.md](/home/bucky/work/usdb/docker/scripts/entrypoints/README.md)
  - container entrypoints used by Compose services and Docker images
- [helpers/README.md](/home/bucky/work/usdb/docker/scripts/helpers/README.md)
  - shared helper libraries and rendering utilities sourced or invoked by other scripts

## Usage

Repo-facing helper commands now live directly under `docker/scripts/tools/`.
There is no separate legacy wrapper layer under `docker/scripts/*.sh`.
