"""Disposable installed release kits and opaque databases for upgrade integration tests."""
import copy
import hashlib
import json
from pathlib import Path
import shutil
from unittest import mock

import node_upgrade as upgrade
import runtime_compatibility as runtime
import usdb_node as node

ROOT = Path(__file__).resolve().parents[2]


def write_manifest(root, value):
    path = root / "release/usdb-release-manifest.json"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2) + "\n")
    path.with_name(path.name + ".sha256").write_text(hashlib.sha256(path.read_bytes()).hexdigest() + "  " + path.name + "\n")


class UpgradeFixture:
    def __init__(self, root, *, native=False):
        self.root, self.home = root, root / "home"
        self.home.mkdir()
        self.bundle = "usdb-testnet-v1"
        self.target_kit = root / "releases" / (self.bundle + "-r7")
        self.source_kit = root / "releases" / (self.bundle + "-r4")
        bundle = self.target_kit / "docker/networks" / self.bundle
        if native:
            from assumeutxo_deployment import prepare_bundle
            prepare_bundle(ROOT / "docker/networks/testnet-v1", bundle, "01" * 32, "", None, None)
        else:
            shutil.copytree(ROOT / "docker/networks/testnet-v1", bundle)
        self.network = node.build_network_identity(bundle)
        self.manifest = dict(schema_version=node.RELEASE_MANIFEST_SCHEMA_VERSION, release_id=self.bundle + "-r7",
                             network_bundle=self.network, runtime_compatibility=runtime.build_runtime_compatibility(self.network),
                             snapshot=node.build_snapshot_state(bundle), images={k: dict(reference=f"ghcr.io/buckyos/{image}@sha256:"+str(i)*64)
                             for i,(k,image) in enumerate((("usdb_services","usdb-services"),("usdb_chain","usdb-chain"),
                                 ("bitcoin_core","usdb-bitcoin-core"),("sourcedao_tools","sourcedao-bootstrap-tools")),1)})
        write_manifest(self.target_kit, self.manifest)
        self.config = self.home / ".config/usdb" / self.bundle
        self.env_path = self.config / "node.env"
        self.target = node.load_release_layout(self.target_kit, self.env_path)
        self.data = root / "data"
        capacity = node.DataRootCapacity(root, 3*1024**4, 3*1024**4)
        with mock.patch.object(node, '_data_root_capacity', return_value=capacity), mock.patch.object(node,'effective_memory_bytes',return_value=64*1024**3):
            node.configure_node(self.target, data_root=self.data, role='full', miner_address='', miner_threads=1,
                                bootnodes='', nat='', bitcoin_rpc_user='upgrade-fixture', bitcoin_p2p='private')
        self.old = copy.deepcopy(self.manifest)
        self.old['release_id'] = self.bundle + '-r4'
        self.old['network_bundle']['btc_activation_registry_id'] = 'a'*64
        self.old['images']['usdb_services']['reference'] = 'ghcr.io/buckyos/usdb-services@sha256:'+'9'*64
        self.old['runtime_compatibility'] = runtime.build_runtime_compatibility(self.old['network_bundle'])
        old_bundle = self.source_kit / 'docker/networks' / self.bundle
        shutil.copytree(bundle,old_bundle)
        raw=json.loads((old_bundle/'network.json').read_text())
        raw['btc_source']['activation_registry_id']='a'*64
        genesis=old_bundle/raw['artifacts']['genesis']['path']
        genesis.write_text(genesis.read_text()+'\n')
        raw['artifacts']['genesis']['sha256']=upgrade.sha(genesis)
        self.old['network_bundle']['genesis_sha256']=upgrade.sha(genesis)
        (old_bundle/'network.json').write_text(json.dumps(raw))
        self.old['network_bundle']['network_json_sha256']=upgrade.sha(old_bundle/'network.json')
        write_manifest(self.source_kit,self.old)
        target_env=node.read_env(self.env_path)
        paths=runtime.build_persistent_data_paths(self.data,self.old['network_bundle'],self.old['runtime_compatibility'])
        # Configure already-running old data without applying a future binary's registry allowlist.
        target_index=Path(target_env['USDB_INDEXER_DATA_HOST_DIR'])
        target_index.rename(paths['USDB_INDEXER_DATA_HOST_DIR'])
        for key,service in runtime.PERSISTENT_DATA_SERVICES.items():
            (paths[key]/runtime.DATASET_IDENTITY_FILE).write_text(json.dumps(runtime.build_dataset_identity(service,self.old['runtime_compatibility']),indent=2,sort_keys=True)+'\n')
            (paths[key]/'opaque.db').write_bytes((service+'-old-state').encode())
        node._atomic_write_private(self.env_path,node.upsert_env(self.env_path.read_text(),{
            **{k:str(p) for k,p in paths.items()},'USDB_RUNTIME_COMPATIBILITY_ID':self.old['runtime_compatibility']['compatibility_id'],
            'USDB_SERVICES_IMAGE':self.old['images']['usdb_services']['reference']}))
        self.paths=paths
        chain=paths['USDB_CHAIN_DATA_HOST_DIR']
        (chain/'geth').mkdir()
        (chain/'geth/nodekey').write_text('private-nodekey-fixture')
        (chain/'keystore').mkdir()
        (chain/'keystore/wallet.json').write_text('private-wallet-fixture')
        (self.config/'monitor/notifications').mkdir(parents=True)
        (self.config/'monitor/events.sqlite3').write_text('old-latched-incidents')
        (self.config/'monitor/config.json').write_text('{"settings":"keep"}')
        (self.config/'monitor/notifications/config.json').write_text('{"token":"private-notification-fixture"}')
        self.backup=root/'upgrade-backup'

    def rewrite_source(self):
        write_manifest(self.source_kit,self.old)

    def host_command(self,args,**kwargs):
        if args[:2]==['systemctl','show']:
            return 'LoadState=not-found\nActiveState=inactive\nUnitFileState=disabled\n'
        if args[:2]==['docker','ps']:
            return ''
        raise AssertionError('Unexpected host command: '+repr(args))
