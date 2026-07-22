use bitcoincore_rpc::bitcoin::hashes::{self, Hash, sha256};
use bitcoincore_rpc::bitcoin::{Script, ScriptBuf};
use std::str::FromStr;

hashes::hash_newtype! {
    /// Electrum-compatible hash of a Bitcoin `scriptPubKey`.
    ///
    /// The serialized bytes are the SHA-256 digest of the script bytes in
    /// reverse byte order, matching the script-hash representation used by
    /// Electrum RPC.
    pub struct BtcScriptHash(sha256::Hash);
}

/// Converts a Bitcoin script into its Electrum-compatible script hash.
pub trait ToBtcScriptHash {
    /// Returns the reversed SHA-256 hash of this script's serialized bytes.
    fn to_btc_script_hash(&self) -> BtcScriptHash;
}

impl ToBtcScriptHash for Script {
    fn to_btc_script_hash(&self) -> BtcScriptHash {
        let mut result = sha256::Hash::hash(self.as_bytes()).to_byte_array();
        result.reverse();

        BtcScriptHash::from_byte_array(result)
    }
}

impl ToBtcScriptHash for ScriptBuf {
    fn to_btc_script_hash(&self) -> BtcScriptHash {
        self.as_script().to_btc_script_hash()
    }
}

/// Parses a Bitcoin address for `network` and returns its script hash.
pub fn address_string_to_script_hash(
    address: &str,
    network: &bitcoincore_rpc::bitcoin::Network,
) -> Result<BtcScriptHash, String> {
    let addr = bitcoincore_rpc::bitcoin::Address::from_str(address)
        .map_err(|e| format!("Invalid address {}: {}", address, e))?;
    let addr = addr
        .require_network(*network)
        .map_err(|e| format!("Address network mismatch for {}: {}", address, e))?;

    Ok(addr.script_pubkey().to_btc_script_hash())
}

/// Parses an Electrum-compatible Bitcoin script-hash string.
pub fn parse_script_hash(s: &str) -> Result<BtcScriptHash, String> {
    BtcScriptHash::from_str(s).map_err(|e| format!("Invalid script hash {}: {}", s, e))
}

/// Parses either a script-hash string or a Bitcoin address for `network`.
pub fn parse_script_hash_any(
    s: &str,
    network: &bitcoincore_rpc::bitcoin::Network,
) -> Result<BtcScriptHash, String> {
    if let Ok(sh) = parse_script_hash(s) {
        return Ok(sh);
    }

    address_string_to_script_hash(s, network)
}

#[cfg(test)]
mod tests {
    use super::*;
    use electrum_client::ToElectrumScriptHash;

    #[test]
    fn script_hash_matches_electrum_representation() {
        let script = Script::builder()
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_DUP)
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_HASH160)
            .push_slice([0u8; 20])
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_EQUALVERIFY)
            .push_opcode(bitcoincore_rpc::bitcoin::blockdata::opcodes::all::OP_CHECKSIG)
            .into_script();

        let btc_script_hash = script.to_btc_script_hash();
        let electrum_hash = script.as_script().to_electrum_scripthash();
        assert_eq!(
            *btc_script_hash.as_byte_array(),
            *electrum_hash,
            "BtcScriptHash should match Electrum ScriptHash"
        );
    }
}
