#!/usr/bin/env python3
"""Exercise setup admission and Core reuse without touching host data or services."""

import io
import os
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "docker/scripts/tools"))
import node_storage as storage
import usdb_node as node
import usdb_p2p as p2p
from common.native_node import native_kit
from common.p2p import HOST, V4

GIB = 1024**3


class NodeStorageTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name)
        self.layout = native_kit(self.root)
        self.data = self.root / "data"
        self.bitcoin = node._data_directories(self.layout, self.data)["BTC_NODE_DATA_HOST_DIR"]
        self.bitcoin.mkdir(parents=True)
        self.marker = self.bitcoin / node.DATASET_IDENTITY_FILE
        self.marker.write_text(node._dataset_marker_content("bitcoin_core", self.layout))

    def file(self, relative, data=b"blocks" * 1024):
        path = self.bitcoin / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        return path

    def count(self):
        return storage.retained_bitcoin_bytes(self.data, self.bitcoin, node.DATASET_IDENTITY_FILE,
                                              node._dataset_marker_content("bitcoin_core", self.layout))

    def capacity(self, free=2 * 1024 * GIB, total=3 * 1024 * GIB):
        return node.DataRootCapacity(self.data, total, free)

    def test_counts_only_allocated_core_stores_without_links_or_optional_indexes(self):
        files = [self.file("blocks/blk00000.dat"), self.file("blocks/index/00001.ldb"),
                 self.file("chainstate/00001.ldb"), self.file("chainstate_snapshot/00001.ldb")]
        self.file("indexes/txindex/00001.ldb")
        self.file("debug.log")
        self.file("wallets/wallet.dat")
        self.file("backup/blk00000.dat")
        external = self.root / "external"
        external.mkdir()
        (external / "huge").write_bytes(b"outside" * 1024)
        (self.bitcoin / "blocks/linked-dir").symlink_to(external)
        (self.bitcoin / "blocks/linked-file").symlink_to(external / "huge")
        os.link(external / "huge", self.bitcoin / "blocks/hardlink")
        sparse = self.file("blocks/blk00001.dat", b"")
        with sparse.open("wb") as output:
            output.truncate(GIB)
        files.append(sparse)
        expected = sum(min(f.stat().st_size, f.stat().st_blocks * 512) for f in files)
        self.assertEqual(self.count(), (expected, ""))
        self.assertLess(expected, GIB)

    def test_identity_missing_changed_or_symlinked_receives_no_credit(self):
        self.file("blocks/blk00000.dat")
        self.marker.unlink()
        self.assertEqual(self.count()[0], 0)
        self.marker.write_text("{}")
        self.assertEqual(self.count()[0], 0)
        self.marker.unlink()
        target = self.root / "marker"
        target.write_text(node._dataset_marker_content("bitcoin_core", self.layout))
        self.marker.symlink_to(target)
        self.assertEqual(self.count()[0], 0)

    def test_symlinked_store_root_and_foreign_filesystem_are_not_credited(self):
        self.file("blocks/blk00000.dat")
        moved = self.root / "external-bitcoin"
        self.bitcoin.rename(moved)
        self.bitcoin.symlink_to(moved)
        self.assertEqual(self.count()[0], 0)
        self.bitcoin.unlink()
        moved.rename(self.bitcoin)
        original = Path.stat

        def metadata(path, *args, **kwargs):
            result = original(path, *args, **kwargs)
            if path == self.data:
                return SimpleNamespace(st_dev=result.st_dev + 1, st_mode=result.st_mode)
            return result

        with mock.patch.object(Path, "stat", metadata):
            self.assertEqual(self.count()[0], 0)

    def test_unreadable_store_cannot_partially_reduce_requirement(self):
        self.file("blocks/blk00000.dat")
        with mock.patch.object(storage.os, "scandir", side_effect=PermissionError(13, "Permission denied")):
            count, note = self.count()
        self.assertEqual(count, 0)
        self.assertIn("Cannot measure", note)

    def test_reuse_reduces_free_requirement_but_preserves_floor_total_and_ord_budget(self):
        for retained, free, extra, required in (
            (0, 1536 * GIB, 0, 1536 * GIB),
            (800 * GIB, 736 * GIB, 0, 736 * GIB),
            (1500 * GIB, 512 * GIB, 0, 512 * GIB),
            (1500 * GIB, 812 * GIB, 300 * GIB, 812 * GIB),
        ):
            with self.subTest(retained=retained, extra=extra), \
                    mock.patch.object(storage, "retained_bitcoin_bytes", return_value=(retained, "")), \
                    mock.patch.object(node, "_data_root_capacity", return_value=self.capacity(free)) as capacity:
                result = node._validate_data_root_capacity(self.data, layout=self.layout, extra_bytes=extra)
                self.assertEqual(result.required_free_bytes, required)
                capacity.return_value = self.capacity(free - 1)
                with self.assertRaisesRegex(ValueError, "insufficient available space"):
                    node._validate_data_root_capacity(self.data, layout=self.layout, extra_bytes=extra)
                capacity.return_value = self.capacity(free, node.MIN_DATA_ROOT_BYTES + extra - 1)
                with self.assertRaisesRegex(ValueError, "filesystem is too small"):
                    node._validate_data_root_capacity(self.data, layout=self.layout, extra_bytes=extra)

    def test_preview_precedes_input_and_invalid_or_empty_answers_cannot_bypass(self):
        output = io.StringIO()
        small = self.root / "small"
        good = self.root / "good"
        answers = iter([str(small), "", str(good)])
        prompts = []

        def answer(prompt):
            prompts.append(prompt)
            self.assertIn("Total capacity:", output.getvalue())
            self.assertIn("Available now:", output.getvalue())
            return next(answers)

        def capacity(path):
            return self.capacity(2 * 1024 * GIB if path == good else 100 * GIB)

        with mock.patch.object(node, "_data_root_capacity", side_effect=capacity):
            result = node._select_data_root(self.layout, default=small, input_fn=answer, output=output)
        self.assertEqual(result, good)
        self.assertEqual(len(prompts), 3)
        self.assertEqual(output.getvalue().count("Cannot use this data directory:"), 2)
        self.assertIn("cannot be skipped", output.getvalue())
        self.assertFalse(good.exists())
        self.assertFalse(self.layout.node_env.exists())

    def test_bad_path_and_interrupt_preserve_existing_data_without_configuration(self):
        invalid = self.root / "ordinary-file"
        invalid.write_bytes(b"preserve")
        output = io.StringIO()
        for failure in (EOFError(), KeyboardInterrupt()):
            with self.subTest(failure=type(failure).__name__):
                answer = mock.Mock(side_effect=[str(invalid), failure])
                expected = ValueError if isinstance(failure, EOFError) else KeyboardInterrupt
                with self.assertRaises(expected):
                    node._select_data_root(self.layout, default=invalid, input_fn=answer, output=output)
                self.assertEqual(invalid.read_bytes(), b"preserve")
                self.assertFalse(self.layout.node_env.exists())

    def test_configure_rechecks_retained_space_before_writing(self):
        self.file("blocks/blk00000.dat")
        before = self.marker.read_bytes()
        args = dict(data_root=self.data, role="full", miner_address="", miner_threads=1,
                    bootnodes="", nat="", bitcoin_rpc_user=None, bitcoin_p2p="private", resource_management="auto")
        with mock.patch.object(storage, "retained_bitcoin_bytes", return_value=(800 * GIB, "")), \
                mock.patch.object(node, "_data_root_capacity", return_value=self.capacity(736 * GIB)) as capacity, \
                mock.patch.object(node, "effective_memory_bytes", return_value=64 * GIB):
            node._validate_data_root_capacity(self.data, layout=self.layout)
            capacity.return_value = self.capacity(700 * GIB)
            with self.assertRaisesRegex(ValueError, "insufficient available space"):
                node.configure_node(self.layout, **args)
            self.assertFalse(self.layout.node_env.exists())
            self.assertFalse((self.data / "networks").exists())
            capacity.return_value = self.capacity(736 * GIB)
            node.configure_node(self.layout, **args)
        self.assertEqual(self.marker.read_bytes(), before)
        self.assertEqual(node.read_env(self.layout.node_env)["BTC_NODE_DATA_HOST_DIR"], str(self.bitcoin))

    def test_enabling_ord_repeats_directory_selection_with_extra_requirement(self):
        other = self.root / "larger"
        directories = iter([str(self.data), str(other)])
        output = io.StringIO()

        def answer(prompt):
            if prompt.startswith("Host data root"):
                return next(directories)
            return "y" if prompt.startswith("Enable local minting backend") else ""

        def capacity(path):
            return self.capacity(1536 * GIB if path == self.data else 2100 * GIB)

        args = node.build_parser().parse_args(["setup", "--p2p-ip-family", "ipv4", "--advertise-ipv4", V4])
        with mock.patch.object(node, "_data_root_capacity", side_effect=capacity), \
                mock.patch.object(node, "effective_memory_bytes", return_value=64 * GIB), \
                mock.patch.object(p2p, "host_capabilities", return_value=HOST), \
                mock.patch.object(node, "configure_node", return_value=self.layout.node_env) as configure:
            node.setup_node(self.layout, input_fn=answer, output=output,
                            resource_management="auto", p2p_options=p2p.options(args))
        self.assertEqual(configure.call_args.kwargs["data_root"], other)
        self.assertTrue(configure.call_args.kwargs["minting"])
        self.assertIn("Additional Ord budget: 300.0 GiB", output.getvalue())
        self.assertIn("Cannot enable Ord", output.getvalue())


if __name__ == "__main__":
    unittest.main()
