use bitcoincore_rpc::bitcoin::hashes::{self, Hash, sha256};
use bitcoincore_rpc::bitcoin::{Script, ScriptBuf};

hashes::hash_newtype! {
    pub struct USDBScriptHash(sha256::Hash);
}

pub trait ToUSDBScriptHash {
    fn to_usdb_script_hash(&self) -> USDBScriptHash;
}

impl ToUSDBScriptHash for Script {
    fn to_usdb_script_hash(&self) -> USDBScriptHash {
        // USDBScriptHash::hash(&self.as_bytes())
        // We use the same method as Electrum's script hash, which is sha256 of the script bytes, then reversed
        let mut result = sha256::Hash::hash(self.as_bytes()).to_byte_array();
        result.reverse();

        USDBScriptHash::from_byte_array(result)
    }
}

impl ToUSDBScriptHash for ScriptBuf {
    fn to_usdb_script_hash(&self) -> USDBScriptHash {
        self.as_script().to_usdb_script_hash()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use electrum_client::{ScriptHash as ElectrumScriptHash, ToElectrumScriptHash};

    #[test]
    fn test_script_hash() {
        let script = Script::builder()
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_DUP)
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_HASH160)
            .push_slice(&[0u8; 20])
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_EQUALVERIFY)
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_CHECKSIG)
            .into_script();

        let usdb_hash = script.to_usdb_script_hash();
        let electrum_hash = script.as_script().to_electrum_scripthash();
        //let right: &[u8; 32]  = electrum_hash.as_ref() ;
        assert_eq!(
            *usdb_hash.as_byte_array(),
            *electrum_hash,
            "USDBScriptHash should match Electrum ScriptHash"
        );
    }
}
