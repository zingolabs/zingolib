//! Record-fabrication rig shared by this crate's integration tests:
//! deterministic offline wallets built through the pepper-sync
//! `new_for_test` constructors (datetime pinned to 0).
//!
//! Each test binary compiles this module separately and uses a subset
//! of the rig, so unused-in-this-binary items are expected.
#![allow(dead_code)]

use std::str::FromStr as _;

use pepper_sync::wallet::{IronwoodNote, OutgoingIronwoodNote, OutputId, WalletTransaction};
use zcash_primitives::transaction::TxId;
use zcash_protocol::memo::Memo;
use zingo_common_components::protocol::ActivationHeights;
use zingo_status::confirmation_status::ConfirmationStatus;
use zingolib::config::{ChainType, WalletConfig};
use zingolib::mocks::orchard_note::OrchardCryptoNoteBuilder;
use zingolib::testutils::default_test_wallet_settings;
use zingolib::wallet::LightWallet;

pub fn regtest_wallet(mnemonic_phrase: &str) -> LightWallet {
    LightWallet::new(
        ChainType::Regtest(ActivationHeights::default()),
        WalletConfig::MnemonicPhrase {
            mnemonic_phrase: mnemonic_phrase.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 1,
            wallet_settings: default_test_wallet_settings(),
        },
    )
    .unwrap()
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
    recipient: &zcash_client_backend::address::UnifiedAddress,
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
