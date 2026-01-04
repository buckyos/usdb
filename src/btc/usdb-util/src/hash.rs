use bitcoincore_rpc::bitcoin::hashes::{self, Hash, sha256};
use bitcoincore_rpc::bitcoin::{Script, ScriptBuf};
use std::str::FromStr;

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

pub fn address_string_to_script_hash(
    address: &str,
    network: &bitcoincore_rpc::bitcoin::Network,
) -> Result<USDBScriptHash, String> {
    let addr = bitcoincore_rpc::bitcoin::Address::from_str(address)
        .map_err(|e| format!("Invalid address {}: {}", address, e))?;
    let addr = addr
        .require_network(*network)
        .map_err(|e| format!("Address network mismatch for {}: {}", address, e))?;

    Ok(addr.script_pubkey().to_usdb_script_hash())
}

pub fn parse_script_hash(s: &str) -> Result<USDBScriptHash, String> {
    USDBScriptHash::from_str(s).map_err(|e| format!("Invalid script hash {}: {}", s, e))
}

pub fn parse_script_hash_any(s: &str, network: &bitcoincore_rpc::bitcoin::Network) -> Result<USDBScriptHash, String> {
    // Try to parse as USDBScriptHash first
    if let Ok(sh) = parse_script_hash(s) {
        return Ok(sh);
    }

    // Otherwise, try to parse as address
    address_string_to_script_hash(s, network)
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
