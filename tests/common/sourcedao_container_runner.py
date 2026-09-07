#!/usr/bin/env python3
"""Opt-in container acceptance adapter for a public-key, temporary Hardhat fixture.

Only the test release layout and local image resolution are replaced. The managed
operation, mounts, Docker process lifecycle, journal and tool commands are real.
"""
import json
from pathlib import Path
import sys
from types import SimpleNamespace

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "docker/scripts/tools"))
import usdb_sourcedao as DAO
from sourcedao_release import semantic_digest

root, bundle, rpc, image, action, *arguments = sys.argv[1:]
root, bundle = Path(root), Path(bundle)
network = json.loads((bundle / "network.json").read_text())
artifact = lambda key: json.loads((bundle / network["artifacts"][key]["path"]).read_text())
config, freeze, genesis = artifact("sourcedao_bootstrap"), artifact("sourcedao_bootstrap_freeze"), artifact("genesis_manifest")
binding = {"network": network["network_bundle_id"], "chain_id": config["chainId"], "genesis_hash": genesis["block_hash"],
           "config_sha256": freeze["config_semantic_sha256"], "golden_sha256": freeze["golden_sha256"], "image": image}
layout = SimpleNamespace(node_env=root / "node.env", bundle_id=network["network_bundle_id"], bundle_dir=bundle, release_id="managed-container-fixture")
relative = Path(str(config["chainId"])) / genesis["block_hash"] / binding["config_sha256"]
private, public = DAO.root(layout) / "private", DAO.root(layout) / "public"
ctx = {"binding": binding, "private_root": private, "public_root": public, "state": private / relative / "state.json",
       "public_state": public / relative / "sourcedao-bootstrap-public-state.json", "validation": public / relative / "sourcedao-bootstrap-validation.json", "rpc": rpc}
DAO.context = lambda _layout: ctx
real_docker = DAO.docker
def docker(args, **kwargs):
    if args[0] == "pull":
        return real_docker(["image", "inspect", image, "--format", "{{.Id}}"])
    return real_docker(args, **kwargs)
DAO.docker = docker
if action == "paths":
    print(json.dumps({key: str(ctx[key]) for key in ("state", "public_state", "validation")} | {"container": DAO.container_name(layout)}))
else:
    args = DAO.node.build_parser().parse_args(["sourcedao", action, "--json", *arguments])
    sys.exit(DAO.execute(layout, args))
