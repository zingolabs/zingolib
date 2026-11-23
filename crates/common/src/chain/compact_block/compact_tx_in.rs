use super::{InvalidCompactBlock, fixed_bytes};
use zcash_protocol::TxId;

/// A transparent input, naming the output it spends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactTxIn {
    /// The txid of the transaction that created the output being spent.
    pub prevout_txid: TxId,
    /// The index of the output being spent in that transaction's `vout`.
    pub prevout_index: u32,
}

impl CompactTxIn {
    /// Parses a compact transparent input from its wire fields.
    pub fn from_wire(
        prevout_txid: Vec<u8>,
        prevout_index: u32,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            prevout_txid: TxId::from_bytes(fixed_bytes("CompactTxIn.prevout_txid", prevout_txid)?),
            prevout_index,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::CompactTxIn;
    use zcash_protocol::TxId;

    #[test]
    fn a_32_byte_txid_is_accepted() {
        let tx_in = CompactTxIn::from_wire(vec![0xaa; 32], 3).unwrap();
        assert_eq!(tx_in.prevout_txid, TxId::from_bytes([0xaa; 32]));
        assert_eq!(tx_in.prevout_index, 3);
    }

    #[test]
    fn a_wrong_length_txid_is_rejected() {
        assert!(CompactTxIn::from_wire(vec![0xaa; 20], 0).is_err());
    }
}
