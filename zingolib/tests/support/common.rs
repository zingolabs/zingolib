//! Record-fabrication rig shared by this crate's integration tests.
#![allow(dead_code)]

use std::str::FromStr as _;

use pepper_sync::wallet::{IronwoodNote, OutgoingIronwoodNote, OutputId, WalletTransaction};
use zcash_primitives::transaction::TxId;
use zcash_protocol::memo::Memo;
use zingo_common_components::protocol::ActivationHeights;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingolib::config::ChainType;
use zingolib::mocks::orchard_note::OrchardCryptoNoteBuilder;
use zingolib::testutils::synthetic_wallet::SyntheticWalletBuilder;
use zingolib::wallet::LightWallet;

pub fn regtest_wallet(mnemonic_phrase: &str) -> LightWallet {
    SyntheticWalletBuilder::new(mnemonic_phrase).build()
}

/// The wallet's first orchard receiver.
pub fn own_orchard_receiver(wallet: &LightWallet) -> orchard::Address {
    *wallet
        .unified_addresses()
        .values()
        .next()
        .unwrap()
        .orchard()
        .unwrap()
}

/// The Zennies-for-Zingo unified address for the regtest chain.
pub fn zfz_unified_address() -> zcash_keys::address::UnifiedAddress {
    let network = ChainType::Regtest(ActivationHeights::default());
    let zcash_keys::address::Address::Unified(address) = zcash_keys::address::Address::decode(
        &network,
        zingolib::get_zennies_for_zingo_address(network),
    )
    .unwrap() else {
        panic!("ZFZ address must be unified");
    };
    address
}

pub fn received(txid_byte: u8, height: u32, memos: &[Memo]) -> WalletTransaction {
    let txid = TxId::from_bytes([txid_byte; 32]);
    WalletTransaction::new_for_test_with_ironwood_notes(
        txid,
        ConfirmationStatus::Confirmed(height.into()),
        memos
            .iter()
            .enumerate()
            .map(|(index, memo)| {
                IronwoodNote::new_for_test(
                    OutputId::new(txid, index as u32),
                    zip32::AccountId::ZERO,
                    zip32::Scope::External,
                    OrchardCryptoNoteBuilder::default().build(),
                    memo.clone(),
                    None,
                )
            })
            .collect(),
        vec![],
    )
}

/// As [`received`], but taking plain strings, as the pre-extraction test
/// rig did.
pub fn received_texts(txid_byte: u8, height: u32, memos: &[&str]) -> WalletTransaction {
    let memos = memos
        .iter()
        .map(|memo| Memo::from_str(memo).unwrap())
        .collect::<Vec<_>>();
    received(txid_byte, height, &memos)
}

pub fn sent(
    txid_byte: u8,
    height: u32,
    recipient: &zcash_keys::address::UnifiedAddress,
    memo: &str,
) -> WalletTransaction {
    let txid = TxId::from_bytes([txid_byte; 32]);
    WalletTransaction::new_for_test_with_ironwood_notes(
        txid,
        ConfirmationStatus::Confirmed(height.into()),
        vec![],
        vec![OutgoingIronwoodNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            OrchardCryptoNoteBuilder::default().build(),
            Memo::from_str(memo).unwrap(),
            Some(recipient.clone()),
        )],
    )
}

/// A basic send-to-self: one internally self-received note whose outgoing
/// view is addressed to `own_receiver`.
pub fn self_send(txid_byte: u8, height: u32, own_receiver: orchard::Address) -> WalletTransaction {
    let txid = TxId::from_bytes([txid_byte; 32]);
    WalletTransaction::new_for_test_with_ironwood_notes(
        txid,
        ConfirmationStatus::Confirmed(height.into()),
        vec![IronwoodNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::Internal,
            OrchardCryptoNoteBuilder::default().build(),
            Memo::Empty,
            None,
        )],
        vec![OutgoingIronwoodNote::new_for_test(
            OutputId::new(txid, 0),
            zip32::AccountId::ZERO,
            zip32::Scope::External,
            OrchardCryptoNoteBuilder::default()
                .recipient(own_receiver)
                .build(),
            Memo::Empty,
            None,
        )],
    )
}

/// The Zennies-for-Zingo dual-output pattern: a send-to-self output to
/// `own_receiver` plus a send to the ZFZ address in one transaction.
pub fn zfz_self_send(
    txid_byte: u8,
    height: u32,
    own_receiver: orchard::Address,
) -> WalletTransaction {
    let txid = TxId::from_bytes([txid_byte; 32]);
    WalletTransaction::new_for_test_with_ironwood_notes(
        txid,
        ConfirmationStatus::Confirmed(height.into()),
        vec![],
        vec![
            OutgoingIronwoodNote::new_for_test(
                OutputId::new(txid, 0),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default()
                    .recipient(own_receiver)
                    .build(),
                Memo::Empty,
                None,
            ),
            OutgoingIronwoodNote::new_for_test(
                OutputId::new(txid, 1),
                zip32::AccountId::ZERO,
                zip32::Scope::External,
                OrchardCryptoNoteBuilder::default().build(),
                Memo::Empty,
                Some(zfz_unified_address()),
            ),
        ],
    )
}
