#!/usr/bin/env python3
"""Real wrapper/Rust workflow checks; S3 and public transport are replaced with an in-memory boundary."""

from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
from unittest import mock

REPO = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(REPO / "docker/scripts/tools"))
import baseline_snapshot_release as baseline
from common.assumeutxo_services import CoreProxy, RpcError


class Store:
    def __init__(self):
        self.objects = {}
        self.uploads = []

    def head(self, key):
        if key not in self.objects:
            return None
        data, digest, size = self.objects[key]
        return {"ContentLength": size, "Metadata": {"usdb-sha256": digest, "usdb-size": str(size)}}

    def upload(self, path, key, digest, size, _content_type):
        data = path.read_bytes()
        assert len(data) == size and hashlib.sha256(data).hexdigest() == digest
        self.objects[key] = (data, digest, size)
        self.uploads.append(key)

    def public(self, url, item):
        key = url.removeprefix("https://snapshot.example/")
        data = self.objects[key][0]
        if len(data) != item["size_bytes"] or hashlib.sha256(data).hexdigest() != item["sha256"]:
            raise ValueError("simulated public hash mismatch")


def main(root: Path, tool: Path):
    inputs = json.loads((root / "fixture.json").read_text())
    env = dict(os.environ, SNAPSHOT_ROOT=str(root / "release"), SNAPSHOT_KEY_ROOT=inputs["key_root"],
               SNAPSHOT_SIGNER_ID="baseline-workflow-test", SNAPSHOT_TOOL_BIN=str(tool),
               BALANCE_HISTORY_ROOT=str(root / "live"), BITCOIN_BIN_DIR=str(root / "absent-bitcoin"),
               SNAPSHOT_PUBLIC_BASE_URL="https://snapshot.example", SNAPSHOT_UPLOAD_PROGRESS="0")
    script = REPO / "src/btc/balance-history/scripts/mainnet_exact_height_snapshot.sh"

    def run(action, *extra, failure=None):
        current = dict(env)
        if failure:
            current["USDB_BH_SNAPSHOT_TEST_FAIL_AT_CHECKPOINT"] = failure
        result = subprocess.run(["bash", str(script), action, "--snapshot-type", "baseline", "--height", "103",
                                 "--block-hash", inputs["hash"], *extra], env=current, capture_output=True, text=True, timeout=60)
        if failure:
            assert result.returncode != 0, result.stdout
            return
        assert result.returncode == 0, result.stderr
        return json.loads(result.stdout)

    run("create", "--core-manifest", inputs["core"], "--registry-manifest", inputs["registry"],
        "--genesis-block", inputs["block"], "--network", "regtest", "--batch-size", "7", failure="baseline_utxos")
    assert run("status")["checkpoint"] == ["utxos", 7]
    assert run("resume-verify")["stage"] == "complete"
    manifest = run("verify")
    assert manifest["source"]["kind"] == "legacy_split"
    first = run("finalize")
    assert run("finalize") == first
    prepared = run("prepare-release")
    record = json.loads(Path(prepared["record_file"]).read_text())
    baseline.validate_record(record)
    assert record["files"]["database"]["sha256"] == manifest["file_sha256"]
    for mutate in [
        lambda value: value.update(schema_version="unknown"),
        lambda value: value["files"]["database"].update(object_key="../escape.db"),
        lambda value: value["files"]["manifest"].update(name="other.json"),
        lambda value: value["identity"].update(history_query_floor=103),
        lambda value: value["files"].update(private_key={}),
    ]:
        invalid = deepcopy(record)
        mutate(invalid)
        try:
            baseline.validate_record(invalid)
        except ValueError:
            pass
        else:
            raise AssertionError("invalid release record was accepted")

    release = baseline.Release(root / "release/releases/baseline", root / "release/builder/baseline",
                               tool, 103, inputs["hash"], Path(inputs["trust"]), "baseline-workflow-test")
    store = Store()
    with mock.patch.object(baseline, "verify_public_file", side_effect=ValueError("public unavailable")):
        try:
            release.publish("https://snapshot.example", store)
        except ValueError as error:
            assert "public unavailable" in str(error)
        else:
            raise AssertionError("public failure should abort publication")
    assert not (release.root / "publish-result.json").exists()
    assert len(store.uploads) == 1
    with mock.patch.object(baseline, "verify_public_file", side_effect=store.public):
        published = release.publish("https://snapshot.example", store)
        assert published["objects"]["database"] == "existing"
        assert set(published["objects"]) == {"database", "manifest", "signature", "trusted_keys", "record"}
        assert len(store.uploads) == 5
        again = release.publish("https://snapshot.example", store)
        assert set(again["objects"].values()) == {"existing"}
        assert len(store.uploads) == 5
        assert release.verify_published("https://snapshot.example")["status"] == "public_verified"
        database_key = record["files"]["database"]["object_key"]
        original, digest, size = store.objects[database_key]
        store.objects[database_key] = (b"!" + original[1:], digest, size)
        try:
            release.publish("https://snapshot.example", store)
        except ValueError as error:
            assert "public hash mismatch" in str(error)
        else:
            raise AssertionError("remote metadata must not substitute for checking bytes")
        store.objects[database_key] = (original, digest, size)
    for data, _, _ in store.objects.values():
        assert b"secret_key_base64" not in data
        assert str(root).encode() not in data
    blocks = json.loads((REPO / "tests/fixtures/assumeutxo-p5/chain.json").read_text())["blocks"]
    hashes = [hashlib.sha256(hashlib.sha256(bytes.fromhex(block)[:80]).digest()).digest()[::-1].hex() for block in blocks]

    def rpc(method, *params):
        if method == "getblockcount":
            return 1000  # The exact producer ceiling, rather than the remote tip, must stop sync.
        if method == "getblockhash" and 0 <= params[0] < len(hashes):
            return hashes[params[0]]
        if method == "getblock" and params[0] in hashes and params[1:] == (0,):
            return blocks[hashes.index(params[0])]
        raise RpcError({"code": -1, "message": f"Unexpected fixture RPC: {method}"})

    proxy = CoreProxy(rpc)
    try:
        for kind, provenance in [("full", "full_replay"), ("native", "assumeutxo")]:
            config = root / f"managed-{kind}.toml"
            config.write_text(config.read_text().replace("http://127.0.0.1:1", f"http://127.0.0.1:{proxy.port}"))
            env["SNAPSHOT_ROOT"] = str(root / f"managed-{kind}")
            created = run("create", "--config", str(config), "--network", "regtest")
            assert created["stage"] == "complete"
            checked = run("verify")
            assert checked["source"]["kind"] == provenance
            assert checked["logical_sha256"] == "2040d80f844736e55d7a6d71683c72d9c08c701750caef7a94a142c5b7b2a960"
            calls = dict(proxy.calls)
            assert run("create", "--config", str(config), "--network", "regtest") == created
            assert dict(proxy.calls) == calls, "Completed create must reuse its frozen source without reopening or syncing"
    finally:
        proxy.close()
    print("Baseline workflow passed: real conversion/resume/verify/finalize; simulated public failure/retry/corruption")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))
