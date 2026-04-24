# Docker Script Helpers

This directory contains shared helper libraries and rendering utilities that are
used by the tool and entrypoint scripts.

## Scripts

- [bootstrap_local_inputs_common.sh](/home/bucky/work/usdb/docker/scripts/helpers/bootstrap_local_inputs_common.sh)
  - shared helper used by local bootstrap-oriented tool scripts to scaffold env
    files and manifests
- [ethw_bootstrap_artifact.sh](/home/bucky/work/usdb/docker/scripts/helpers/ethw_bootstrap_artifact.sh)
  - ETHW genesis artifact validation and signature helper functions
- [ethw_init_marker.sh](/home/bucky/work/usdb/docker/scripts/helpers/ethw_init_marker.sh)
  - read and write helpers for the ETHW init marker file
- [render_balance_history_config.sh](/home/bucky/work/usdb/docker/scripts/helpers/render_balance_history_config.sh)
  - renders `balance-history` config from environment variables
- [render_control_plane_config.sh](/home/bucky/work/usdb/docker/scripts/helpers/render_control_plane_config.sh)
  - renders `usdb-control-plane` config from environment variables
- [render_usdb_indexer_config.sh](/home/bucky/work/usdb/docker/scripts/helpers/render_usdb_indexer_config.sh)
  - renders `usdb-indexer` config from environment variables
- [snapshot_marker.sh](/home/bucky/work/usdb/docker/scripts/helpers/snapshot_marker.sh)
  - read and write helpers for the balance-history snapshot marker
- [wait_for_tcp.sh](/home/bucky/work/usdb/docker/scripts/helpers/wait_for_tcp.sh)
  - tiny TCP readiness probe used by multiple entrypoints

## Usage Notes

- Most files here are sourced or called by other scripts and are not intended to
  be run directly by developers.
- Keep shared shell functions here when they are reused across more than one
  tool or entrypoint script.
