use std::collections::HashMap;

use super::pending_spends;
use super::*;
use zcash_primitives::transaction::{TransactionData, TxVersion};
use zcash_protocol::consensus::BranchId;
use zcash_transparent::address::Script;
use zcash_transparent::bundle::{Authorized, OutPoint, TxIn};
use zcash_transparent::keys::NonHardenedChildIndex;
use zip32::AccountId;
use zl_common::status::ConfirmationStatus;

use crate::engine::{TransparentCoinOps, WalletTransactionOps};
use crate::keys::transparent::{TransparentAddressId, TransparentScope};
use crate::wallet::{OutputId, TransparentCoin, WalletTransaction};

#[test]
fn pending_spends_reports_the_transparent_inputs_of_pending_records() {
    let funding_txid = TxId::from_bytes([1; 32]);
    let coin = TransparentCoin::from_parts(
        OutputId::new(funding_txid, 0),
        TransparentAddressId::new(
            AccountId::ZERO,
            TransparentScope::External,
            NonHardenedChildIndex::ZERO,
        ),
        String::new(),
        Script::default(),
        zcash_protocol::value::Zatoshis::const_from_u64(100_000),
    );
    let funding =
        WalletTransaction::new_for_test(funding_txid, ConfirmationStatus::Confirmed(10.into()))
            .with_transparent_coins_for_test(vec![coin]);

    let vin = vec![TxIn::from_parts(
        OutPoint::new([1; 32], 0),
        Script::default(),
        u32::MAX,
    )];
    let bundle = zcash_transparent::bundle::Bundle {
        vin,
        vout: Vec::new(),
        authorization: Authorized,
    };
    let pending_tx = TransactionData::from_parts(
        TxVersion::V4,
        BranchId::Sapling,
        0,
        BlockHeight::from_u32(11),
        Some(bundle),
        None,
        None,
        None,
    )
    .freeze()
    .unwrap();
    let pending_txid = pending_tx.txid();
    let pending_record = WalletTransaction::from_parts(
        pending_txid,
        ConfirmationStatus::Calculated(11.into()),
        pending_tx,
        0,
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );

    let mut wallet_transactions =
        HashMap::from([(funding_txid, funding), (pending_txid, pending_record)]);
    let spends = pending_spends(&wallet_transactions);
    assert_eq!(
        spends.get(&OutputId::new(funding_txid, 0)),
        Some(&pending_txid)
    );

    // Only pending records reserve their inputs: the same transaction
    // confirmed reports nothing, because a confirmed transaction's spends
    // are recorded on the outputs by the scan that found its block.
    wallet_transactions
        .get_mut(&pending_txid)
        .unwrap()
        .update_status(ConfirmationStatus::Confirmed(12.into()), 0, false);
    assert!(pending_spends(&wallet_transactions).is_empty());
}
