"""Private node fixture with real release inputs and a controllable Docker daemon."""
from contextlib import ExitStack
from copy import deepcopy
import json
from unittest import mock

from test_usdb_node import UsdbNodeTests
import usdb_sourcedao as DAO


class SourceDaoFixture:
    def __enter__(self):
        self.stack = ExitStack()
        self.node = UsdbNodeTests()
        self.node.setUp()
        self.stack.callback(self.node.tearDown)
        revision = "c" * 40
        self.node.manifest["repositories"] = {"source_dao": {"repository": "buckyos/SourceDAO", "revision": revision}}
        self.node.manifest["images"]["sourcedao_tools"].update(source_revision=revision, source_repository="buckyos/SourceDAO", platform="linux/amd64",
            attestation={"repository": "buckyos/SourceDAO", "signer_workflow": "buckyos/SourceDAO/.github/workflows/usdb-tools-image.yml"})
        self.node.write_manifest()
        self.layout = DAO.node.load_release_layout(self.node.root, self.node.node_env)
        self.node.configure_full_node(self.layout, "sourcedao-test")
        self.ctx = DAO.context(self.layout)
        self.key = self.node.root / "admin.key"
        self.key.write_text("PRIVATE_KEY_SENTINEL")
        self.key.chmod(0o600)
        self.container = None
        self.calls = []
        self.failure = None
        self.live = {"schema_version": "sourcedao-bootstrap-check:v1", **{k: v for k, v in self.ctx["binding"].items() if k != "image"},
                     "finalized": False, "blockers": [], "checkpoint": {"number": 15}, "bootstrap_admin": "admin", "ready_for_bootstrap": True}
        self.stack.enter_context(mock.patch.object(DAO, "docker", side_effect=self.docker))
        return self

    def __exit__(self, *args):
        return self.stack.__exit__(*args)

    def docker(self, args, **kwargs):
        self.calls.append(list(args))
        if self.failure and args[:len(self.failure)] == self.failure:
            raise ValueError("injected Docker failure")
        if args[:2] == ["container", "ls"]:
            return "a" * 64 if self.container else ""
        if args[:2] == ["container", "inspect"]:
            return json.dumps([self.container])
        if args[0] == "pull":
            return "pulled"
        if args[:2] == ["image", "inspect"]:
            return "cached-image"
        if args[0] == "run":
            return json.dumps(self.live)
        if args[0] == "create":
            if self.container:
                raise ValueError("container already exists")
            labels = dict(args[i + 1].split("=", 1) for i, value in enumerate(args) if value == "--label")
            self.container = {"Id": "a" * 64, "Config": {"Image": self.ctx["binding"]["image"], "Labels": labels},
                              "State": {"Status": "created", "ExitCode": 0, "OOMKilled": False}}
            return "a" * 64
        if args[0] == "start":
            self.container["State"]["Status"] = "running"
            return "started"
        if args[0] == "logs":
            return "private log"
        if args[:2] == ["container", "rm"]:
            self.container = None
            return "removed"
        raise AssertionError(args)

    def start(self, action="bootstrap"):
        return DAO.start(self.layout, action, key=self.key if action == "bootstrap" else None)

    def finish(self, *, exit_code=0, completed=True):
        self.container["State"].update(Status="exited", ExitCode=exit_code)
        if completed:
            identity = {k: v for k, v in self.ctx["binding"].items() if k not in {"image", "network"}}
            DAO.node._atomic_write_private(self.ctx["state"], json.dumps({"status": "completed", "ceremony_identity": identity}))
            self.live["finalized"] = True

    def stale_lock(self, task_id):
        path = self.ctx["state"].with_name("state.json.lock")
        DAO.node._atomic_write_private(path, json.dumps({"managed_task_id": task_id}))
        return path
