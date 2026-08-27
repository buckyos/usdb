use bitcoincore_rpc::bitcoin::Script;

/// Maximum script size used by Bitcoin Core's `CScript::IsUnspendable` check.
pub const BITCOIN_CORE_MAX_SCRIPT_SIZE: usize = 10_000;

/// Matches Bitcoin Core's UTXO exclusion rule for provably unspendable outputs.
///
/// This intentionally does not use rust-bitcoin's deprecated
/// `is_provably_unspendable`: that helper also classifies illegal opcodes and
/// therefore has different semantics from Bitcoin Core's UTXO set.
pub fn is_core_unspendable(script: &Script) -> bool {
    script.len() > BITCOIN_CORE_MAX_SCRIPT_SIZE || script.is_op_return()
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoincore_rpc::bitcoin::ScriptBuf;

    #[test]
    fn core_unspendable_matches_op_return_and_size_boundaries() {
        assert!(!is_core_unspendable(Script::new()));
        assert!(is_core_unspendable(Script::from_bytes(&[0x6a])));
        assert!(is_core_unspendable(Script::from_bytes(&[0x6a, 0x01, 0x01])));

        let maximum = ScriptBuf::from(vec![0x51; BITCOIN_CORE_MAX_SCRIPT_SIZE]);
        assert!(!is_core_unspendable(maximum.as_script()));

        let oversized = ScriptBuf::from(vec![0x51; BITCOIN_CORE_MAX_SCRIPT_SIZE + 1]);
        assert!(is_core_unspendable(oversized.as_script()));
    }

    #[test]
    fn core_unspendable_does_not_expand_to_illegal_opcode_policy() {
        let disabled_opcode = ScriptBuf::from(vec![0x7e]);
        assert!(!is_core_unspendable(disabled_opcode.as_script()));
    }
}
