"""Two installed networks sharing source datasets, with generated fake systemd units."""

from dataclasses import replace
import json
from pathlib import Path
from types import SimpleNamespace
import subprocess

import control_plane_monitor
import node_monitor
import node_upgrade
import runtime_compatibility as runtime
import usdb_node as node
from common.node_upgrade import UpgradeFixture, write_manifest


class NetworkSwitchFixture(UpgradeFixture):
    def __init__(self, root):
        super().__init__(root, native=True)
        old_bundle = "usdb-testnet-v0"
        original = self.source_kit
        self.source_kit = original.with_name(old_bundle + "-r4")
        original.rename(self.source_kit)
        bundled = self.source_kit / "docker/networks" / old_bundle
        (bundled.parent / self.bundle).rename(bundled)
        raw = json.loads((bundled / "network.json").read_text())
        raw["network_bundle_id"] = old_bundle
        (bundled / "network.json").write_text(json.dumps(raw))
        self.old["release_id"] = old_bundle + "-r4"
        self.old["network_bundle"].update(bundle_id=old_bundle, network_json_sha256=node_upgrade.sha(bundled / "network.json"))
        self.old["runtime_compatibility"] = runtime.build_runtime_compatibility(self.old["network_bundle"])
        write_manifest(self.source_kit, self.old)
        config = self.config.with_name(old_bundle)
        self.config.rename(config)
        self.config = config
        self.env_path = config / "node.env"
        (self.data / "networks" / self.bundle).rename(self.data / "networks" / old_bundle)
        env = node.read_env(self.env_path)
        env = {k: v.replace("/networks/" + self.bundle + "/", "/networks/" + old_bundle + "/") for k, v in env.items()}
        self.paths = runtime.build_persistent_data_paths(self.data, self.old["network_bundle"], self.old["runtime_compatibility"])
        env.update({k: str(p) for k, p in self.paths.items()})
        env["USDB_RUNTIME_COMPATIBILITY_ID"] = self.old["runtime_compatibility"]["compatibility_id"]
        self.env_path.write_text(node.upsert_env("", env))
        for key, service in runtime.PERSISTENT_DATA_SERVICES.items():
            (self.paths[key] / runtime.DATASET_IDENTITY_FILE).write_text(json.dumps(runtime.build_dataset_identity(service, self.old["runtime_compatibility"]), indent=2, sort_keys=True) + "\n")
        self.source = replace(self.target, bundle_id=old_bundle, node_env=self.env_path, kit_root=self.source_kit,
                              release_id=self.old["release_id"], network_identity=self.old["network_bundle"],
                              runtime_compatibility=self.old["runtime_compatibility"])
        self.units = root / "units"
        self.units.mkdir()
        self.context = SimpleNamespace(launcher=self.home / ".local/bin/usdb-node", docker_launcher=Path("/usr/bin/docker"),
                                       service_user="operator", home=self.home)
        self.states, self.commands = {}, []
        self.install_unit("usdb-node-bootstrap", node.render_controller_unit(self.source, **vars(self.context)))
        self.install_unit("usdb-node-monitor", node_monitor.render_unit(self.source, node, self.context))
        self.install_unit("usdb-console-monitor", control_plane_monitor.legacy_render_unit(self.source, node, self.context))

    def controller(self, layout):
        return self.units / f"usdb-node-bootstrap-{layout.bundle_id}.service"

    def install_unit(self, prefix, content):
        path = self.units / f"{prefix}-{self.source.bundle_id}.service"
        path.write_text(content)
        self.states[path.name] = dict(LoadState="loaded", ActiveState="inactive", SubState="dead", UnitFileState="enabled",
                                     NeedDaemonReload="no", FragmentPath=str(path), DropInPaths="", MainPID="0")
        return path

    def probe(self, command, **kwargs):
        assert command[:2] == ["systemctl", "show"], command
        state = self.states[command[-1]]
        return subprocess.CompletedProcess(command, 0, "\n".join(f"{k}={v}" for k, v in state.items()), "")

    def mutate(self, command, **kwargs):
        assert command[:3] == ["systemctl", "disable", "--now"], command
        self.commands.append(command)
        self.states[command[-1]].update(UnitFileState="disabled", ActiveState="inactive", SubState="dead", MainPID="0")
        return subprocess.CompletedProcess(command, 0)
