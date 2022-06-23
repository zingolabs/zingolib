use zcash_primitives::consensus::BranchId;
use zcash_primitives::sapling::redjubjub::Signature;
use zcash_primitives::transaction::components::{sapling, Amount};
use zcash_primitives::transaction::{TransactionData, TxVersion};
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
    let td: TransactionData<zcash_primitives::transaction::Authorized> =
        TransactionData::from_parts(
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
