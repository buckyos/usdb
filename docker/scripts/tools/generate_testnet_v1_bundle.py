#!/usr/bin/env python3
"""Reproduce the resettable testnet-v1 bundle from frozen v0 inputs and V2 rules."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import tempfile

from artifact_signing import canonical
from runtime_compatibility import build_persistent_data_paths, build_runtime_compatibility
from sourcedao_release import copy_public_bundle, semantic_digest
from usdb_node import upsert_env
from validate_network_bundle import read_env, read_json, require, validate_network_bundle

ROOT = Path(__file__).resolve().parents[3]
BUNDLE_ID = "usdb-testnet-v1"
CHAIN_ID = 202610030  # Frozen YYYYMMDD + sequence, not the generation or release number.


def generate(output: Path, activation_generator: Path, genesis_hash_tool: Path, geth_tool: Path, artifacts: Path) -> None:
    """Preserve reviewed economics and BTC inputs; regenerate every network binding."""
    require(not output.exists(), f"Output already exists: {output}")
    source = ROOT / "docker/networks/testnet-v0"
    network = validate_network_bundle(source)
    registry_path = ROOT / "src/btc/usdb-util/activation-registry/usdb-testnet-v1.json"
    golden = json.loads(subprocess.check_output([str(activation_generator), "--registry", str(registry_path)], text=True))
    registry_id = golden["registries"][0]["activation_registry_id"]
    copy_public_bundle(source, output, network)

    def write(relative: str, value: dict) -> None:
        path = output / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")

    def sha(relative: str) -> str:
        return hashlib.sha256((output / relative).read_bytes()).hexdigest()

    network.update(network_bundle_id=BUNDLE_ID, chain_id=CHAIN_ID, network_id=CHAIN_ID)
    network["btc_source"].update(rules_scope=BUNDLE_ID, activation_registry_id=registry_id)
    write("artifacts/btc-activation-registry-catalog.json", dict(
        schema_version="uip-0008-btc-activation-registry-catalog:v1", current_registry_id=registry_id,
        registries=[read_json(registry_path)]))
    network["artifacts"]["btc_activation_registry_catalog"] = dict(path="artifacts/btc-activation-registry-catalog.json")
    chain = read_json(output / "artifacts/usdb-chain-bootstrap-config.json")
    chain["chainId"] = CHAIN_ID
    chain["usdbConsensus"]["activations"][0]["btcActivationRegistryId"] = registry_id
    frozen_genesis = read_json(source / "artifacts/usdb-genesis.json")
    for contract in chain["predeploys"].values():
        artifact_file = artifacts / contract["artifact"]
        runtime = read_json(artifact_file)["deployedBytecode"]
        account = frozen_genesis["alloc"][contract["address"][2:].lower()]
        require(runtime == account["code"], f"SourceDAO runtime changed: {contract['artifact']}")
        contract["artifactSha256"] = hashlib.sha256(artifact_file.read_bytes()).hexdigest()
    write("artifacts/usdb-chain-bootstrap-config.json", chain)
    dao = read_json(output / "artifacts/sourcedao-bootstrap-config.json")
    dao["chainId"] = CHAIN_ID
    write("artifacts/sourcedao-bootstrap-config.json", dao)
    # Rebuild the system account: its fixed-price range is bound to the new chain ID.
    # A text edit of config.chainId leaves an invalid v0 system-state allocation.
    genesis = json.loads(subprocess.check_output([
        str(geth_tool), "--usdb", "dumpgenesis", "--usdb.bootstrap.config",
        str(output / "artifacts/usdb-chain-bootstrap-config.json"),
        "--usdb.bootstrap.artifacts", str(artifacts)], text=True))
    system_account = "0000000000000000000000000000000000001000"
    require(set(genesis["alloc"]) == set(frozen_genesis["alloc"]), "Fresh genesis changed allocation accounts")
    for address, account in frozen_genesis["alloc"].items():
        if address != system_account:
            require(genesis["alloc"][address] == account, f"Fresh genesis changed reviewed allocation: {address}")
    require(genesis["alloc"][system_account] != frozen_genesis["alloc"][system_account], "System state was not rebound")
    write("artifacts/usdb-genesis.json", genesis)
    block_hash = subprocess.check_output([str(genesis_hash_tool), str(output / "artifacts/usdb-genesis.json")], text=True).strip()
    manifest = read_json(output / "artifacts/usdb-genesis.manifest.json")
    require(block_hash != manifest["block_hash"], "Reset generation must have a distinct genesis block hash")
    manifest.update(network_bundle_id=BUNDLE_ID, chain_id=CHAIN_ID, network_id=CHAIN_ID,
                    block_hash=block_hash, file_sha256=sha("artifacts/usdb-genesis.json"),
                    bootstrap_config_sha256=sha("artifacts/usdb-chain-bootstrap-config.json"),
                    sourcedao_config_sha256=sha("artifacts/sourcedao-bootstrap-config.json"))
    # The release manifest records the eventual qualified toolchain revisions.
    # This file records frozen input provenance, not an uncommitted generator revision.
    manifest.pop("go_ethereum_revision", None)
    manifest["sourcedao_revision"] = "09591c29bfb27ca14d209de25ed8e2c71614b841"
    manifest["derivation"] = dict(source_network_bundle_id="usdb-testnet-v0",
                                  source_genesis_sha256=hashlib.sha256((source / "artifacts/usdb-genesis.json").read_bytes()).hexdigest(),
                                  allocation_policy="reuse-contracts-and-admin-rebuild-system-state")
    write("artifacts/usdb-genesis.manifest.json", manifest)
    freeze = read_json(output / "artifacts/sourcedao-bootstrap-freeze.json")
    freeze.update(network_bundle_id=BUNDLE_ID, chain_id=CHAIN_ID,
                  config_sha256=sha("artifacts/sourcedao-bootstrap-config.json"), config_semantic_sha256=semantic_digest(dao))
    write("artifacts/sourcedao-bootstrap-freeze.json", freeze)
    bootstrap = read_json(output / "artifacts/bootstrap-manifest.json")
    bootstrap.update(network_bundle_id=BUNDLE_ID, usdb_chain_id=CHAIN_ID, usdb_network_id=CHAIN_ID)
    write("artifacts/bootstrap-manifest.json", bootstrap)
    for relative in ("network.env", "compose.network.yml", "node.env.example", "bootnodes.json"):
        path = output / relative
        path.write_text(path.read_text().replace("usdb-testnet-v0", BUNDLE_ID))
    env = output / "network.env"
    env.write_text(upsert_env(env.read_text(), dict(USDB_CHAIN_ID=str(CHAIN_ID), USDB_NETWORK_ID=str(CHAIN_ID),
        BTC_ACTIVATION_REGISTRY_ID=registry_id, USDB_RULES_SCOPE=BUNDLE_ID,
        BTC_ACTIVATION_REGISTRY_CATALOG_FILE="/network/btc-activation-registry-catalog.json")))
    identity = dict(bundle_id=BUNDLE_ID, chain_id=CHAIN_ID, genesis_block_hash=block_hash,
                    btc_network_id=network["btc_source"]["network_id"],
                    btc_index_origin_height=network["btc_source"]["index_origin_height"],
                    btc_activation_registry_id=registry_id, btc_rules_scope=BUNDLE_ID)
    compatibility = build_runtime_compatibility(identity)
    template = output / "node.env.example"
    root = Path(read_env(template)["USDB_DATA_ROOT"])
    template.write_text(upsert_env(template.read_text(), dict(USDB_RUNTIME_COMPATIBILITY_ID=compatibility["compatibility_id"],
        **{key: str(value) for key, value in build_persistent_data_paths(root, identity, compatibility).items()})))
    (output / "README.md").write_text("""# USDB testnet-v1

开发测试网重置代际，chain ID / network ID 为 `202610030`。
BTC 主网 origin `963800`；独立 rules scope `usdb-testnet-v1`，从 origin 使用 JSON schema v1 + MinerPass 状态机 V2。
USDB 链从 block 0 开始；不导入 v0 区块、余额、运行期 SourceDAO 状态或 indexer 数据。
SourceDAO 的冻结初始分配、委员会、bootstrap admin 与 PoW 参数沿用 v0。

`network.json` 是固定身份输入；`release-bootstrap.json` 复用既有 Bitcoin 主网签名 UTXO 发布材料。
Candidate/Publish 根据 `usdb-testnet-v1-rN` 选择本目录并生成 AssumeUTXO 节点配置。
Bitcoin 基础数据可按兼容合同复用；indexer/链/控制面使用新目录。请勿对 v0 数据目录直接执行 init。

bootnodes 沿用原部署端点，须在运营方完成 v1 重置后验收连通性；文件存在不表示旧节点已经升级。
生成和发布步骤见 `doc/publish/usdb-testnet-v1-network-bundle.md`。
""")
    for entry in network["artifacts"].values():
        entry["sha256"] = sha(entry["path"])
    write("network.json", network)
    # These signed objects bind Bitcoin's snapshot, not a USDB chain generation.
    selection = read_json(source / "release-bootstrap.json")
    for entry in selection["files"].values():
        target = output / entry["path"]
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source / entry["path"], target)
    selection.update(network_bundle_id=BUNDLE_ID, source_network_sha256=sha("network.json"))
    (output / "release-bootstrap.json").write_bytes(canonical(selection))
    validate_network_bundle(output)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--activation-generator", type=Path, required=True)
    parser.add_argument("--genesis-hash-tool", type=Path, required=True)
    parser.add_argument("--geth-tool", type=Path, required=True)
    parser.add_argument("--sourcedao-artifacts", type=Path, default=ROOT.parent / "SourceDAO/artifacts")
    destination = parser.add_mutually_exclusive_group(required=True)
    destination.add_argument("--output-dir", type=Path)
    destination.add_argument("--check", action="store_true")
    args = parser.parse_args()
    try:
        with tempfile.TemporaryDirectory(prefix="usdb-testnet-v1-") as temporary:
            output = args.output_dir or Path(temporary) / "bundle"
            generate(output, args.activation_generator.resolve(), args.genesis_hash_tool.resolve(), args.geth_tool.resolve(), args.sourcedao_artifacts.resolve())
            if args.check:
                expected = ROOT / "docker/networks/testnet-v1"
                actual_files = {p.relative_to(output) for p in output.rglob("*") if p.is_file()}
                expected_files = {p.relative_to(expected) for p in expected.rglob("*") if p.is_file()}
                require(actual_files == expected_files, "Generated v1 bundle file inventory differs")
                for relative in sorted(actual_files):
                    require((output / relative).read_bytes() == (expected / relative).read_bytes(), f"Generated v1 artifact differs: {relative}")
            print(f"Testnet-v1 bundle verified: chain_id={CHAIN_ID}, output={output}")
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"Testnet-v1 generation failed: {error}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
