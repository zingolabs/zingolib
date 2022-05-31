use zcash_primitives::consensus::{BlockHeight, BranchId, TestNetwork};
use zcash_primitives::sapling::{redjubjub::Signature, Node, Note, Rseed, ValueCommitment};
use zcash_primitives::transaction::components::{sapling, Amount, OutputDescription, GROTH_PROOF_SIZE};
use zcash_primitives::transaction::{Transaction, TransactionData, TxVersion};
pub fn new_transactiondata() -> TransactionData<zcash_primitives::transaction::Authorized> {
    let authorization = sapling::Authorized {
        binding_sig: Signature::read(&vec![0u8; 64][..]).expect("Signature read error!"),
    };
    let sapling_bundle = sapling::Bundle {
        shielded_spends: vec![],
        shielded_outputs: vec![],
        value_balance: Amount::zero(),
        authorization,
    };
    let mut td: TransactionData<zcash_primitives::transaction::Authorized> = TransactionData::from_parts(
        TxVersion::Sapling,
        BranchId::Sapling,
        0,
        0u32.into(),
        None,
        None,
        Some(sapling_bundle),
        None,
    );

    td
}
