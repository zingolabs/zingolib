use pepper_sync::wallet::WalletTransaction;
use zcash_primitives::transaction::TxId;
use zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED;
use zl_common::status::ConfirmationStatus;

use crate::{
    config::{ChainType, WalletConfig},
    testutils::default_test_wallet_settings,
    wallet::{LightWallet, error::WalletError},
};

fn test_wallet() -> LightWallet {
    LightWallet::new(
        ChainType::Mainnet,
        WalletConfig::MnemonicPhrase {
            mnemonic_phrase: HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 419_200,
            wallet_settings: default_test_wallet_settings(),
        },
    )
    .unwrap()
}

#[test]
fn remove_failed_transaction_not_found() {
    let mut wallet = test_wallet();
    let txid = TxId::from_bytes([0u8; 32]);

    let result = wallet.remove_failed_transaction(txid);

    assert!(matches!(result, Err(WalletError::TransactionNotFound(id)) if id == txid));
}

#[test]
fn remove_failed_transaction_not_failed() {
    let mut wallet = test_wallet();
    let txid = TxId::from_bytes([1u8; 32]);
    let status = ConfirmationStatus::Calculated(1.into());
    wallet
        .wallet_transactions
        .insert(txid, WalletTransaction::new_for_test(txid, status));

    let result = wallet.remove_failed_transaction(txid);

    assert!(matches!(result, Err(WalletError::RemovalError)));
    assert!(wallet.wallet_transactions.contains_key(&txid));
}

#[test]
fn remove_failed_transaction_success() {
    let mut wallet = test_wallet();
    let txid = TxId::from_bytes([2u8; 32]);
    let status = ConfirmationStatus::Failed(1.into());
    wallet
        .wallet_transactions
        .insert(txid, WalletTransaction::new_for_test(txid, status));

    let result = wallet.remove_failed_transaction(txid);

    assert!(result.is_ok());
    assert!(!wallet.wallet_transactions.contains_key(&txid));
    assert!(wallet.save_required);
}
