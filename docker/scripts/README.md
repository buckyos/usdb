# Docker Scripts

`docker/scripts/` now separates shell scripts by role instead of keeping every
script in one flat directory.

## Layout

- [tools/README.md](/home/bucky/work/usdb/docker/scripts/tools/README.md)
  - user-facing helpers you can run directly from the repo root
  - includes runtime profile, service coverage, network, and env comparison tables
- [entrypoints/README.md](/home/bucky/work/usdb/docker/scripts/entrypoints/README.md)
  - container entrypoints used by Compose services and Docker images
- [helpers/README.md](/home/bucky/work/usdb/docker/scripts/helpers/README.md)
  - shared helper libraries and rendering utilities sourced or invoked by other scripts

## Compatibility

Common repo-facing commands are still available at the historical paths under
`docker/scripts/*.sh` via thin wrappers. New internal references should prefer
the categorized paths above.
