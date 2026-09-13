#!/usr/bin/env python3
"""Accept native bootstrap with real Core, two balance-history and two indexer services."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import time

from common.assumeutxo_services import CoreProxy, Processes, Rpc, RpcError, capture_anchor, free_port


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--bitcoind", required=True, type=Path)
    parser.add_argument("--balance-history", type=Path)
    parser.add_argument("--indexer", type=Path)
    args = parser.parse_args()
    if not __debug__:
        parser.error("Acceptance assertions require Python without -O/PYTHONOPTIMIZE")
    repo = Path(__file__).resolve().parents[1]
    bh_binary = (args.balance_history or repo / "src/btc/target/release/balance-history").resolve()
    indexer_binary = (args.indexer or repo / "src/btc/target/release/usdb-indexer").resolve()
    fixture = repo / "tests/fixtures/assumeutxo-p5"
    root = Path(tempfile.mkdtemp(prefix="usdb-p65-services-"))
    print(f"P6.5 isolated run: {root}", flush=True)
    processes = Processes(root)
    start = time.monotonic()
    result = dict(status="running", run=str(root), stages=[],
                  scope="Real regtest services; Core prefix submitted, not loadtxoutset dual-chainstate or mainnet performance")
    proxy = None

    def file_sha256(path):
        with path.open("rb") as source:
            return hashlib.file_digest(source, "sha256").hexdigest()

    def stage(name, **details):
        item = dict(name=name, elapsed_seconds=round(time.monotonic() - start, 3), **details)
        result["stages"].append(item)
        print(json.dumps(item), flush=True)

    def core_start(name):
        data = root / name
        data.mkdir()
        port = free_port()
        processes.start(name, [args.bitcoind, "-regtest", f"-datadir={data}", "-server=1", "-txindex=0", "-prune=0",
                               "-listen=0", "-connect=0", "-dnsseed=0", "-discover=0", "-dbcache=64", f"-rpcport={port}"])
        rpc = Rpc(port, data / "regtest/.cookie")
        processes.wait(lambda: rpc("getnetworkinfo"), f"{name} RPC")
        assert rpc("getblockcount") == 0
        assert rpc("getindexinfo") == {}
        return data, rpc

    try:
        _, producer = core_start("producer")
        env = dict(os.environ, CARGO_BUILD_JOBS="2", P65_CORE_URL=producer.url,
                   P65_CORE_COOKIE=str(producer.cookie), P65_CHAIN_RESULT=str(root / "chain.json"))
        with (root / "fixture-build.log").open("w") as log:
            subprocess.run(["cargo", "test", "--offline", "--locked", "--manifest-path", "src/btc/Cargo.toml", "-p", "usdb-indexer",
                            "--bin", "usdb-indexer", "generate_assumeutxo_service_chain", "--", "--ignored", "--test-threads=1"],
                           env=env, cwd=repo, stdout=log, stderr=log, check=True)
        chain = json.loads((root / "chain.json").read_text())
        processes.stop("producer")
        data, btc = core_start("consumer")
        blocks = {b["height"]: b for b in chain["blocks"]}

        def submit(items):
            for block in items:
                assert btc("submitblock", block["raw"]) is None

        submit([blocks[h] for h in range(1, 102)])
        assert btc("getblockhash", 101) == chain["base_hash"]
        source = root / "snapshot.dat"
        shutil.copyfile(fixture / "snapshot.dat", source)
        identity = json.loads((fixture / "identity.json").read_text())
        checkpoint = json.loads((fixture / "checkpoint.json").read_text())
        ports = {name: free_port() for name in ["native-bh", "full-bh", "native-indexer", "full-indexer"]}
        apis = {name: Rpc(port) for name, port in ports.items()}
        proxy = CoreProxy(btc)
        proxy.missing_undo = blocks[103]["hash"]

        def table(name, fields):
            return f"[{name}]\n" + "\n".join(f"{key} = {json.dumps(value)}" for key, value in fields.items()) + "\n"

        for mode in ["native", "full"]:
            bh_name, ix_name = f"{mode}-bh", f"{mode}-indexer"
            bh_root, ix_root = root / bh_name, root / ix_name
            bh_root.mkdir()
            ix_root.mkdir()
            config = table("btc", dict(network="regtest", data_dir=str(data / "regtest"), rpc_url=btc.url))
            config += "[ordinals]\n[electrs]\n"
            config += table("sync", dict(batch_size=2, local_loader_threshold=0 if mode == "native" else 4294967295,
                                          utxo_max_cache_bytes=4194304, balance_max_cache_bytes=4194304))
            config += table("rpc_server", dict(host="127.0.0.1", port=ports[bh_name]))
            if mode == "native":
                config += table("bootstrap", dict(snapshot_file=str(source), import_batch_size=31, replay_batch_size=2))
                config += table("bootstrap.identity", dict(origin_height=102, origin_block_hash=blocks[102]["hash"]))
                config += table("bootstrap.identity.snapshot", identity)
                config += table("bootstrap.identity.regtest_checkpoint", {k: v for k, v in checkpoint.items() if k != "snapshot"})
                config += table("bootstrap.identity.regtest_checkpoint.snapshot", checkpoint["snapshot"])
            (bh_root / "config.toml").write_text(config)
            indexer_config = dict(bitcoin=dict(network="regtest", rpc_url=Rpc(proxy.port).url if mode == "native" else btc.url,
                                              auth="None" if mode == "native" else {"CookieFile": str(btc.cookie)}),
                                  ordinals={}, balance_history=dict(rpc_url=apis[bh_name].url),
                                  usdb=dict(genesis_block_height=102, inscription_source="bitcoind", inscription_source_shadow_compare=False,
                                            upstream_poll_interval_ms=100, rpc_server_host="127.0.0.1", rpc_server_port=ports[ix_name]))
            (ix_root / "config.json").write_text(json.dumps(indexer_config, indent=2) + "\n")

        def service_start(name):
            processes.start(name, [bh_binary if name.endswith("-bh") else indexer_binary,
                                   "--root-dir", root / name, "--skip-process-lock"])

        for name in ports:
            service_start(name)

        def bootstrap_waiting():
            progress = json.loads((root / "native-bh/bootstrap-progress.json").read_text())
            return progress if progress.get("phase") == "waiting_for_blocks" and progress.get("height") == 101 else None

        progress = processes.wait(bootstrap_waiting, "native bootstrap waiting below G")
        assert not (root / "native-bh/db/balance_history").exists()
        try:
            readiness = apis["native-indexer"]("get_readiness")
            assert not readiness["consensus_ready"]
        except OSError:
            readiness = {"rpc_alive": False, "consensus_ready": False}
        stage("startup_before_G", btc_tip=101, bootstrap=progress, indexer=readiness)
        processes.stop("native-bh", crash=True)
        service_start("native-bh")
        processes.wait(bootstrap_waiting, "bootstrap restart below G")

        def ready(name, height):
            value = apis[name]("get_readiness")
            field = "stable_height" if name.endswith("-bh") else "synced_block_height"
            return value if value["consensus_ready"] and value.get(field) == height else None

        submit([blocks[h] for h in range(102, 114)])
        for name in ["native-bh", "full-bh", "full-indexer"]:
            processes.wait(lambda name=name: ready(name, 103), f"{name} ready at 103")

        def undo_blocked():
            value = apis["native-indexer"]("get_readiness")
            return value if value.get("block_processing_pending_height") == 103 and proxy.faults["missing_undo"] else None

        blocked = processes.wait(undo_blocked, "indexer missing historical prevout")
        assert not blocked["consensus_ready"] and blocked["synced_block_height"] == 102
        try:
            capture_anchor(apis["native-bh"], apis["native-indexer"], 103)
        except AssertionError:
            pass
        else:
            raise AssertionError("Anchor capture accepted an indexer blocked on undo")
        stage("missing_undo", readiness=blocked, faults=dict(proxy.faults))
        undo_faults_before_restart = proxy.faults["missing_undo"]
        processes.stop("native-indexer", crash=True)
        service_start("native-indexer")
        processes.wait(lambda: undo_blocked() if proxy.faults["missing_undo"] > undo_faults_before_restart else None,
                       "indexer missing undo after crash")
        proxy.missing_undo = None
        processes.wait(lambda: ready("native-indexer", 103), "indexer recovered mint")
        pass_id = chain["pass_id"]

        def snapshot(mode, height):
            return apis[f"{mode}-indexer"]("get_pass_snapshot", dict(inscription_id=pass_id, at_height=height))

        minted = snapshot("native", 103)
        assert minted and minted["state"] == "active" and minted["owner"] == chain["owner_a"]
        assert minted["satpoint"] == chain["mint_satpoint"]
        stage("mint_recovered", snapshot=minted)

        proxy.missing_block = blocks[104]["hash"]
        submit([blocks[114]])
        processes.wait(lambda: ready("full-indexer", 104), "reference transfer at 104")
        processes.wait(lambda: proxy.faults["missing_block"] > 0, "missing block fault reached")
        missing_block = apis["native-indexer"]("get_readiness")
        assert not missing_block["consensus_ready"] and missing_block["synced_block_height"] == 103
        assert missing_block["query_ready"] and snapshot("native", 103) == minted
        stage("missing_block", readiness=missing_block, faults=dict(proxy.faults))
        proxy.missing_block = None
        submit([blocks[h] for h in range(115, 119)])

        def compare(height, expected_satpoint, expected_owner):
            for name in ports:
                processes.wait(lambda name=name: ready(name, height), f"{name} ready at {height}")
            records = []
            for h in range(102, height + 1):
                for method, params, suffix in [
                    ("get_block_commit", (h,), "bh"),
                    ("get_state_ref_at_height", (dict(block_height=h),), "bh"),
                    ("get_pass_block_commit", (dict(block_height=h),), "indexer"),
                    ("get_state_ref_at_height", (dict(block_height=h),), "indexer"),
                ]:
                    native = apis[f"native-{suffix}"](method, *params)
                    full = apis[f"full-{suffix}"](method, *params)
                    assert native is not None and native == full, (h, method, native, full)
                    records.append(dict(height=h, method=method, service=suffix, value=native))
            for method in ["get_local_state_commit_info", "get_system_state_info"]:
                native = apis["native-indexer"](method)
                assert native is not None and native == apis["full-indexer"](method), method
                records.append(dict(method=method, value=native))
            for h in range(103, height + 1):
                assert snapshot("native", h) == snapshot("full", h)
                params = dict(inscription_id=pass_id, block_height=h, mode="at_or_before")
                native = apis["native-indexer"]("get_pass_energy", params)
                assert native == apis["full-indexer"]("get_pass_energy", params)
                records.append(dict(method="get_pass_energy", height=h, value=native))
                for owner in [chain["owner_a"], chain["owner_b"]]:
                    params = dict(script_hash=owner, block_height=h)
                    native = apis["native-bh"]("get_address_balance", params)
                    assert native == apis["full-bh"]("get_address_balance", params)
                    records.append(dict(method="get_address_balance", height=h, owner=owner, value=native))
            current = snapshot("native", height)
            assert current["satpoint"] == expected_satpoint and current["owner"] == expected_owner
            assert proxy.calls["getrawtransaction"] == 0
            return dict(height=height, snapshot=current, comparisons=records)

        initial = compare(108, chain["transfer_satpoint"], chain["owner_b"])
        assert initial["snapshot"]["state"] == "dormant"
        result["original"] = initial
        result["original_anchor"] = capture_anchor(apis["native-bh"], apis["native-indexer"], 108)
        stage("original_chain_equal", height=108, comparisons=len(initial["comparisons"]))

        # The mint owner now has no coins, but its observed script mapping remains useful.
        registry = apis["native-bh"]("resolve_script_hashes", dict(script_hashes=[chain["owner_a"], chain["owner_b"]], include_script_pubkey=True))
        assert registry["registry"]["coverage_mode"] == "post_snapshot_only"
        assert registry["registry"]["registry_artifact_id"] is None
        assert all(item["status"] == "found_overlay" and item["address"] for item in registry["items"])
        zero_balance = apis["native-bh"]("get_address_balance", dict(script_hash=chain["owner_a"], block_height=108))
        assert zero_balance[-1]["balance"] == 0
        rejections = []
        for method, params in [
            ("get_address_balance", dict(script_hash=chain["owner_a"], block_height=101)),
            ("get_address_balance_delta", dict(script_hash=chain["owner_a"], block_height=102)),
            ("get_state_ref_at_height", dict(block_height=101)),
        ]:
            try:
                apis["native-bh"](method, params)
            except RpcError as error:
                assert error.error["code"] == -32048, error
                rejections.append(dict(method=method, error=error.error))
            else:
                raise AssertionError(f"{method} accepted a query below the native floor")
        result["registry"] = registry
        stage("query_floors_and_zero_balance_registry", rejected=rejections, zero_balance=zero_balance)

        # A sealed native DB must not depend on re-opening the imported snapshot file.
        source.unlink()
        for name in ["native-indexer", "native-bh"]:
            processes.stop(name, crash=True)
        for name in ["native-bh", "native-indexer"]:
            service_start(name)
        restarted = compare(108, chain["transfer_satpoint"], chain["owner_b"])
        assert restarted == initial
        stage("sealed_crash_restart_equal", height=108, import_source_removed=True)

        btc("invalidateblock", blocks[104]["hash"])
        submit(chain["fork"])
        assert btc("getblockcount") == 119
        fork_result = compare(109, chain["fork_satpoint"], chain["owner_a"])
        assert fork_result["snapshot"]["state"] == "active"
        fork_energy = apis["native-indexer"]("get_pass_energy", dict(inscription_id=pass_id, block_height=109, mode="at_or_before"))
        assert int(fork_energy["effective_energy"]) > 0 and fork_energy["owner_balance"] == 700_000_000
        result["fork"] = fork_result
        result["fork_anchor"] = capture_anchor(apis["native-bh"], apis["native-indexer"], 109)
        stage("reorg_equal", fork_height=104, stable_height=109, comparisons=len(fork_result["comparisons"]))
        assert initial["snapshot"]["owner"] != fork_result["snapshot"]["owner"]
        result.update(status="pass", core_version=btc("getnetworkinfo")["subversion"], indexes=btc("getindexinfo"),
                      bootstrap=apis["native-bh"]("get_bootstrap_info"), proxy_calls=dict(proxy.calls), faults=dict(proxy.faults),
                      binary_sha256={p.name: file_sha256(p) for p in [args.bitcoind, bh_binary, indexer_binary]},
                      source_head=subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip(),
                      harness_sha256={name: file_sha256(repo / "tests" / name) for name in [
                          "run_assumeutxo_p65_services.py", "assumeutxo_service_fixture.rs",
                          "assumeutxo_stale_anchor.rs", "common/assumeutxo_services.py", "check_assumeutxo_p65_mainnet.py"]},
                      production_source_sha256={name: file_sha256(repo / name) for name in [
                          "src/btc/usdb-indexer/src/index/indexer.rs"]})
    except BaseException as error:
        result.update(status="fail", error=str(error))
        raise
    finally:
        cleanup_errors = []
        for close in [processes.close] + ([proxy.close] if proxy else []):
            try:
                close()
            except Exception as error:
                cleanup_errors.append(repr(error))
        if cleanup_errors:
            result.update(status="fail", cleanup_errors=cleanup_errors)
        result["disk_bytes_after_shutdown"] = sum(p.stat().st_size for p in root.rglob("*") if p.is_file())
        result.update(elapsed_seconds=round(time.monotonic() - start, 3), sampled_peak_rss_kib=processes.peak_rss_kib)
        (root / "result.json").write_text(json.dumps(result, indent=2) + "\n")
        print(f"P6.5 {result['status']}: {root / 'result.json'}", flush=True)
        if cleanup_errors:
            raise RuntimeError(cleanup_errors)


if __name__ == "__main__":
    main()
