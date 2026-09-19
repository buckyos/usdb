"""Isolate interactive setup from host inspection, credentials and service changes."""
from contextlib import ExitStack
from copy import deepcopy
import io
from pathlib import Path
import tempfile
from types import SimpleNamespace
from unittest import mock

import usdb_node as NODE
import usdb_p2p as P2P
from common.p2p import HOST


class SetupFixture:
    def __enter__(self):
        self.stack = ExitStack()
        self.root = Path(self.stack.enter_context(tempfile.TemporaryDirectory()))
        self.layout = SimpleNamespace(node_env=self.root / "node.env", snapshot={"status": "none"},
                                      release_id="usdb-testnet-v0-r1")
        self.host = deepcopy(HOST)
        self.output = io.StringIO()
        self.stack.enter_context(mock.patch.object(P2P, "host_capabilities", side_effect=lambda: deepcopy(self.host)))
        self.engine = self.stack.enter_context(mock.patch.object(P2P, "engine_capabilities", return_value={}))
        self.stack.enter_context(mock.patch.object(NODE, "effective_memory_bytes", return_value=64 * 1024**3))
        self.stack.enter_context(mock.patch.object(NODE, "_data_root_capacity", return_value=NODE.DataRootCapacity(
            filesystem_path=self.root, total_bytes=3 * 1024**4, free_bytes=3 * 1024**4)))
        self.configure = self.stack.enter_context(mock.patch.object(NODE, "configure_node", return_value=self.layout.node_env))
        return self

    def run(self, *, family="auto", seeds="", confirm="y", before_confirm=None, options=None):
        """Observe the rendered preview at the exact point consent is requested."""
        answers = iter([str(self.root / "data"), "full", seeds, family, "n", "n", "n", confirm])

        def answer(prompt):
            if prompt.startswith("Write this node configuration") and before_confirm:
                before_confirm(self.output.getvalue())
            return next(answers)

        return NODE.setup_node(self.layout, input_fn=answer, output=self.output, p2p_options=options)

    def __exit__(self, *args):
        return self.stack.__exit__(*args)
