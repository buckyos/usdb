"""Small, offline installed-kit/configuration fixtures for post-install advice."""

import json
from pathlib import Path
import shutil

import node_upgrade as upgrade
import node_upgrade_archives as archives
import node_upgrade_session as session
from prepare_release_node_kit import NODE_KIT_FILES
import usdb_node as node
from common.node_upgrade import ROOT, write_manifest


def minimal_manifest(release):
    """Only metadata consumed by offline installation guidance is required."""
    return dict(schema_version=node.RELEASE_MANIFEST_SCHEMA_VERSION, release_id=release,
                network_bundle=dict(bundle_id=release.rsplit("-r", 1)[0]))


def copy_tools(kit):
    """Use the packaging allowlist so missing runtime imports fail integration tests."""
    for name in NODE_KIT_FILES:
        if name.startswith("docker/scripts/tools/") and name.endswith(".py"):
            path = kit / name
            path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, path)


class GuidanceFixture:
    def __init__(self, root):
        self.root = root
        self.home = root / "home"
        self.home.mkdir()
        self.bin = self.home / ".local/bin"
        self.bundle = "usdb-testnet-v12"
        self.kit = self.make_kit(self.bundle + "-r11")
        self.data = root / "retained data"

    def make_kit(self, release):
        kit = self.root / "releases" / release
        write_manifest(kit, minimal_manifest(release))
        script = kit / "docker/scripts/tools/usdb_node.py"
        script.parent.mkdir(parents=True)
        script.write_text("# Fixture: recovery guidance must not execute this file.\n")
        return kit

    def config(self, bundle):
        path = self.home / ".config/usdb" / bundle / "node.env"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"USDB_DATA_ROOT={self.data}\nBTC_RPC_PASSWORD=private-secret-fixture\n")
        return path

    def record(self, bundle, *, phase="prepared", marker=True, register=False):
        config = self.config(bundle)
        kit = self.make_kit(bundle + "-r9")
        backup = self.root / "saved backup"
        backup.mkdir()
        plan = dict(schema_version=upgrade.SCHEMA, target_kit=str(kit), target_release=bundle + "-r9",
                    env_path=str(config), target_manifest_sha256=upgrade.sha(kit / "release/usdb-release-manifest.json"))
        record = dict(schema_version=session.SCHEMA, operation_id="a"*32, phase=phase, plan=plan)
        (backup / "upgrade.json").write_text(json.dumps(record))
        pointer = dict(operation_id=record["operation_id"], backup_dir=str(backup))
        if marker:
            (config.parent / upgrade.PENDING).write_text(json.dumps(dict(pointer, target_release=plan["target_release"])))
        if register:
            catalog = self.data / archives.CATALOG
            catalog.mkdir(parents=True)
            (catalog / (record["operation_id"] + ".json")).write_text(json.dumps(pointer))
        return config, backup, kit
