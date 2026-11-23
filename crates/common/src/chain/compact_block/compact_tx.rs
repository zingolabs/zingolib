use super::{
    CompactOrchardAction, CompactSaplingOutput, CompactSaplingSpend, CompactTxIn,
    InvalidCompactBlock, TxOut, fixed_bytes,
};
use zcash_protocol::TxId;

/// A compact transaction: the minimum shielded and transparent data a wallet needs to detect
/// payments to it and spends from it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactTx {
    /// The index of the transaction within the block.
    pub index: u64,
    /// The transaction's id, in protocol (not byte-reversed) order.
    pub txid: TxId,
    /// The transaction fee, when the server can provide it; zero otherwise.
    pub fee: u32,
    /// The Sapling spends.
    pub spends: Vec<CompactSaplingSpend>,
    /// The Sapling outputs.
    pub outputs: Vec<CompactSaplingOutput>,
    /// The Orchard actions.
    pub actions: Vec<CompactOrchardAction>,
    /// The Ironwood actions.
    pub ironwood_actions: Vec<CompactOrchardAction>,
    /// The transparent inputs.
    pub vin: Vec<CompactTxIn>,
    /// The transparent outputs.
    pub vout: Vec<TxOut>,
}

impl CompactTx {
    /// Parses a compact transaction's own scalar fields, taking its nested components
    /// (spends, outputs, actions, transparent inputs and outputs) already parsed.
    pub fn from_wire_parts(
        index: u64,
        txid: Vec<u8>,
        fee: u32,
        spends: Vec<CompactSaplingSpend>,
        outputs: Vec<CompactSaplingOutput>,
        actions: Vec<CompactOrchardAction>,
        ironwood_actions: Vec<CompactOrchardAction>,
        vin: Vec<CompactTxIn>,
        vout: Vec<TxOut>,
    ) -> Result<Self, InvalidCompactBlock> {
        Ok(Self {
            index,
            txid: TxId::from_bytes(fixed_bytes("CompactTx.txid", txid)?),
            fee,
            spends,
            outputs,
            actions,
            ironwood_actions,
            vin,
            vout,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{CompactTx, CompactTxIn};
    use zcash_protocol::TxId;

    #[test]
    fn a_32_byte_txid_is_accepted() {
        let tx = CompactTx::from_wire_parts(
            0,
            vec![0xaa; 32],
            0,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
            vec![],
        )
        .unwrap();
        assert_eq!(tx.txid, TxId::from_bytes([0xaa; 32]));
    }

    #[test]
    fn a_wrong_length_txid_is_rejected() {
        assert!(
            CompactTx::from_wire_parts(
                0,
                vec![0xaa; 31],
                0,
                vec![],
                vec![],
                vec![],
                vec![],
                vec![],
                vec![]
            )
            .is_err()
        );
    }

    /// Composing a transaction from already-validated nested components needs no further
    /// validation of its own: the struct carries whatever its caller already proved valid.
    /// This is what the structural change buys over a proto-typed contract — the bundling step
    /// itself has nothing left to check.
    #[test]
    fn already_validated_nested_components_pass_through_unchanged() {
        let spend = CompactTxIn::from_wire(vec![0xbb; 32], 1).unwrap();
        let tx = CompactTx::from_wire_parts(
            0,
            vec![0xaa; 32],
            1000,
            vec![],
            vec![],
            vec![],
            vec![],
            vec![spend],
            vec![],
        )
        .unwrap();
        assert_eq!(tx.fee, 1000);
        assert_eq!(tx.vin, vec![spend]);
    }
}
