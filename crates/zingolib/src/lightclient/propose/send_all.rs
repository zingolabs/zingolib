use zcash_protocol::PoolType;
use zcash_protocol::value::Zatoshis;

use crate::lightclient::LightClient;
use crate::testutils::synthetic_wallet::{self, SyntheticWalletBuilder};
use crate::utils::conversion::address_from_str;
use crate::wallet::error::ProposeSendError;
use crate::wallet::keys::unified::ReceiverSelection;

/// Migrated from libtonode `send_all::ptfm_insufficient_funds`: a
/// send-all whose only note cannot cover the fee of a cross-pool spend
/// reports the fee plus one as the required amount.
#[tokio::test]
async fn ptfm_insufficient_funds() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(10_000)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let proposal_error = client
        .propose_send_all(
            synthetic_wallet::external_zcash_address(PoolType::SAPLING),
            None,
            zip32::AccountId::ZERO,
        )
        .await;

    match proposal_error {
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds {
                available: a,
                required: r,
            },
        )) => {
            assert_eq!(a, Zatoshis::const_from_u64(10_000));
            assert_eq!(r, Zatoshis::const_from_u64(20_001));
        }
        _ => panic!("expected an InsufficientFunds error"),
    }
}

/// Migrated from libtonode `send_all::ptfm_zero_value`: a send-all
/// whose only note is entirely consumed by the fee is rejected as
/// insufficient funds. The 20_000-zat note exactly covers the V6 fee
/// (orchard input bundle plus ironwood payment bundle, two padded
/// actions each), leaving zero to send.
#[tokio::test]
async fn ptfm_zero_value() {
    let wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(20_000)
        .build();
    let mut client = LightClient::new_for_test(wallet).await;

    let proposal_error = client
        .propose_send_all(
            synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
            None,
            zip32::AccountId::ZERO,
        )
        .await;

    match proposal_error {
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds {
                available: a,
                required: r,
            },
        )) => {
            assert_eq!(a, Zatoshis::const_from_u64(20_000));
            assert_eq!(r, Zatoshis::const_from_u64(20_001));
        }
        _ => panic!("expected an InsufficientFunds error"),
    }
}

/// Migrated from libtonode `send_all::ptfm_general`: a send-all from a
/// wallet fragmented across both shielded pools drains every non-dust
/// note and leaves the dust behind. The original assembled the
/// fragmentation with five LocalNet sends and asserted zero
/// balances-excluding-dust after mining, which is this same contract,
/// one mined round trip later: send-all pays out the dust-excluded
/// spendable balance minus the fee, so notes at or below the 5_000-zat
/// `MARGINAL_FEE` dust line (which net nothing after paying for their
/// own input) are never selected.
#[tokio::test]
async fn ptfm_general() {
    let viable_values = [100_000u64, 50_000];
    let orchard_values = [100_000, 5_000, 4_000];
    let sapling_values = [50_000, 4_000];

    let mut builder = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED);
    for value in orchard_values {
        builder = builder.orchard_note(value);
    }
    for value in sapling_values {
        builder = builder.sapling_note(value);
    }
    let mut client = LightClient::new_for_test(builder.build()).await;

    let proposal = client
        .propose_send_all(
            synthetic_wallet::external_zcash_address(PoolType::SAPLING),
            None,
            zip32::AccountId::ZERO,
        )
        .await
        .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let mut selected: Vec<u64> = step
        .shielded_inputs()
        .expect("a shielded-funds send-all selects shielded inputs")
        .notes()
        .iter()
        .map(|note| u64::from(note.note().value()))
        .collect();
    selected.sort_unstable();
    assert_eq!(
        selected,
        [50_000, 100_000],
        "send-all selects exactly the non-dust notes of both pools"
    );
    let fee = u64::from(step.balance().fee_required());
    let payment: u64 = step
        .transaction_request()
        .payments()
        .values()
        .map(|payment| u64::from(payment.amount().expect("send-all payments carry amounts")))
        .sum();
    assert_eq!(payment + fee, viable_values.iter().sum::<u64>());
    let change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(change, 0, "send-all leaves no non-dust value behind");
}

/// Migrated from libtonode `fast::send_not_fully_synced` (renamed: no
/// sync exists offline). The original proposed a send-all to the
/// wallet's own sapling address while the validator ran five blocks
/// ahead, asserting only that propose-and-send succeeded. Proposing
/// never consults the server, so the offline half is exactly this
/// self-destination send-all. The behind-tip send half remains covered
/// by `load_wallet::verify_old_wallet_uses_server_height_in_send`.
#[tokio::test]
async fn send_all_to_own_sapling_proposes() {
    let note_value = 100_000;
    let mut wallet = SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
        .orchard_note(note_value)
        .build();
    let (_, own_sapling) = wallet
        .generate_unified_address(ReceiverSelection::sapling_only(), zip32::AccountId::ZERO)
        .unwrap();
    let destination = address_from_str(&own_sapling.encode(&wallet.chain_type())).unwrap();
    let mut client = LightClient::new_for_test(wallet).await;

    let proposal = client
        .propose_send_all(destination, None, zip32::AccountId::ZERO)
        .await
        .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let fee = u64::from(step.balance().fee_required());
    let payment: u64 = step
        .transaction_request()
        .payments()
        .values()
        .map(|payment| u64::from(payment.amount().expect("send-all payments carry amounts")))
        .sum();
    assert_eq!(payment + fee, note_value);
}

#[tokio::test]
async fn send_all_to_transparent_recipient() {
    let mut client = LightClient::new_for_test(
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .orchard_note(50_000)
            .build(),
    )
    .await;

    let proposal = client
        .propose_send_all(
            synthetic_wallet::external_zcash_address(PoolType::Transparent),
            None,
            zip32::AccountId::ZERO,
        )
        .await
        .unwrap();

    assert_eq!(proposal.steps().len(), 1);
    let step = proposal.steps().first();
    let fee = u64::from(step.balance().fee_required());
    let payment: u64 = step
        .transaction_request()
        .payments()
        .values()
        .map(|payment| u64::from(payment.amount().unwrap()))
        .sum();
    assert_eq!(payment + fee, 150_000);
    let change: u64 = step
        .balance()
        .proposed_change()
        .iter()
        .map(|change| u64::from(change.value()))
        .sum();
    assert_eq!(change, 0, "send-all leaves no non-dust value behind");
}

#[tokio::test]
async fn max_send_value_is_zero_for_an_empty_wallet() {
    let client = LightClient::new_for_test(
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED).build(),
    )
    .await;

    for address in [
        synthetic_wallet::external_zcash_address(PoolType::Transparent),
        synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
    ] {
        assert_eq!(
            client
                .max_send_value(address, zip32::AccountId::ZERO)
                .await
                .unwrap(),
            Zatoshis::ZERO
        );
    }
}

#[tokio::test]
async fn max_send_value_requires_a_scan_on_an_unsynced_wallet() {
    let config = crate::config::ClientConfig::builder()
        .set_wallet_config(crate::config::WalletConfig::MnemonicPhrase {
            mnemonic_phrase: zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 419200,
            wallet_settings: crate::testutils::default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let client = LightClient::new(config, true).await.unwrap();

    assert!(matches!(
        client
            .max_send_value(
                synthetic_wallet::external_zcash_address(PoolType::Transparent),
                zip32::AccountId::ZERO
            )
            .await,
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::ScanRequired
        ))
    ));
}

#[tokio::test]
async fn max_send_value_is_zero_when_fee_consumes_everything() {
    let client = LightClient::new_for_test(
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(20_000)
            .build(),
    )
    .await;

    assert_eq!(
        client
            .max_send_value(
                synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
                zip32::AccountId::ZERO
            )
            .await
            .unwrap(),
        Zatoshis::ZERO
    );
}

#[tokio::test]
async fn send_all_rejects_a_memo_to_a_transparent_recipient() {
    let mut client = LightClient::new_for_test(
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .build(),
    )
    .await;
    let memo = zcash_protocol::memo::MemoBytes::from_bytes(b"memo").unwrap();

    let proposal_error = client
        .propose_send_all(
            synthetic_wallet::external_zcash_address(PoolType::Transparent),
            Some(memo),
            zip32::AccountId::ZERO,
        )
        .await;
    assert!(matches!(
        proposal_error,
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::Payment(
                zcash_client_backend::zip321::PaymentError::TransparentMemo
            )
        ))
    ));
}

#[tokio::test]
async fn send_all_on_an_unsynced_wallet_requires_a_scan() {
    let config = crate::config::ClientConfig::builder()
        .set_wallet_config(crate::config::WalletConfig::MnemonicPhrase {
            mnemonic_phrase: zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED.to_string(),
            no_of_accounts: 1.try_into().unwrap(),
            birthday: 419200,
            wallet_settings: crate::testutils::default_test_wallet_settings(),
        })
        .build()
        .unwrap();
    let mut client = LightClient::new(config, true).await.unwrap();

    let proposal_error = client
        .propose_send_all(
            synthetic_wallet::external_zcash_address(PoolType::ORCHARD),
            None,
            zip32::AccountId::ZERO,
        )
        .await;
    assert!(matches!(
        proposal_error,
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::ScanRequired
        ))
    ));
}

fn external_tex_address() -> zcash_address::ZcashAddress {
    zcash_address::ZcashAddress::try_from_encoded(&synthetic_wallet::external_tex_address())
        .expect("a TEX address the stranger wallet just encoded parses")
}

/// Pins the zero-valued change note the send path always writes,
/// which the send-max sizing does not price. With two legacy Orchard
/// notes a post-NU6.3 orchard_v3 bundle costs spends + outputs rather
/// than max(spends, outputs), so that change output is worth one more
/// logical action, and the reported max must still send to a TEX
/// recipient.
#[tokio::test]
async fn max_send_value_to_tex_with_legacy_orchard_notes_is_sendable() {
    let mut client = LightClient::new_for_test(
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .orchard_note(50_000)
            .build(),
    )
    .await;
    let destination = external_tex_address();

    let max = client
        .max_send_value(destination.clone(), zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert!(
        max > Zatoshis::ZERO,
        "two legacy orchard notes must report a sendable max to a TEX address"
    );

    let request = zcash_client_backend::zip321::TransactionRequest::new(vec![
        zcash_client_backend::zip321::Payment::without_memo(destination.clone(), max),
    ])
    .unwrap();
    client
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .expect("the reported max must actually propose");

    let past_max = (max + Zatoshis::const_from_u64(1)).unwrap();
    let past_request = zcash_client_backend::zip321::TransactionRequest::new(vec![
        zcash_client_backend::zip321::Payment::without_memo(destination, past_max),
    ])
    .unwrap();
    assert!(matches!(
        client
            .propose_send(past_request, zip32::AccountId::ZERO)
            .await,
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds { .. }
        ))
    ));
}

/// Pins the same unpriced change output against a shielded
/// destination, where the two legacy Orchard notes still make it cost
/// an extra action but the send carries no padded transparent leg.
#[tokio::test]
async fn max_send_value_to_shielded_with_legacy_orchard_notes_is_sendable() {
    let mut client = LightClient::new_for_test(
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .orchard_note(100_000)
            .orchard_note(50_000)
            .build(),
    )
    .await;
    let destination = synthetic_wallet::external_zcash_address(PoolType::ORCHARD);

    let max = client
        .max_send_value(destination.clone(), zip32::AccountId::ZERO)
        .await
        .unwrap();
    assert!(
        max > Zatoshis::ZERO,
        "two legacy orchard notes must report a sendable max to a shielded address"
    );

    let request = zcash_client_backend::zip321::TransactionRequest::new(vec![
        zcash_client_backend::zip321::Payment::without_memo(destination.clone(), max),
    ])
    .unwrap();
    client
        .propose_send(request, zip32::AccountId::ZERO)
        .await
        .expect("the reported max must actually propose");

    let past_max = (max + Zatoshis::const_from_u64(1)).unwrap();
    let past_request = zcash_client_backend::zip321::TransactionRequest::new(vec![
        zcash_client_backend::zip321::Payment::without_memo(destination, past_max),
    ])
    .unwrap();
    assert!(matches!(
        client
            .propose_send(past_request, zip32::AccountId::ZERO)
            .await,
        Err(ProposeSendError::Proposal(
            zcash_client_backend::data_api::error::Error::InsufficientFunds { .. }
        ))
    ));
}

/// The selector refuses 80_000 from the Ironwood note alone.
#[tokio::test]
async fn send_all_to_tex_falls_back_when_the_selector_refuses_the_sized_amount() {
    let mut client = LightClient::new_for_test(
        SyntheticWalletBuilder::new(zingo_test_vectors::seeds::HOSPITAL_MUSEUM_SEED)
            .ironwood_note(100_000)
            .sapling_note(15_000)
            .build(),
    )
    .await;
    let destination = external_tex_address();

    let proposal = client
        .propose_send_all(destination.clone(), None, zip32::AccountId::ZERO)
        .await
        .expect("the send-max proposal stands in for the refused request");
    let fee: u64 = proposal
        .steps()
        .iter()
        .map(|step| step.balance().fee_required().into_u64())
        .sum();
    assert_eq!(
        crate::wallet::propose::recipient_amount(&proposal),
        Zatoshis::const_from_u64(80_000)
    );
    assert_eq!(fee, 35_000);

    assert_eq!(
        client
            .max_send_value(destination, zip32::AccountId::ZERO)
            .await
            .unwrap(),
        Zatoshis::const_from_u64(80_000)
    );
}
